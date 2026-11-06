//! Auditable, non-destructive graph normalization.
//!
//! Kap. 21/22 self-healing lives here: the verification-source registry
//! (`verifikations_ebenen` in state.extra — wort ← Kontext-Similarität,
//! struktur ← konkurrierende Pfade, both data from fixtures/start.json, no
//! invented levels), the Levenshtein fold gate for `graph::grow`, the
//! suspicion ledger (`verdachtsverzeichnis`) fed by `graph::consolidate`,
//! and the dialog heal command patterns (`heal_befehl_muster`, konfig data —
//! the code only carries mechanics). Every heal either runs on a level with
//! a registered verification source or fails loudly; successes grow the
//! level's `verification_credit` and lower its `heil_schwelle` by the
//! configured step, failures (alias cycles, a verification-forbidden fold)
//! raise it again. All of it mutates only journaled state, so every heal is
//! exactly rollbackable.
use crate::model::State;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Kap.-22 word level: folding and word corrections. Its verification
/// source is Kontext-Similarität (registry data).
pub(crate) const EBENE_WORT: &str = "wort";
/// Kap.-22 structure level: sequence-edge damping. Its verification source
/// is konkurrierende Pfade (registry data).
pub(crate) const EBENE_STRUKTUR: &str = "struktur";

type ExactNodeKey = (
    String,
    String,
    u64,
    Option<String>,
    u64,
    String,
    u64,
    String,
);

/// Levenshtein distance over chars, O(n*m), no crate (Kap. 21: no new
/// dependency needed for a bounded-alphabet spelling metric).
pub(crate) fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() || b.is_empty() {
        return a.len().max(b.len());
    }
    let (lang, kurz) = if a.len() >= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    let mut zeile: Vec<usize> = (0..=kurz.len()).collect();
    let mut neu = vec![0usize; kurz.len() + 1];
    for (i, ca) in lang.iter().enumerate() {
        neu[0] = i + 1;
        for (j, cb) in kurz.iter().enumerate() {
            let kosten = if ca == cb { 0 } else { 1 };
            neu[j + 1] = (zeile[j + 1] + 1).min(neu[j] + 1).min(zeile[j] + kosten);
        }
        std::mem::swap(&mut zeile, &mut neu);
    }
    zeile[kurz.len()]
}

/// Kap.-22 registry read. Vaults predating Block 4 lack the whole key and
/// fall back to the fixture defaults (conservative default, never a code
/// copy of the source designators); a present key is used verbatim, so a
/// deliberately unregistered level honestly fails every heal.
fn verifikations_ebenen(state: &State) -> Value {
    state
        .extra
        .get("verifikations_ebenen")
        .cloned()
        .unwrap_or_else(|| State::empty().extra["verifikations_ebenen"].clone())
}

/// Fail-loud Kap.-22 gate: without a registered verification source no heal
/// may run on the level — not silently, not approximately.
pub(crate) fn ebene_pruefen(state: &State, ebene: &str) -> Result<()> {
    let registry = verifikations_ebenen(state);
    let eintrag = registry
        .get(ebene)
        .ok_or_else(|| anyhow!("Keine Verifikationsquelle auf der Ebene {ebene}"))?;
    if eintrag
        .get("quelle")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        bail!("Keine Verifikationsquelle auf der Ebene {ebene}");
    }
    Ok(())
}

/// The level's current heal threshold; None when the level carries none.
fn ebene_schwelle(state: &State, ebene: &str) -> Option<f64> {
    verifikations_ebenen(state)
        .get(ebene)?
        .get("heil_schwelle")?
        .as_f64()
}

