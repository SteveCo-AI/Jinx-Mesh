//! Exact rollback and dialog healing: /undo (steps and round) over the
//! op-journal suffix and the Kap.-21 heal commands with their journaled
//! state transitions.
use super::{changed_graph, metadata_after, App, Session, ACTIVATION_BUDGET};
use crate::{
    engine::Engine,
    graph,
    journal::{self, JournalVermerk},
    model::GraphSlice,
    store::Store,
};
use anyhow::{bail, Result};
use serde_json::json;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Undo-Umfang eines /undo-Befehls: die letzten n Schritte oder die ganze
/// Runde ab der letzten Konsolidierungsgrenze.
enum Umfang {
    Schritte(u64),
    Runde,
}

impl App {
    /// Roll back the last n committed learning steps exactly (Kap. 14).
    pub fn undo(&mut self, schritte: u64) -> Result<serde_json::Value> {
        self.undo_impl(Umfang::Schritte(schritte))
    }

    /// Roll back the whole current round: the suffix starting at the most
    /// recent consolidate boundary (inclusive), or the whole journal when no
    /// boundary exists.
    pub fn undo_runde(&mut self) -> Result<serde_json::Value> {
        self.undo_impl(Umfang::Runde)
    }

    fn undo_impl(&mut self, umfang: Umfang) -> Result<serde_json::Value> {
        self.tick();
        let s = self
            .session
            .as_mut()
            .filter(|s| !s.frozen)
            .ok_or_else(|| anyhow::anyhow!("Developer-Anmeldung erforderlich"))?;
        if self.store.version() < 3 {
            bail!("Rollback erfordert das Vault-Format 3");
        }
        // Undo always pops a journal suffix, so the ops of a step meet exactly
        // the state that step produced.
        let start = match umfang {
            Umfang::Schritte(n) => s
                .journal
                .len()
                .saturating_sub(usize::try_from(n).unwrap_or(usize::MAX)),
            Umfang::Runde => journal::runden_index(&s.journal),
        };
        if start >= s.journal.len() {
            return Ok(json!({"zurueckgerollt":0,"ops":0,"journal_länge":s.journal.len()}));
        }
        let alte_meta = s.payload.state.clone();
        let vor_cache = s.graph_cache.clone();
        let key = s.key.as_ref().expect("developer key");
        // F04 — the chosen span is rolled back on a bounded workspace, never
        // through the resident graph cache (that hydrated the whole span
        // into the inference state, and Engine::active_state takes it over
        // completely). The rollback runs in batches:
        // 1. one store-free scan collects the records the span rewrites and
        //    the neuron creations it deletes;
        // 2. record bases load in pages of at most ACTIVATION_BUDGET ids
        //    (Store::graph_nodes caps a page at 128 anyway) — resident bases
        //    from the pre-undo slice, cold bases from the vault;
        // 3. the journal ops apply in reverse onto those bases. Every op
        //    stores complete old values, so the workspace ends up holding
        //    the exact post-rollback records with COMPLETE outgoing edge
        //    maps — cold-section exactness (Kap. 14): a neuron op may
        //    re-insert a cold node, but the suffix's edge ops alone yield
        //    only the span's changed edges, never the stored set a commit
        //    must receive. Without the cold reload the commit would replace
        //    the record's edges with that partial map and silently drop
        //    every unchanged connection of the cold source (F02).
        // 4. ONE atomic Store::commit persists the workspace. Commit per
        //    batch was rejected on the existing store layout: Store::commit
        //    is a single transaction (audit, journal suffix deletion, graph
        //    upserts, snapshot together), and partial batch commits would
        //    leave a half-rolled-back vault whose op-journal no longer
        //    matches the persisted state — unrecoverable. A crash before
        //    the commit changes nothing at all.
        // Peak memory: the workspace grows with the span, but only as the
        // commit payload — Store::commit seals every changed record inside
        // its transaction regardless. The resident inference slice and
        // every store read stay bounded at 128 nodes throughout.
        let mut ops_gesamt = 0usize;
        // alt=None marks a neuron the step created; undoing the creating
        // step deletes the record instead of rewriting it.
        let mut geloeschte = std::collections::BTreeSet::new();
        let mut beruehrte = std::collections::BTreeSet::new();
        for op in s.journal[start..]
            .iter()
            .flat_map(|eintrag| eintrag.ops.iter())
        {
            ops_gesamt += 1;
            if op.kind == "neuron" && op.alt.is_none() {
                geloeschte.insert(op.ziel.clone());
            } else if op.kind == "neuron" || op.kind == "kante" {
                beruehrte.insert(op.ziel.clone());
            }
        }
        let mut arbeitscache = GraphSlice::default();
        for id in &beruehrte {
            if let Some(node) = vor_cache.nodes.get(id) {
                arbeitscache.nodes.insert(id.clone(), node.clone());
                arbeitscache.edges.insert(
                    id.clone(),
                    vor_cache.edges.get(id).cloned().unwrap_or_default(),
                );
            }
        }
        let kalte_quellen: Vec<String> = beruehrte
            .iter()
            .filter(|id| !vor_cache.nodes.contains_key(*id) && !geloeschte.contains(*id))
            .cloned()
            .collect();
        // Reads precede every mutation, so a failed page load leaves the
        // session untouched. From the first rolled-back op on, every error
        // discards the session instead (fail closed, F01 pattern).
        for chunk in kalte_quellen.chunks(ACTIVATION_BUDGET) {
            let page = self.store.graph_nodes(key, chunk)?;
            for (id, node) in page.nodes {
                arbeitscache.nodes.insert(id.clone(), node);
                arbeitscache
                    .edges
                    .insert(id.clone(), page.edges.get(&id).cloned().unwrap_or_default());
            }
        }
        let mut entfernte = Vec::new();
        for eintrag in s.journal[start..].iter().rev() {
            match journal::zurueckrollen(&mut arbeitscache, &mut s.payload.state, eintrag) {
                Ok(weg) => entfernte.extend(weg),
                Err(fehler) => {
                    self.session = None;
                    return Err(fehler);
                }
            }
        }
        // Post-undo resident slice: the restored records first, untouched
        // pre-undo residents fill the remaining budget — never larger than
        // the 128-node activation budget. The cap applies ONLY to the
        // inference working set; the commit below persists every touched
        // record exactly, so no edge is lost to a truncated cache.
        let mut nach_cache = GraphSlice::default();
        for (id, node) in &arbeitscache.nodes {
            if nach_cache.nodes.len() >= ACTIVATION_BUDGET {
                break;
            }
            nach_cache.nodes.insert(id.clone(), node.clone());
            nach_cache.edges.insert(
                id.clone(),
                arbeitscache.edges.get(id).cloned().unwrap_or_default(),
            );
        }
        for (id, node) in &vor_cache.nodes {
            if nach_cache.nodes.len() >= ACTIVATION_BUDGET {
                break;
            }
            if nach_cache.nodes.contains_key(id) || entfernte.contains(id) {
                continue;
            }
            nach_cache.nodes.insert(id.clone(), node.clone());
            nach_cache.edges.insert(
                id.clone(),
                vor_cache.edges.get(id).cloned().unwrap_or_default(),
            );
        }
        s.graph_cache = nach_cache;
        let aktiv = Engine::active_state(&s.payload.state, &s.graph_cache);
        match Engine::new(aktiv, s.payload.actions.clone()) {
            Ok(engine) => s.engine = engine,
            Err(fehler) => {
                self.session = None;
                return Err(fehler);
            }
        }
        s.feedback_target = s.payload.state.extra.get("feedback_target").cloned();
        let graph_updates = arbeitscache;
        let streichen_ab = s.journal[start].seq;
        let zurueck = s.journal.len() - start;
        s.journal.truncate(start);
        let vermerk = JournalVermerk {
            streichen_ab: Some(streichen_ab),
            graph_entfernungen: entfernte,
            ..JournalVermerk::default()
        };
        if let Err(e) = self.store.commit(
            &mut s.payload,
            &alte_meta,
            &graph_updates,
            s.key.as_ref().expect("developer key"),
            "undo",
            &[],
            &vermerk,
        ) {
            self.session = None;
            return Err(e);
        }
        s.last = Instant::now();
        Ok(json!({"zurueckgerollt":zurueck,"ops":ops_gesamt,"journal_länge":s.journal.len()}))
    }
    /// Kap.-21 dialog heal command: parse the argument through the configured
    /// pattern data (heal_befehl_muster, never a code formulation) and run
    /// the matching heal. Write mode only — /freeze refuses like the DEV
    /// feedback; every heal lands in the op-journal as art "heal" and is
    /// exactly rollbackable via /undo.
    pub fn heal_befehl(&mut self, befehl: &str, argument: &str) -> Result<serde_json::Value> {
        self.tick();
        let (x, y) = {
            let s = self
                .session
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Gesperrt: zuerst /login"))?;
            if s.frozen {
                bail!("Heilungen sind nur im Schreib-Modus erlaubt");
            }
            crate::healing::befehl_parsen(&s.engine.state, befehl, argument)?
        };
        match y {
            Some(y) => self.falsch(&x, &y),
            None if befehl == "echtes_wort" => self.echtes_wort(&x),
            None => bail!("Der Heil-Befehl {befehl} erwartet zwei Worte"),
        }
    }

