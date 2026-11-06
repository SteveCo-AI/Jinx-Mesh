//! Chat and training moves: frozen read-only inference, dialog learning,
//! continuation of budget-limited traversals, consolidation rounds, and the
//! authenticated file import — each journaled write-ahead and gate-checked.
use super::{
    activation::{activate_graph, checkpoint_fits, source_matches},
    changed_graph, metadata_after, App, Session, ACTIVATION_BUDGET,
};
use crate::{
    engine::Engine,
    graph,
    journal::{self, JournalEintrag, JournalVermerk},
    model::{ActivationStatus, GraphSlice, Reply},
    store::{Store, TraversalCheckpoint},
};
use anyhow::{bail, Result};
use serde_json::json;
use std::{
    path::Path,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

impl App {
    /// Evaluate learned connections without training, archive, or checkpoint writes.
    pub fn infer_read_only(&mut self, text: &str) -> Result<Reply> {
        self.infer_read_only_with_progress(text, |_| {})
    }

    fn infer_read_only_with_progress<F>(&mut self, text: &str, mut progress: F) -> Result<Reply>
    where
        F: FnMut(&ActivationStatus),
    {
        if graph::tokens(text).len() > ACTIVATION_BUDGET || text.len() > 16384 {
            bail!("Nachricht überschreitet das Inferenzbudget");
        }
        self.tick();
        let s = self
            .session
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Gesperrt: zuerst /login"))?;
        let key = s.key.as_ref().or(s.read_key.as_ref()).expect("session key");
        let (records, mut activation, _, _, _) = activate_graph(
            &self.store,
            key,
            &s.payload.state,
            text,
            None,
            None,
            &mut progress,
            4,
        )?;
        // Read-only inference uses no persisted checkpoint, audit, or archive writes.
        activation.can_continue = false;
        s.graph_cache = records;
        s.engine = Engine::new(
            Engine::active_state(&s.payload.state, &s.graph_cache),
            s.payload.actions.clone(),
        )?;
        let mut r = s.engine.infer(text)?;
        s.prefetch_status = Some(activation.clone());
        r.activation = Some(activation);
        s.feedback_target = if r.text.is_empty() {
            None
        } else {
            Some(crate::language::target(text, &r.text))
        };
        s.last = Instant::now();
        Ok(r)
    }

    pub fn message(&mut self, text: &str) -> Result<Reply> {
        self.message_with_progress(text, |_| {})
    }

    pub fn message_with_progress<F>(&mut self, text: &str, mut progress: F) -> Result<Reply>
    where
        F: FnMut(&ActivationStatus),
    {
        let raw_text = text;
        let (feedback, text) = crate::language::split_feedback(text);
        if raw_text.len() > 16384 {
            bail!("Nachricht zu lang (16384 UTF-8-Bytes)");
        }
        if graph::tokens(text).len() > ACTIVATION_BUDGET {
            bail!("Nachricht überschreitet das Aktivierungsbudget von 128 Wörtern");
        }
        self.tick();
        let s = self
            .session
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Gesperrt: zuerst /login"))?;
        if s.frozen {
            if feedback.is_some() {
                bail!("Bewertungen sind nur im DEV-Modus erlaubt");
            }
            return self.infer_read_only_with_progress(text, progress);
        }
        let key = s.key.as_ref().expect("developer key");
        let old_metadata = s.payload.state.clone();
        let prior_graph = s.graph_cache.clone();
        s.prefetch_status = None;
        // A new user turn starts new work; identical text does not silently
        // resume an earlier traversal.
        let _ = self.store.clear_checkpoint(key, "active")?;
        let resume = None;
        let (records, activation, frontier, visited, episodes) = activate_graph(
            &self.store,
            key,
            &s.payload.state,
            text,
            None,
            resume.as_ref(),
            &mut progress,
            4,
        )?;
        s.graph_cache = records;
        s.engine = Engine::new(
            Engine::active_state(&s.payload.state, &s.graph_cache),
            s.payload.actions.clone(),
        )?;
        // Persisted view of the section this move can affect, captured after
        // activation and before any learning mutates it; journal ops diff
        // exactly against this snapshot.
        let journiert_alt = s.graph_cache.clone();
        if let Some(target) = &s.feedback_target {
            s.engine
                .state
                .extra
                .insert("feedback_target".into(), target.clone());
        } else {
            s.engine.state.extra.remove("feedback_target");
        }
        let old = s.engine.state.clone();
        let archive_user_id = s.payload.archive_count.saturating_add(1);
        let result = s
            .engine
            .train_with_source(raw_text, &format!("episode:{archive_user_id}"));
        match result {
            Ok(reply) => {
                // Keep exact utterances in paged encrypted archive blocks, outside the
                // eagerly loaded graph. Timestamps remain metadata.
                let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
                let seq = s.engine.state.interaktionen;
                let archive_reply_id = archive_user_id.saturating_add(1);
                let archived = vec![
                    json!({"id":format!("episode:{archive_user_id}"),"sequence":seq,"unix_time":now,"speaker":"user","text":raw_text}),
                    json!({"id":format!("reply:{archive_reply_id}"),"sequence":seq,"unix_time":now,"speaker":"assistant","text":reply.text}),
                ];
                Engine::merge_active(&mut s.graph_cache, &s.engine.state);
                s.payload.state = metadata_after(&old_metadata, &s.engine.state);
                // Write-ahead learning step (Kap. 14): the journal entry
                // becomes durable before the state change commits, so a crash
                // in between leaves an honestly discardable unconfirmed entry.
                // Kap. 18: an explicit +/- rating is the move's feedback
                // signal and shares the art "feedback".
                let art = if feedback.is_some() {
                    journal::ART_FEEDBACK
                } else {
                    journal::ART_LEARN
                };
                let journal_seq = s
                    .payload
                    .op_journal_seq
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("Journal voll"))?;
                let eintrag = journal::eintrag(
                    journal_seq,
                    s.engine.state.interaktionen,
                    art,
                    now,
                    journal::Ausschnitt {
                        cache: &journiert_alt,
                        meta: &old_metadata,
                    },
                    journal::Ausschnitt {
                        cache: &s.graph_cache,
                        meta: &s.payload.state,
                    },
                )?;
                // Kap.-24 integration gate: the proposed learning diff is
                // checked against the konfig rule data before anything
                // persists. A reject restores the pre-move state exactly;
                // only the rejection marker (art "integration_reject")
                // commits — the move itself never does.
                let gruende = crate::gate::verify(
                    &old_metadata,
                    &eintrag.ops,
                    &crate::gate::regeln(&s.engine.state),
                );
                if !gruende.is_empty() {
                    let nummer = old.interaktionen.saturating_add(1);
                    s.engine.state = old;
                    s.graph_cache = prior_graph;
                    s.payload.state = old_metadata;
                    if let Err(e) = gate_reject_vermerken(&mut self.store, s, nummer) {
                        self.session = None;
                        return Err(e);
                    }
                    bail!("{}", crate::gate::hinweis(&gruende));
                }
                let mut neuer_eintrag = None;
                let mut vermerk = JournalVermerk::default();
                if !eintrag.ops.is_empty() {
                    let kap = journal::kap(&s.engine.state);
                    if let Err(e) = self.store.journal_append(key, &eintrag, kap) {
                        self.session = None;
                        return Err(e);
                    }
                    vermerk.bestaetigt = Some(journal_seq);
                    neuer_eintrag = Some(eintrag);
                }
                let graph_updates = changed_graph(&prior_graph, &s.graph_cache);
                if let Err(e) = self.store.commit(
                    &mut s.payload,
                    &old_metadata,
                    &graph_updates,
                    s.key.as_ref().expect("developer key"),
                    "dialog",
                    &archived,
                    &vermerk,
                ) {
                    // Fail closed after a storage error, never continue with uncommitted learning.
                    self.session = None;
                    return Err(e);
                }
                if let Some(eintrag) = neuer_eintrag {
                    let kap = journal::kap(&s.engine.state);
                    s.journal.push(eintrag);
                    while s.journal.len() > kap {
                        s.journal.remove(0);
                    }
                }
                s.feedback_target = s.engine.state.extra.get("feedback_target").cloned();
                let mut reply = reply;
                let mut activation = activation;
                activation.source_matches = source_matches(text, &episodes);
                reply.activation = Some(activation);
                if !frontier.is_empty() {
                    let checkpoint = TraversalCheckpoint {
                        revision: s.payload.sequence,
                        frontier,
                        visited,
                        provisional: reply.text.clone(),
                        query: text.to_owned(),
                    };
                    if reply
                        .activation
                        .as_ref()
                        .is_some_and(|status| status.can_continue)
                        && checkpoint_fits(&checkpoint)
                    {
                        let _ = self.store.save_checkpoint(
                            s.key.as_ref().expect("developer key"),
                            "active",
                            &checkpoint,
                        )?;
                    } else if let Some(status) = &mut reply.activation {
                        status.can_continue = false;
                    }
                }
                s.last = Instant::now();
                Ok(reply)
            }
            Err(e) => {
                s.engine.state = old;
                s.graph_cache = prior_graph;
                Err(e)
            }
        }
    }

    /// Continue a revision-matched graph traversal left pending by the activation budget.
    /// It performs inference only; it does not add another training event.
    pub fn continue_message(&mut self) -> Result<Reply> {
        self.continue_message_with_progress(|_| {})
    }

    pub fn continue_message_with_progress<F>(&mut self, mut progress: F) -> Result<Reply>
    where
        F: FnMut(&ActivationStatus),
    {
        self.tick();
        let s = self
            .session
            .as_mut()
            .filter(|s| !s.frozen)
            .ok_or_else(|| anyhow::anyhow!("Developer-Anmeldung erforderlich"))?;
        let key = s.key.as_ref().expect("developer key");
        let checkpoint = self
            .store
            .load_checkpoint(key, "active", s.payload.sequence)?
            .ok_or_else(|| anyhow::anyhow!("Keine passende Graph-Aktivierung zum Fortsetzen"))?;
        let metadata = s.payload.state.clone();
        let prior = s.graph_cache.clone();
        let (records, activation, frontier, visited, episodes) = activate_graph(
            &self.store,
            key,
            &metadata,
            &checkpoint.query,
            None,
            Some(&checkpoint),
            &mut progress,
            4,
        )?;
        s.graph_cache = records;
        s.engine = Engine::new(
            Engine::active_state(&metadata, &s.graph_cache),
            s.payload.actions.clone(),
        )?;
        let mut reply = s.engine.infer(&checkpoint.query)?;
        s.feedback_target = if reply.text.is_empty() {
            None
        } else {
            Some(crate::language::target(&checkpoint.query, &reply.text))
        };
        let mut activation = activation;
        activation.source_matches = source_matches(&checkpoint.query, &episodes);
        reply.activation = Some(activation);
        if frontier.is_empty() {
            let _ = self.store.clear_checkpoint(key, "active")?;
        } else {
            let next = TraversalCheckpoint {
                revision: s.payload.sequence,
                frontier,
                visited,
                provisional: reply.text.clone(),
                query: checkpoint.query,
            };
            if reply
                .activation
                .as_ref()
                .is_some_and(|status| status.can_continue)
                && checkpoint_fits(&next)
            {
                let _ = self.store.save_checkpoint(key, "active", &next)?;
            } else if let Some(status) = &mut reply.activation {
                status.can_continue = false;
            }
        }
        s.last = Instant::now();
        let _ = prior;
        Ok(reply)
    }

    pub fn consolidate(&mut self) -> Result<serde_json::Value> {
        self.tick();
        let s = self
            .session
            .as_mut()
            .filter(|s| !s.frozen)
            .ok_or_else(|| anyhow::anyhow!("Developer-Anmeldung erforderlich"))?;
        let old_metadata = s.payload.state.clone();
        let before = s.graph_cache.clone();
        // Pre-round engine state for an exact restore when the Kap.-24 gate
        // refuses the consolidation round.
        let alt_aktiv = s.engine.state.clone();
        let healing = crate::healing::heal(&mut s.engine.state);
        graph::consolidate(&mut s.engine.state);
        Engine::merge_active(&mut s.graph_cache, &s.engine.state);
        s.payload.state = metadata_after(&old_metadata, &s.engine.state);
        let graph_updates = changed_graph(&before, &s.graph_cache);
        // Every consolidation round leaves a journal entry — even without a
        // learning change it marks the round boundary for /undo round.
        let zeit = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let journal_seq = s
            .payload
            .op_journal_seq
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Journal voll"))?;
        let eintrag = journal::eintrag(
            journal_seq,
            s.engine.state.interaktionen,
            journal::ART_CONSOLIDATE,
            zeit,
            journal::Ausschnitt {
                cache: &before,
                meta: &old_metadata,
            },
            journal::Ausschnitt {
                cache: &s.graph_cache,
                meta: &s.payload.state,
            },
        )?;
        // Kap.-24 integration gate: the round commits only when its diff
        // satisfies the rule data; a reject restores the pre-round state
        // exactly and leaves the rejection marker instead of a boundary.
        let gruende = crate::gate::verify(
            &old_metadata,
            &eintrag.ops,
            &crate::gate::regeln(&s.engine.state),
        );
        if !gruende.is_empty() {
            let nummer = alt_aktiv.interaktionen;
            s.engine.state = alt_aktiv;
            s.graph_cache = before;
            s.payload.state = old_metadata;
            if let Err(e) = gate_reject_vermerken(&mut self.store, s, nummer) {
                self.session = None;
                return Err(e);
            }
            bail!("{}", crate::gate::hinweis(&gruende));
        }
        let kap = journal::kap(&s.engine.state);
        let key = s.key.as_ref().expect("developer key");
        if let Err(e) = self.store.journal_append(key, &eintrag, kap) {
            self.session = None;
            return Err(e);
        }
        let vermerk = JournalVermerk {
            bestaetigt: Some(journal_seq),
            ..JournalVermerk::default()
        };
        if let Err(e) = self.store.commit(
            &mut s.payload,
            &old_metadata,
            &graph_updates,
            key,
            "consolidation",
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
        Ok(
            json!({"scope":"active_neighborhood","active_nodes":before.nodes.len(),"aliases_added":healing.aliases_added,"edges_merged":healing.edges_merged,"quarantined":healing.quarantined}),
        )
    }

    /// Authenticated owner-only data import; no execution and no dialog feedback.
    pub fn train_file(&mut self, path: &Path, code: bool) -> Result<serde_json::Value> {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        self.tick();
        let s = self
            .session
            .as_mut()
            .filter(|s| !s.frozen)
            .ok_or_else(|| anyhow::anyhow!("Developer-Anmeldung erforderlich"))?;
        let mut raw = zeroize::Zeroizing::new(Vec::new());
        std::fs::File::open(path)?
            .take(262145)
            .read_to_end(&mut raw)?;
        if raw.len() > 262144 {
            bail!("Datei zu groß (256 KiB)");
        }
        let text = std::str::from_utf8(&raw)?;
        let hash = format!("{:x}", Sha256::digest(&raw));
        let source_key = format!("{}:{hash}", if code { "code" } else { "text" });
        if s.engine
            .state
            .extra
            .get("rust_sources")
            .and_then(|v| v.get(&source_key))
            .is_some()
        {
            return Ok(json!({"already_learned":true,"sha256":hash}));
        }
        let ts = if code {
            crate::graph::code_tokens(text)
        } else {
            graph::tokens(text)
        };
        if ts.iter().any(|t| t.len() > 1024) {
            bail!("Ein Token überschreitet 1024 Bytes");
        }
        if ts.len() > ACTIVATION_BUDGET {
            bail!(
                "Datei überschreitet das Importbudget von 128 Wörtern; bitte in Abschnitte teilen"
            );
        }
        let old_metadata = s.payload.state.clone();
        let before = s.graph_cache.clone();
        let (records, _, _, _, _) = activate_graph(
            &self.store,
            s.key.as_ref().expect("developer key"),
            &s.payload.state,
            text,
            Some(&ts),
            None,
            &mut |_| {},
            4,
        )?;
        s.graph_cache = records;
        s.engine = Engine::new(
            Engine::active_state(&s.payload.state, &s.graph_cache),
            s.payload.actions.clone(),
        )?;
        let journiert_alt = s.graph_cache.clone();
        let old = s.engine.state.clone();
        let file_growth_budget = ACTIVATION_BUDGET.saturating_sub(s.engine.state.neuronen.len());
        let added = graph::grow(&mut s.engine.state, &ts, file_growth_budget);
        graph::hear(&mut s.engine.state, &ts);
        if !code {
            crate::language::observe(&mut s.engine.state, &crate::language::parse(text));
        }
        s.engine
            .state
            .extra
            .entry("rust_sources".into())
            .or_insert_with(|| json!({}))[&source_key] = json!({"tokens":ts.len(),"code":code});
        if let Err(e) = s.engine.state.validate() {
            s.engine.state = old;
            return Err(e);
        }
        Engine::merge_active(&mut s.graph_cache, &s.engine.state);
        s.payload.state = metadata_after(&old_metadata, &s.engine.state);
        // The import is one learning move; its journal entry precedes the
        // commit exactly like a dialog learning move.
        let zeit = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let journal_seq = s
            .payload
            .op_journal_seq
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Journal voll"))?;
        let eintrag = journal::eintrag(
            journal_seq,
            s.engine.state.interaktionen,
            journal::ART_LEARN,
            zeit,
            journal::Ausschnitt {
                cache: &journiert_alt,
                meta: &old_metadata,
            },
            journal::Ausschnitt {
                cache: &s.graph_cache,
                meta: &s.payload.state,
            },
        )?;
        // Kap.-24 integration gate: the import commits only when its diff
        // satisfies the rule data; a reject restores the pre-import state
        // exactly, so the source digest is not consumed either.
        let gruende = crate::gate::verify(
            &old_metadata,
            &eintrag.ops,
            &crate::gate::regeln(&s.engine.state),
        );
        if !gruende.is_empty() {
            let nummer = old.interaktionen;
            s.engine.state = old;
            s.graph_cache = before;
            s.payload.state = old_metadata;
            if let Err(e) = gate_reject_vermerken(&mut self.store, s, nummer) {
                self.session = None;
                return Err(e);
            }
            bail!("{}", crate::gate::hinweis(&gruende));
        }
        let mut neuer_eintrag = None;
        let mut vermerk = JournalVermerk::default();
        if !eintrag.ops.is_empty() {
            let kap = journal::kap(&s.engine.state);
            if let Err(e) =
                self.store
                    .journal_append(s.key.as_ref().expect("developer key"), &eintrag, kap)
            {
                self.session = None;
                return Err(e);
            }
            vermerk.bestaetigt = Some(journal_seq);
            neuer_eintrag = Some(eintrag);
        }
        let graph_updates = changed_graph(&before, &s.graph_cache);
        if let Err(e) = self.store.commit(
            &mut s.payload,
            &old_metadata,
            &graph_updates,
            s.key.as_ref().expect("developer key"),
            "file_import",
            &[],
            &vermerk,
        ) {
            self.session = None;
            return Err(e);
        }
        if let Some(eintrag) = neuer_eintrag {
            let kap = journal::kap(&s.engine.state);
            s.journal.push(eintrag);
            while s.journal.len() > kap {
                s.journal.remove(0);
            }
        }
        s.last = Instant::now();
        Ok(json!({"sha256":hash,"tokens":ts.len(),"new_neurons":added}))
    }
}

/// Kap.-24 gate rejection bookkeeping: the refused move committed nothing,
/// so the marker entry carries no ops. It is appended write-ahead and then
/// confirmed by a bookkeeping commit of the UNCHANGED state — an
/// unconfirmed marker would be discarded as a crash remnant at the next
/// login, silently losing the honest rejection record. Like every journal
/// entry it survives a reload, shows up in /status, and rolls back with
/// /undo.
fn gate_reject_vermerken(store: &mut Store, s: &mut Session, nummer: u64) -> Result<()> {
    let zeit = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let journal_seq = s
        .payload
        .op_journal_seq
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("Journal voll"))?;
    let eintrag = JournalEintrag {
        seq: journal_seq,
        interaktionsnummer: nummer,
        art: journal::ART_INTEGRATION_REJECT.into(),
        zeit,
        ops: Vec::new(),
    };
    let kap = journal::kap(&s.payload.state);
    store.journal_append(s.key.as_ref().expect("developer key"), &eintrag, kap)?;
    let alt = s.payload.state.clone();
    let vermerk = JournalVermerk {
        bestaetigt: Some(journal_seq),
        ..JournalVermerk::default()
    };
    store.commit(
        &mut s.payload,
        &alt,
        &GraphSlice::default(),
        s.key.as_ref().expect("developer key"),
        "integration_reject",
        &[],
        &vermerk,
    )?;
    s.journal.push(eintrag);
    while s.journal.len() > kap {
        s.journal.remove(0);
    }
    Ok(())
}