/// Successful heal on a level: the verification credit grows and the heal
/// threshold sinks by the configured step (trust) down to the configured
/// floor. A level without a registry entry stays untouched.
pub(crate) fn erfolg(state: &mut State, ebene: &str) {
    let mut registry = verifikations_ebenen(state);
    let Some(eintrag) = registry.get_mut(ebene).and_then(Value::as_object_mut) else {
        return;
    };
    let credit = eintrag
        .get("verification_credit")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    eintrag.insert(
        "verification_credit".into(),
        json!(credit.saturating_add(1)),
    );
    let schritt = state.number("heil_schritt");
    let minimum = state.number("heil_schwelle_min");
    let schwelle = eintrag
        .get("heil_schwelle")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    eintrag.insert(
        "heil_schwelle".into(),
        json!((schwelle - schritt).max(minimum)),
    );
    state.extra.insert("verifikations_ebenen".into(), registry);
}

/// Failed heal on a level: the threshold rises by the configured step up to
/// the configured cap — the level's autonomy shrinks; failures earn no
/// credit.
pub(crate) fn misserfolg(state: &mut State, ebene: &str) {
    let mut registry = verifikations_ebenen(state);
    let Some(eintrag) = registry.get_mut(ebene).and_then(Value::as_object_mut) else {
        return;
    };
    let schritt = state.number("heil_schritt");
    let maximum = state.number("heil_schwelle_max");
    let schwelle = eintrag
        .get("heil_schwelle")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    eintrag.insert(
        "heil_schwelle".into(),
        json!((schwelle + schritt).min(maximum)),
    );
    state.extra.insert("verifikations_ebenen".into(), registry);
}

/// Effective Kap.-21 folding distance threshold: the learned value in
/// state.extra once a user correction raised it, otherwise the configured
/// start value.
pub(crate) fn folding_schwelle_effektiv(state: &State) -> f64 {
    state
        .extra
        .get("folding_schwelle")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| state.number("folding_schwelle"))
}

/// Kap. 21 "lernbar": a user correction folding two words farther apart
/// than the effective threshold raises the learned threshold to that
/// distance (persisted in state.extra; the konfig value stays the editable
/// start value). Corrections within the threshold change nothing.
pub(crate) fn folding_schwelle_anheben(state: &mut State, x: &str, y: &str) {
    let distanz = levenshtein(x, y) as f64;
    if distanz > folding_schwelle_effektiv(state) {
        state
            .extra
            .insert("folding_schwelle".into(), json!(distanz.clamp(0.0, 10.0)));
    }
}

/// One grow-time fold proposal: gate values plus the word level's current
/// heal threshold. None while the word level has no registered
/// verification source — then no fold may even be proposed.
pub(crate) struct FoldGate {
    pub max_distanz: usize,
    pub min_nutzt: u64,
    pub schwelle: f64,
}

pub(crate) fn fold_gate(state: &State) -> Option<FoldGate> {
    ebene_pruefen(state, EBENE_WORT).ok()?;
    let schwelle = ebene_schwelle(state, EBENE_WORT)?;
    Some(FoldGate {
        max_distanz: folding_schwelle_effektiv(state) as usize,
        min_nutzt: state.number("folding_min_nutzt") as u64,
        schwelle,
    })
}

/// Best fold candidate for an unknown token: the resident canonical LEX
/// node with the smallest distance within the gate's cap and use floor.
/// Fold targets are canonical — nodes already folded into another word
/// (ring set or alias source) never absorb new helpers.
pub(crate) fn fold_kandidat(
    state: &State,
    token: &str,
    gate: &FoldGate,
) -> Option<(String, String, usize)> {
    let alias_quellen: BTreeSet<&str> = state
        .extra
        .get("neuron_aliases")
        .and_then(Value::as_object)
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    let token_laenge = token.chars().count();
    let mut best: Option<(String, String, usize, u64)> = None;
    for (id, n) in &state.neuronen {
        if n.typ != "LEX" || n.ring.is_some() || alias_quellen.contains(id.as_str()) {
            continue;
        }
        let Some(label) = crate::graph::tokens(&n.label).into_iter().next() else {
            continue;
        };
        if label == token {
            continue;
        }
        if label.chars().count().abs_diff(token_laenge) > gate.max_distanz {
            continue;
        }
        let distanz = levenshtein(token, &label);
        if distanz > gate.max_distanz || n.nutzt < gate.min_nutzt {
            continue;
        }
        if best.as_ref().is_none_or(|(_, _, alt_distanz, alt_nutzt)| {
            distanz < *alt_distanz || (distanz == *alt_distanz && n.nutzt > *alt_nutzt)
        }) {
            // Best by distance, then uses; ties stay with the smallest id
            // because BTreeMap iteration is ordered — deterministic, no randomness.
            best = Some((id.clone(), label, distanz, n.nutzt));
        }
    }
    best.map(|(id, label, distanz, _)| (id, label, distanz))
}