    /// Kap. 21 "/falsch x ist y" (formulation from konfig data): x folds
    /// into the ring of y — every edge from or to x is rewritten onto y
    /// through the alias, the origin stays visible, and the whole step is
    /// journaled as art "heal".
    fn falsch(&mut self, x: &str, y: &str) -> Result<serde_json::Value> {
        self.tick();
        let s = self
            .session
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Gesperrt: zuerst /login"))?;
        if s.frozen {
            bail!("Heilungen sind nur im Schreib-Modus erlaubt");
        }
        // Kap. 22: without a registered verification source on the word
        // level the heal fails loudly — never silently, never approximately.
        crate::healing::ebene_pruefen(&s.engine.state, crate::healing::EBENE_WORT)?;
        let store = &self.store;
        let x_id = wort_laden(s, store, x)?;
        let y_id = wort_laden(s, store, y)?;
        if x_id == y_id {
            bail!("Ein Wort kann nicht in sich selbst gefoldet werden");
        }
        // Folding x into its own descendant would forge an alias cycle.
        let mut hop = y_id.clone();
        let mut besucht = std::collections::BTreeSet::new();
        while besucht.insert(hop.clone()) {
            match s
                .engine
                .state
                .extra
                .get("neuron_aliases")
                .and_then(|a| a.get(&hop))
                .and_then(serde_json::Value::as_str)
            {
                Some(next) if next == x_id => {
                    bail!("Folding würde einen Alias-Zykel erzeugen");
                }
                Some(next) => hop = next.to_owned(),
                None => break,
            }
        }
        let alte_meta = s.payload.state.clone();
        let vor_cache = s.graph_cache.clone();
        // Kap.-21 edge rewrite in the resident records: every edge from x
        // moves onto y, every edge into x moves onto y; a conflicting pair
        // keeps the stronger weight, exactly like the alias merge of the
        // healing pass. Nothing is deleted — the journal records every op.
        let mut umgebogen = 0usize;
        if let Some(quelle) = s.graph_cache.edges.remove(&x_id) {
            let ziele = s.graph_cache.edges.entry(y_id.clone()).or_default();
            for (b, gewicht) in quelle {
                umgebogen += 1;
                let slot = ziele.entry(b).or_insert(gewicht);
                if *slot < gewicht {
                    *slot = gewicht;
                }
            }
        }
        let quellen: Vec<String> = s.graph_cache.edges.keys().cloned().collect();
        for quelle in quellen {
            let Some(ziele) = s.graph_cache.edges.get_mut(&quelle) else {
                continue;
            };
            if let Some(gewicht) = ziele.remove(&x_id) {
                umgebogen += 1;
                let slot = ziele.entry(y_id.clone()).or_insert(gewicht);
                if *slot < gewicht {
                    *slot = gewicht;
                }
            }
        }
        s.engine = Engine::new(
            Engine::active_state(&s.payload.state, &s.graph_cache),
            s.payload.actions.clone(),
        )?;
        let y_label = s.engine.state.neuronen[&y_id].label.clone();
        {
            let zustand = &mut s.engine.state;
            let alias = zustand
                .extra
                .entry("neuron_aliases".into())
                .or_insert_with(|| json!({}));
            if let Some(map) = alias.as_object_mut() {
                map.insert(x_id.clone(), json!(y_id.clone()));
            }
            let x_label = zustand
                .neuronen
                .get_mut(&x_id)
                .ok_or_else(|| anyhow::anyhow!("Wort ist nicht im Netz: {x}"))?
                .label
                .clone();
            let knoten = zustand.neuronen.get_mut(&x_id).expect("geladenes Wort");
            knoten.ring = Some(y_id.clone());
            knoten.extra.insert("folding_origin".into(), json!(true));
            // Kap. 21 "lernbar": a correction beyond the effective distance
            // threshold raises the learned threshold in the state.
            crate::healing::folding_schwelle_anheben(zustand, &x_label, &y_label);
            // Kap. 22: the user correction is a confirmed heal on the word
            // level — the credit grows, the threshold sinks.
            crate::healing::erfolg(zustand, crate::healing::EBENE_WORT);
            zustand.validate()?;
        }
        Engine::merge_active(&mut s.graph_cache, &s.engine.state);
        s.payload.state = metadata_after(&alte_meta, &s.engine.state);
        let zeit = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let journal_seq = s
            .payload
            .op_journal_seq
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Journal voll"))?;
        let eintrag = journal::eintrag(
            journal_seq,
            s.engine.state.interaktionen,
            journal::ART_HEAL,
            zeit,
            journal::Ausschnitt {
                cache: &vor_cache,
                meta: &alte_meta,
            },
            journal::Ausschnitt {
                cache: &s.graph_cache,
                meta: &s.payload.state,
            },
        )?;
        let kap = journal::kap(&s.engine.state);
        if let Err(e) =
            self.store
                .journal_append(s.key.as_ref().expect("developer key"), &eintrag, kap)
        {
            self.session = None;
            return Err(e);
        }
        let vermerk = JournalVermerk {
            bestaetigt: Some(journal_seq),
            ..JournalVermerk::default()
        };
        let graph_updates = changed_graph(&vor_cache, &s.graph_cache);
        if let Err(e) = self.store.commit(
            &mut s.payload,
            &alte_meta,
            &graph_updates,
            s.key.as_ref().expect("developer key"),
            "heal",
            &[],
            &vermerk,
        ) {
            self.session = None;
            return Err(e);
        }
        s.journal.push(eintrag);
        while s.journal.len() > kap {
            s.journal.remove(0);
        }
        s.last = Instant::now();
        Ok(json!({
            "befehl": "falsch", "x": x, "y": y,
            "alias": x_id, "ziel": y_id,
            "kanten_umgebogen": umgebogen,
        }))
    }