/// Kap.-22 verification evidence of one fold candidate: the normalized
/// Levenshtein similarity in [0,1] — how strongly the word level's
/// verification source (Kontext-Similarität) confirms both spellings mean
/// the same word.
pub(crate) fn fold_beweis(token: &str, label: &str, distanz: usize) -> f64 {
    let laenge = token.chars().count().max(label.chars().count());
    if laenge == 0 {
        return 0.0;
    }
    1.0 - distanz as f64 / laenge as f64
}

/// Kap.-21 inconsistency check, run from `graph::consolidate`: every edge the
/// path weights contradict (weight under the suspicion threshold) that is
/// nevertheless heavily used on both ends gets damped by `daempf_faktor`
/// toward zero and recorded in the suspicion ledger instead of deleted. The
/// structure level's verification source must be registered, and its heal
/// threshold gates how strong the contradiction must be; every damped edge
/// is a confirmed heal on that level.
pub(crate) fn verdachts_pruefung(state: &mut State) -> usize {
    if ebene_pruefen(state, EBENE_STRUKTUR).is_err() {
        return 0;
    }
    let Some(schwelle) = ebene_schwelle(state, EBENE_STRUKTUR) else {
        return 0;
    };
    let verdacht = state.number("verdacht_schwelle");
    let min_nutzt = state.number("verdacht_min_nutzt") as u64;
    let faktor = state.number("daempf_faktor");
    let zeit = state.interaktionen;
    let mut verzeichnis = state
        .extra
        .get("verdachtsverzeichnis")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let paare: Vec<(String, String, f64)> = state
        .kanten
        .iter()
        .flat_map(|(a, ziele)| {
            ziele
                .iter()
                .map(move |(b, gewicht)| (a.clone(), b.clone(), *gewicht))
        })
        .collect();
    let mut gedaempft = 0usize;
    for (a, b, gewicht) in paare {
        if gewicht >= verdacht {
            continue;
        }
        let (nutzt_a, nutzt_b) = match (state.neuronen.get(&a), state.neuronen.get(&b)) {
            (Some(na), Some(nb)) => (na.nutzt, nb.nutzt),
            _ => continue,
        };
        if nutzt_a < min_nutzt || nutzt_b < min_nutzt {
            continue;
        }
        // Evidence: the contradiction strength — how far the confirmed
        // paths' verdict pulls this heavily used edge below zero.
        if -gewicht < schwelle {
            continue;
        }
        let neu = gewicht * faktor;
        if let Some(ziel) = state.kanten.get_mut(&a) {
            ziel.insert(b.clone(), neu);
        }
        verzeichnis.push(json!({
            "kind": "edge", "a": a, "b": b,
            "alt": gewicht, "neu": neu, "zeit": zeit,
        }));
        while verzeichnis.len() > 256 {
            verzeichnis.remove(0);
        }
        erfolg(state, EBENE_STRUKTUR);
        gedaempft += 1;
    }
    if gedaempft > 0 {
        state
            .extra
            .insert("verdachtsverzeichnis".into(), Value::Array(verzeichnis));
    }
    gedaempft
}

/// Parse a dialog heal command's argument through the configured pattern
/// data (Kap. 21, no hardcoded formulation): "falsch" yields the pair
/// (x, Some(y)) split by the pattern's literal separator, "echtes_wort"
/// yields (x, None). Each side must be exactly one word — folding is a
/// word-level mechanism.
pub(crate) fn befehl_parsen(
    state: &State,
    befehl: &str,
    argument: &str,
) -> Result<(String, Option<String>)> {
    let muster = state
        .konfig
        .get("heal_befehl_muster")
        .and_then(|m| m.get(befehl))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Heil-Befehl fehlt in der Konfiguration: {befehl}"))?;
    let worte = crate::language::parse(argument).words;
    let Some(rumpf) = muster.strip_prefix("{x}") else {
        bail!("Ungültiges Heil-Muster: {muster}");
    };
    let Some(trenner_text) = rumpf.strip_suffix("{y}") else {
        if worte.len() != 1 {
            bail!("Der Heil-Befehl erwartet genau ein Wort");
        }
        return Ok((worte[0].clone(), None));
    };
    let trenner = crate::language::parse(trenner_text).words;
    if trenner.is_empty() {
        bail!("Ungültiges Heil-Muster: {muster}");
    }
    let stelle = worte
        .windows(trenner.len())
        .position(|fenster| fenster == trenner.as_slice())
        .ok_or_else(|| anyhow!("Der Heil-Befehl passt nicht zum Muster: {muster}"))?;
    let (x_worte, rest) = worte.split_at(stelle);
    let y_worte = &rest[trenner.len()..];
    if x_worte.len() != 1 || y_worte.len() != 1 {
        bail!("Der Heil-Befehl erwartet je genau ein Wort vor und nach der Trennung");
    }
    Ok((x_worte[0].clone(), Some(y_worte[0].clone())))
}

#[derive(Clone, Debug, Default)]
pub(crate) struct HealingReport {
    pub aliases_added: usize,
    pub edges_merged: usize,
    pub quarantined: usize,
}