    /// Kap. 21 "/echtes_wort x" (formulation from konfig data): x loses its
    /// ring and alias references (Rück-Folding) and becomes a regular
    /// standalone LEX neuron again. Journaled as art "heal".
    fn echtes_wort(&mut self, x: &str) -> Result<serde_json::Value> {
        self.tick();
        let s = self
            .session
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Gesperrt: zuerst /login"))?;
        if s.frozen {
            bail!("Heilungen sind nur im Schreib-Modus erlaubt");
        }
        crate::healing::ebene_pruefen(&s.engine.state, crate::healing::EBENE_WORT)?;
        let store = &self.store;
        let x_id = wort_laden(s, store, x)?;
        let ring = s
            .engine
            .state
            .neuronen
            .get(&x_id)
            .and_then(|n| n.ring.clone());
        let alias_ziel = s
            .engine
            .state
            .extra
            .get("neuron_aliases")
            .and_then(|a| a.get(&x_id).cloned());
        if ring.is_none() && alias_ziel.is_none() {
            bail!("Wort ist bereits ein eigenständiges Neuron: {x}");
        }
        let alte_meta = s.payload.state.clone();
        let vor_cache = s.graph_cache.clone();
        {
            let zustand = &mut s.engine.state;
            if let Some(alias) = zustand
                .extra
                .get_mut("neuron_aliases")
                .and_then(serde_json::Value::as_object_mut)
            {
                alias.remove(&x_id);
            }
            if let Some(knoten) = zustand.neuronen.get_mut(&x_id) {
                knoten.ring = None;
            }
            // The promotion is a confirmed heal on the word level (Kap. 22).
            crate::healing::erfolg(zustand, crate::healing::EBENE_WORT);
            zustand.validate()?;
        }
        Engine::merge_active(&mut s.graph_cache, &s.engine.state);
        s.payload.state = metadata_after(&alte_meta, &s.engine.state);
        let zeit = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let journal_seq = s
            .payload
            .op_journal_seq
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Journal voll"))?;
        let eintrag = journal::eintrag(
            journal_seq,
            s.engine.state.interaktionen,
            journal::ART_HEAL,
            zeit,
            journal::Ausschnitt {
                cache: &vor_cache,
                meta: &alte_meta,
            },
            journal::Ausschnitt {
                cache: &s.graph_cache,
                meta: &s.payload.state,
            },
        )?;
        let kap = journal::kap(&s.engine.state);
        if let Err(e) =
            self.store
                .journal_append(s.key.as_ref().expect("developer key"), &eintrag, kap)
        {
            self.session = None;
            return Err(e);
        }
        let vermerk = JournalVermerk {
            bestaetigt: Some(journal_seq),
            ..JournalVermerk::default()
        };
        let graph_updates = changed_graph(&vor_cache, &s.graph_cache);
        if let Err(e) = self.store.commit(
            &mut s.payload,
            &alte_meta,
            &graph_updates,
            s.key.as_ref().expect("developer key"),
            "heal",
            &[],
            &vermerk,
        ) {
            self.session = None;
            return Err(e);
        }
        s.journal.push(eintrag);
        while s.journal.len() > kap {
            s.journal.remove(0);
        }
        s.last = Instant::now();
        Ok(json!({
            "befehl": "echtes_wort", "x": x, "knoten": x_id,
            "ring_entfernt": ring.is_some(),
            "alias_entfernt": alias_ziel.is_some(),
        }))
    }
}

/// Resolve a Kap.-21 heal command's word to its resident node id, loading
/// the exact record from the vault on demand (bounded to that one record).
/// Cold legacy punctuation spellings cannot be found by id and fail
/// honestly, like every other resident-only lookup.
fn wort_laden(s: &mut Session, store: &Store, wort: &str) -> Result<String> {
    if let Some(id) = graph::wort_namentlich(&s.engine.state, wort) {
        return Ok(id);
    }
    let id = format!("lex:{wort}");
    let key = s
        .key
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Anmeldung erforderlich"))?;
    let page = store.graph_nodes(key, std::slice::from_ref(&id))?;
    if !page.nodes.contains_key(&id) {
        bail!("Wort ist nicht im Netz: {wort}");
    }
    for (knoten_id, knoten) in page.nodes {
        if let Some(kanten) = page.edges.get(&knoten_id) {
            s.graph_cache
                .edges
                .insert(knoten_id.clone(), kanten.clone());
        }
        s.graph_cache.nodes.insert(knoten_id, knoten);
    }
    s.engine = Engine::new(
        Engine::active_state(&s.payload.state, &s.graph_cache),
        s.payload.actions.clone(),
    )?;
    Ok(id)
}