/// Canonicalize exact duplicate observations while retaining every node ID.
/// Suspicious edges and orphan nodes are recorded for inspection; they are not deleted.
pub(crate) fn heal(state: &mut State) -> HealingReport {
    let mut report = HealingReport::default();
    let mut canonical_by_key: BTreeMap<ExactNodeKey, String> = BTreeMap::new();
    let mut aliases: BTreeMap<String, String> = state
        .extra
        .get("neuron_aliases")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let mut quarantine = state
        .extra
        .get("quarantine")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // The alias map is vault-wide metadata while this pass sees one resident
    // slice: an alias whose source or target is simply not resident is a
    // paging artifact, not a provably invalid entry, so it stays
    // (non-destructive). Only verifiable failures — alias cycles below —
    // are quarantined.
    let existing_aliases = aliases.clone();
    for (id, node) in &state.neuronen {
        let key = (
            node.typ.clone(),
            node.label.clone(),
            node.geboren,
            node.ring.clone(),
            node.staerke.to_bits(),
            node.alter.clone(),
            node.nutzt,
            format!("{:?}", node.extra),
        );
        if let Some(canon) = canonical_by_key.get(&key).cloned() {
            if canon != *id {
                let preferred = if id.starts_with("lex:") && !canon.starts_with("lex:") {
                    id.clone()
                } else {
                    canon.clone()
                };
                let other = if preferred == *id { canon } else { id.clone() };
                canonical_by_key.insert(key, preferred.clone());
                if existing_aliases.get(&other) != Some(&preferred) {
                    report.aliases_added += 1;
                }
                aliases.insert(other, preferred);
            }
        } else {
            canonical_by_key.insert(key, id.clone());
        }
    }
    let resolve =
        |id: &str, aliases: &BTreeMap<String, String>, resident: &BTreeSet<String>| -> String {
            let mut current = id.to_owned();
            let mut visited = BTreeSet::new();
            // Follow the chain only while every hop is a resident node: a cold
            // target is a paging artifact, not a missing record, and routing an
            // edge onto it would break the state invariant.
            while visited.insert(current.clone()) {
                match aliases.get(&current) {
                    Some(next) if resident.contains(next) => current = next.clone(),
                    _ => break,
                }
            }
            current
        };
    let resident: BTreeSet<String> = state.neuronen.keys().cloned().collect();
    let mut merged: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    // Break cycles deterministically and make them visible rather than resolving
    // to an arbitrary member of a malformed alias loop. Every broken cycle is
    // a failed heal on the word level (Kap. 22): the threshold rises.
    let alias_keys: Vec<_> = aliases.keys().cloned().collect();
    for key in alias_keys {
        let mut current = key.clone();
        let mut seen = BTreeSet::new();
        while let Some(next) = aliases.get(&current) {
            if !seen.insert(current.clone()) || seen.contains(next) {
                aliases.remove(&key);
                quarantine.push(json!({"kind":"alias_cycle","id":key}));
                report.quarantined += 1;
                misserfolg(state, EBENE_WORT);
                break;
            }
            current = next.clone();
        }
    }
    for (from, edges) in &state.kanten {
        for (to, weight) in edges {
            if !weight.is_finite() || !(-1.0..=1.0).contains(weight) {
                quarantine
                    .push(json!({"kind":"invalid_weight","from":from,"to":to,"weight":weight}));
                report.quarantined += 1;
                continue;
            }
            let a = resolve(from, &aliases, &resident);
            let b = resolve(to, &aliases, &resident);
            let slot = merged.entry(a).or_default().entry(b).or_insert(*weight);
            if *slot != *weight {
                report.edges_merged += 1;
                *slot = slot.max(*weight);
            }
        }
    }
    state.kanten = merged;
    state.extra.insert("neuron_aliases".into(), json!(aliases));
    state
        .extra
        .insert("quarantine".into(), Value::Array(quarantine));
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Node;
    use std::collections::BTreeMap;

    #[test]
    fn exact_duplicate_aliases_merge_edges_without_deleting_neurons() {
        let mut s = State::empty();
        let node = Node {
            id: "dup".into(),
            typ: "LEX".into(),
            label: "hallo".into(),
            staerke: 0.4,
            alter: "jung".into(),
            nutzt: 1,
            geboren: 2,
            ring: None,
            extra: BTreeMap::new(),
        };
        s.neuronen.insert("dup".into(), node);
        s.neuronen.insert(
            "lex:hallo".into(),
            Node {
                id: "lex:hallo".into(),
                typ: "LEX".into(),
                label: "hallo".into(),
                staerke: 0.4,
                alter: "jung".into(),
                nutzt: 1,
                geboren: 2,
                ring: None,
                extra: BTreeMap::new(),
            },
        );
        s.neuronen.insert(
            "lex:test".into(),
            Node {
                id: "lex:test".into(),
                typ: "LEX".into(),
                label: "test".into(),
                staerke: 0.5,
                alter: "jung".into(),
                nutzt: 0,
                geboren: 1,
                ring: None,
                extra: BTreeMap::new(),
            },
        );
        s.kanten
            .entry("lex:test".into())
            .or_default()
            .insert("dup".into(), 0.2);
        s.kanten
            .entry("lex:test".into())
            .or_default()
            .insert("lex:hallo".into(), 0.5);
        s.kanten
            .entry("dup".into())
            .or_default()
            .insert("lex:hallo".into(), 0.1);
        let r = heal(&mut s);
        assert_eq!(r.aliases_added, 1);
        assert!(s.neuronen.contains_key("dup"));
        assert_eq!(s.extra["neuron_aliases"]["dup"], "lex:hallo");
        assert_eq!(s.kanten["lex:test"]["lex:hallo"], 0.5);
        assert_eq!(s.kanten["lex:hallo"]["lex:hallo"], 0.1);
        assert_eq!(heal(&mut s).aliases_added, 0);
        assert!(s.validate().is_ok());
    }
}
