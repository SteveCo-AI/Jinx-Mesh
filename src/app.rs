use crate::{
    crypto::Key,
    engine::Engine,
    graph,
    model::{ActivationStatus, GraphSlice, Reply, SourceMatch, State},
    store::{Import, Payload, Store, TraversalCheckpoint},
};
use anyhow::{bail, Result};
use serde_json::json;
use std::{
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use totp_rs::{Algorithm, Secret, TOTP};
use zeroize::Zeroize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Locked,
    Developer,
    Frozen,
}
#[derive(Clone, Copy)]
pub struct SessionPolicy {
    pub idle: Duration,
    pub maximum: Duration,
}
impl Default for SessionPolicy {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(1800),
            maximum: Duration::from_secs(8 * 3600),
        }
    }
}
struct Session {
    engine: Engine,
    graph_cache: GraphSlice,
    prefetch_status: Option<ActivationStatus>,
    key: Option<Key>,
    payload: Payload,
    started: Instant,
    last: Instant,
    frozen: bool,
}
impl Drop for Session {
    fn drop(&mut self) {
        if let Some(s) = &mut self.payload.totp {
            s.zeroize();
        }
    }
}
pub struct App {
    store: Store,
    session: Option<Session>,
    policy: SessionPolicy,
}

/// Bounded archive retrieval result. `complete` is false when the configured
/// page budget ended before the full archive was scanned.
#[derive(Clone, Debug)]
pub struct RecallResult {
    pub hits: Vec<serde_json::Value>,
    pub episodes_scanned: usize,
    pub total_episodes: u64,
    pub complete: bool,
    pub next_offset: Option<u64>,
}
fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}
fn totp(secret: &str) -> Result<TOTP> {
    Ok(TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        Secret::Encoded(secret.into())
            .to_bytes()
            .map_err(|_| anyhow::anyhow!("2FA-Konfiguration"))?,
        Some("Jinx".into()),
        "developer".into(),
    )?)
}
impl App {
    pub fn create(
        path: &Path,
        user: &str,
        password: &str,
        totp_secret: Option<String>,
        confirmation: Option<&str>,
        import: Import,
    ) -> Result<Self> {
        if let Some(s) = &totp_secret {
            if !totp(s)?.check_current(confirmation.unwrap_or(""))? {
                bail!("2FA-Einrichtung nicht bestätigt");
            }
        }
        Ok(Self {
            store: Store::create(path, user, password, totp_secret, Some(import))?,
            session: None,
            policy: SessionPolicy::default(),
        })
    }
    /// Fresh vault with zero vocabulary, zero concepts and no response templates.
    pub fn create_empty(
        path: &Path,
        user: &str,
        password: &str,
        totp_secret: Option<String>,
        confirmation: Option<&str>,
    ) -> Result<Self> {
        if let Some(secret) = &totp_secret {
            if !totp(secret)?.check_current(confirmation.unwrap_or(""))? {
                bail!("2FA-Einrichtung nicht bestätigt");
            }
        }
        Ok(Self {
            store: Store::create(path, user, password, totp_secret, None)?,
            session: None,
            policy: SessionPolicy::default(),
        })
    }
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            store: Store::open(path)?,
            session: None,
            policy: SessionPolicy::default(),
        })
    }
    pub fn with_policy(mut self, policy: SessionPolicy) -> Self {
        self.policy = policy;
        self
    }
    pub fn new_totp_secret() -> String {
        Secret::generate_secret().to_encoded().to_string()
    }
    pub fn tick(&mut self) {
        if self.session.as_ref().is_some_and(|s| {
            s.last.elapsed() >= self.policy.idle || s.started.elapsed() >= self.policy.maximum
        }) {
            self.session = None;
        }
    }
    pub fn mode(&mut self) -> Mode {
        self.tick();
        self.session
            .as_ref()
            .map(|s| {
                if s.frozen {
                    Mode::Frozen
                } else {
                    Mode::Developer
                }
            })
            .unwrap_or(Mode::Locked)
    }
    pub fn login(&mut self, user: &str, password: &str, code: Option<&str>) -> Result<()> {
        // A failed reauthentication cannot leave an old developer session active.
        self.logout();
        let time = now()?;
        self.store.check_throttle(time)?;
        let attempt = (|| -> Result<(Payload, Key)> {
            let (mut payload, key) = self.store.unlock(user, password)?;
            if let Some(secret) = &payload.totp {
                let otp = totp(secret)?;
                let token = code.unwrap_or("");
                if token.len() != 6 || !token.bytes().all(|b| b.is_ascii_digit()) {
                    bail!("Anmeldung fehlgeschlagen");
                }
                // Check skew explicitly and persist the consumed counter before granting access.
                let step = time / 30;
                let mut strict = otp;
                strict.skew = 0;
                let matched = [step.saturating_sub(1), step, step + 1]
                    .into_iter()
                    .find(|s| {
                        payload.last_totp.is_none_or(|last| *s > last)
                            && strict.check(token, *s * 30)
                    });
                payload.last_totp =
                    Some(matched.ok_or_else(|| anyhow::anyhow!("Anmeldung fehlgeschlagen"))?);
            }
            Ok((payload, key))
        })();
        let (payload, key) = match attempt {
            Ok(ok) => ok,
            Err(_) => {
                self.store.failure(now()?)?;
                bail!("Anmeldung fehlgeschlagen");
            }
        };
        self.store.authenticated(&payload, &key)?;
        let graph_cache = if payload.state.neuronen.is_empty() {
            GraphSlice::default()
        } else {
            GraphSlice {
                nodes: payload.state.neuronen.clone(),
                edges: payload.state.kanten.clone(),
            }
        };
        let active = Engine::active_state(&payload.state, &graph_cache);
        let engine = Engine::new(active, payload.actions.clone())?;
        self.session = Some(Session {
            engine,
            graph_cache,
            prefetch_status: None,
            key: Some(key),
            payload,
            started: Instant::now(),
            last: Instant::now(),
            frozen: false,
        });
        Ok(())
    }
    pub fn logout(&mut self) {
        self.session = None;
    }
    pub fn freeze(&mut self) -> Result<()> {
        self.tick();
        let s = self
            .session
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Anmeldung erforderlich"))?;
        s.frozen = true;
        s.key = None;
        if let Some(secret) = &mut s.payload.totp {
            secret.zeroize();
        }
        s.payload.totp = None;
        s.last = Instant::now();
        Ok(())
    }
    pub fn message(&mut self, text: &str) -> Result<Reply> {
        self.message_with_progress(text, |_| {})
    }

    pub fn message_with_progress<F>(&mut self, text: &str, mut progress: F) -> Result<Reply>
    where
        F: FnMut(&ActivationStatus),
    {
        if graph::tokens(text).len() > ACTIVATION_BUDGET {
            bail!("Nachricht überschreitet das Aktivierungsbudget von 128 Wörtern");
        }
        self.tick();
        let s = self
            .session
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Gesperrt: zuerst /login"))?;
        if s.frozen {
            let r = s.engine.infer(text)?;
            s.last = Instant::now();
            return Ok(r);
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
        let old = s.engine.state.clone();
        let archive_user_id = s.payload.archive_count.saturating_add(1);
        let result = s
            .engine
            .train_with_source(text, &format!("episode:{archive_user_id}"));
        match result {
            Ok(reply) => {
                // Keep exact utterances in paged encrypted archive blocks, outside the
                // eagerly loaded graph. Timestamps remain metadata.
                let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
                let seq = s.engine.state.interaktionen;
                let archive_reply_id = archive_user_id.saturating_add(1);
                let archived = vec![
                    json!({"id":format!("episode:{archive_user_id}"),"sequence":seq,"unix_time":now,"speaker":"user","text":text}),
                    json!({"id":format!("reply:{archive_reply_id}"),"sequence":seq,"unix_time":now,"speaker":"assistant","text":reply.text}),
                ];
                Engine::merge_active(&mut s.graph_cache, &s.engine.state);
                s.payload.state = metadata_after(&old_metadata, &s.engine.state);
                let graph_updates = changed_graph(&prior_graph, &s.graph_cache);
                if let Err(e) = self.store.commit(
                    &mut s.payload,
                    &old_metadata,
                    &graph_updates,
                    s.key.as_ref().expect("developer key"),
                    "dialog",
                    &archived,
                ) {
                    // Fail closed after a storage error, never continue with uncommitted learning.
                    self.session = None;
                    return Err(e);
                }
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

    /// Run one bounded, read-only continuation step while the terminal waits
    /// for user input. The periodic caller must use this only during the chat
    /// prompt, never while prompting for credentials.
    pub fn prefetch_idle_step(&mut self) -> Result<Option<ActivationStatus>> {
        self.tick();
        let Some(s) = self.session.as_mut().filter(|session| !session.frozen) else {
            return Ok(None);
        };
        let key = s.key.as_ref().expect("developer key");
        let Some(checkpoint) = self
            .store
            .load_checkpoint(key, "active", s.payload.sequence)?
        else {
            return Ok(None);
        };
        let metadata = s.payload.state.clone();
        let (records, activation, frontier, visited, _) = activate_graph(
            &self.store,
            key,
            &metadata,
            &checkpoint.query,
            None,
            Some(&checkpoint),
            &mut |_| {},
            1,
        )?;
        s.graph_cache = records;
        s.engine = Engine::new(
            Engine::active_state(&metadata, &s.graph_cache),
            s.payload.actions.clone(),
        )?;
        if frontier.is_empty() {
            let _ = self.store.clear_checkpoint(key, "active")?;
        } else {
            let next = TraversalCheckpoint {
                revision: s.payload.sequence,
                frontier,
                visited,
                provisional: checkpoint.provisional,
                query: checkpoint.query,
            };
            if activation.can_continue && checkpoint_fits(&next) {
                let _ = self.store.save_checkpoint(key, "active", &next)?;
            }
        }
        s.prefetch_status = Some(activation.clone());
        Ok(Some(activation))
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
        let healing = crate::healing::heal(&mut s.engine.state);
        graph::consolidate(&mut s.engine.state);
        Engine::merge_active(&mut s.graph_cache, &s.engine.state);
        s.payload.state = metadata_after(&old_metadata, &s.engine.state);
        let graph_updates = changed_graph(&before, &s.graph_cache);
        if let Err(e) = self.store.commit(
            &mut s.payload,
            &old_metadata,
            &graph_updates,
            s.key.as_ref().expect("developer key"),
            "consolidation",
            &[],
        ) {
            self.session = None;
            return Err(e);
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
        let old = s.engine.state.clone();
        let file_growth_budget = ACTIVATION_BUDGET.saturating_sub(s.engine.state.neuronen.len());
        let added = graph::grow(&mut s.engine.state, &ts, file_growth_budget);
        graph::hear(&mut s.engine.state, &ts);
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
        let graph_updates = changed_graph(&before, &s.graph_cache);
        if let Err(e) = self.store.commit(
            &mut s.payload,
            &old_metadata,
            &graph_updates,
            s.key.as_ref().expect("developer key"),
            "file_import",
            &[],
        ) {
            self.session = None;
            return Err(e);
        }
        s.last = Instant::now();
        Ok(json!({"sha256":hash,"tokens":ts.len(),"new_neurons":added}))
    }
    /// Bounded, read-only view of the currently resident network. No archive text.
    pub fn graph_view(&self) -> serde_json::Value {
        let Some(s) = self.session.as_ref() else {
            return json!({"locked": true, "nodes": [], "edges": []});
        };
        let nodes: Vec<_> = s
            .engine
            .state
            .neuronen
            .values()
            .take(128)
            .map(|n| json!({"id": n.id, "label": n.label, "strength": n.staerke}))
            .collect();
        let ids: std::collections::BTreeSet<_> =
            nodes.iter().filter_map(|n| n["id"].as_str()).collect();
        let edges: Vec<_> = s
            .engine
            .state
            .kanten
            .iter()
            .flat_map(|(a, targets)| targets.iter().map(move |(b, w)| (a, b, w)))
            .filter(|(a, b, _)| ids.contains(a.as_str()) && ids.contains(b.as_str()))
            .take(512)
            .map(|(a, b, w)| json!({"from": a, "to": b, "weight": w}))
            .collect();
        json!({"locked": false, "nodes": nodes, "edges": edges,
            "total": s.payload.graph_count, "status": s.prefetch_status})
    }

    pub fn status(&mut self) -> Result<serde_json::Value> {
        self.tick();
        let s = self
            .session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Gesperrt"))?;
        let retrieval = crate::retrieval::RetrievalController::defaults()?;
        Ok(
            json!({"mode":if s.frozen{"frozen"}else{"developer"},"neurons":s.payload.graph_count,"active_nodes":s.graph_cache.nodes.len(),"active_edges":s.graph_cache.edges.values().map(|e|e.len()).sum::<usize>(),"interactions":s.engine.state.interaktionen,"prefetch":s.prefetch_status,"audit_entries":self.store.audit_len()?,"retrieval_handlers":retrieval.ordered_ids()}),
        )
    }

    /// Stream a complete integrity check of every archived conversation block.
    /// Normal login checks only the bounded manifest and indexed reads verify
    /// their own blocks; this command intentionally pays the full I/O cost.
    pub fn verify_archive_full(&mut self) -> Result<u64> {
        self.tick();
        let s = self
            .session
            .as_ref()
            .filter(|s| !s.frozen)
            .ok_or_else(|| anyhow::anyhow!("Developer-Anmeldung erforderlich"))?;
        self.store.verify_archive_full(
            s.key.as_ref().expect("developer key"),
            s.payload.archive_count,
            &s.payload.archive_root,
        )?;
        Ok(s.payload.archive_count)
    }

    /// Stream an authenticated integrity check of every encrypted graph record.
    pub fn verify_graph_full(&mut self) -> Result<u64> {
        self.tick();
        let s = self
            .session
            .as_ref()
            .filter(|s| !s.frozen)
            .ok_or_else(|| anyhow::anyhow!("Developer-Anmeldung erforderlich"))?;
        self.store.verify_graph_full(
            s.key.as_ref().expect("developer key"),
            s.payload.graph_count,
            &s.payload.graph_root,
        )?;
        Ok(s.payload.graph_count)
    }

    /// Exact local recall from authenticated, encrypted conversation episodes.
    /// This is a bounded keyword retrieval primitive, not semantic understanding.
    pub fn recall(&mut self, query: &str, limit: usize) -> Result<Vec<serde_json::Value>> {
        self.recall_with_strategy(query, limit, None)
    }

    /// Run explicitly selected archive handlers. The default combines lexical
    /// and observed two-token context; temporal ordering is opt-in.
    pub fn recall_with_strategy(
        &mut self,
        query: &str,
        limit: usize,
        strategy: Option<&str>,
    ) -> Result<Vec<serde_json::Value>> {
        Ok(self
            .recall_with_strategy_progress(query, limit, strategy, |_, _| {})?
            .hits)
    }

    /// Search a fixed number of encrypted archive pages and report actual page
    /// progress. Context vectors are fetched from bounded learned graph pages.
    pub fn recall_with_strategy_progress<F>(
        &mut self,
        query: &str,
        limit: usize,
        strategy: Option<&str>,
        progress: F,
    ) -> Result<RecallResult>
    where
        F: FnMut(usize, usize),
    {
        self.recall_with_strategy_before_progress(query, limit, strategy, 0, progress)
    }

    /// Search older archive ranges by offset. The offset counts episodes back
    /// from the newest item and is returned as a continuation cursor.
    pub fn recall_with_strategy_before_progress<F>(
        &mut self,
        query: &str,
        limit: usize,
        strategy: Option<&str>,
        before: u64,
        mut progress: F,
    ) -> Result<RecallResult>
    where
        F: FnMut(usize, usize),
    {
        self.tick();
        let s = self
            .session
            .as_ref()
            .filter(|s| !s.frozen)
            .ok_or_else(|| anyhow::anyhow!("Developer-Anmeldung erforderlich"))?;
        if graph::tokens(query).is_empty() {
            return Ok(RecallResult {
                hits: Vec::new(),
                episodes_scanned: 0,
                total_episodes: 0,
                complete: true,
                next_offset: None,
            });
        }
        let key = s
            .key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Anmeldung erforderlich"))?;
        let total = self.store.archive_len()?;
        const ARCHIVE_PAGE: usize = 32;
        const ARCHIVE_PAGE_BUDGET: usize = 128;
        const GRAPH_CONTEXT_BUDGET: usize = 128;
        let controller = crate::retrieval::RetrievalController::defaults()?;
        let selected: Vec<&str> = match strategy {
            Some(id) if controller.has(id) => vec![id],
            Some(id) => bail!("Unbekannte Retrieval-Strategie: {id}"),
            None => vec!["lexical", "contextual"],
        };
        let mut scored: Vec<crate::retrieval::Candidate> = Vec::new();
        let mut end = total.saturating_sub(before);
        let result_budget = limit.clamp(1, 20);
        let total_pages = end.div_ceil(ARCHIVE_PAGE as u64) as usize;
        let mut pages_scanned = 0usize;
        while end > 0 && pages_scanned < ARCHIVE_PAGE_BUDGET {
            let start = end.saturating_sub(ARCHIVE_PAGE as u64);
            let events = self
                .store
                .archive_page(key, start, (end - start) as usize)?;
            let mut context = GraphSlice::default();
            if selected.contains(&"contextual") {
                let query_seeds: Vec<String> = graph::tokens(query)
                    .into_iter()
                    .take(32)
                    .map(|token| format!("lex:{token}"))
                    .collect();
                let query_context =
                    self.store
                        .graph_neighbors(key, &query_seeds, GRAPH_CONTEXT_BUDGET / 2)?;
                merge_slice(&mut context, query_context, GRAPH_CONTEXT_BUDGET);
                let candidate_seeds: Vec<String> = events
                    .iter()
                    .filter_map(|event| event["text"].as_str())
                    .flat_map(graph::tokens)
                    .map(|token| format!("lex:{token}"))
                    .filter(|id| !context.nodes.contains_key(id))
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .take(32)
                    .collect();
                let candidate_context =
                    self.store
                        .graph_neighbors(key, &candidate_seeds, GRAPH_CONTEXT_BUDGET / 2)?;
                merge_slice(&mut context, candidate_context, GRAPH_CONTEXT_BUDGET);
            }
            for id in &selected {
                if !controller.has(id) {
                    bail!("Retrieval-Handler fehlt: {id}");
                }
                scored.extend(controller.get(id)?.search(
                    query,
                    &events,
                    result_budget * 3,
                    &context,
                ));
            }
            if strategy == Some("temporal") {
                scored.sort_by_key(|candidate| {
                    std::cmp::Reverse(candidate.episode["unix_time"].as_u64().unwrap_or(0))
                });
            } else {
                scored.sort_by_key(|candidate| std::cmp::Reverse(candidate.score));
            }
            scored.truncate(result_budget * 3);
            end = start;
            pages_scanned += 1;
            if pages_scanned.is_multiple_of(4) || end == 0 {
                progress(pages_scanned, total_pages);
            }
        }
        if strategy == Some("temporal") {
            scored.sort_by_key(|candidate| {
                std::cmp::Reverse(candidate.episode["unix_time"].as_u64().unwrap_or(0))
            });
        } else {
            scored.sort_by_key(|candidate| std::cmp::Reverse(candidate.score));
        }
        let mut seen = std::collections::BTreeSet::new();
        let hits = scored
            .into_iter()
            .filter(|candidate| {
                seen.insert(candidate.episode["id"].as_str().unwrap_or("").to_owned())
            })
            .take(result_budget)
            .map(|candidate| candidate.episode)
            .collect();
        let complete = end == 0;
        Ok(RecallResult {
            hits,
            episodes_scanned: (pages_scanned * ARCHIVE_PAGE)
                .min(total.saturating_sub(before) as usize),
            total_episodes: total,
            complete,
            next_offset: (!complete)
                .then_some(before.saturating_add((pages_scanned * ARCHIVE_PAGE) as u64)),
        })
    }
}

const ACTIVATION_BUDGET: usize = 128;
const ACTIVATION_PAGE: usize = 32;

fn metadata_after(metadata: &State, active: &State) -> State {
    let mut next = metadata.clone();
    next.interaktionen = active.interaktionen;
    next.letzte_slaap = active.letzte_slaap;
    // Sequence statistics are learned controller data, not lexical graph rows.
    next.extra = active.extra.clone();
    next
}

fn changed_graph(before: &GraphSlice, after: &GraphSlice) -> GraphSlice {
    let dirty: std::collections::BTreeSet<String> = after
        .nodes
        .iter()
        .filter(|(id, node)| before.nodes.get(*id) != Some(*node))
        .map(|(id, _)| id.clone())
        .chain(
            after
                .edges
                .iter()
                .filter(|(id, edges)| before.edges.get(*id) != Some(*edges))
                .map(|(id, _)| id.clone()),
        )
        .collect();
    let mut updates = GraphSlice::default();
    for id in dirty {
        if let Some(node) = after.nodes.get(&id) {
            updates.nodes.insert(id.clone(), node.clone());
            updates.edges.insert(
                id.clone(),
                after.edges.get(&id).cloned().unwrap_or_default(),
            );
        }
    }
    updates
}

fn merge_slice(records: &mut GraphSlice, page: GraphSlice, budget: usize) {
    let mut added = 0usize;
    for (id, node) in page.nodes {
        if records.nodes.contains_key(&id) {
            records.nodes.insert(id.clone(), node);
            if let Some(edges) = page.edges.get(&id) {
                records.edges.insert(id, edges.clone());
            }
        } else if records.nodes.len() < budget && added < budget {
            records.nodes.insert(id.clone(), node);
            if let Some(edges) = page.edges.get(&id) {
                records.edges.insert(id, edges.clone());
            }
            added += 1;
        }
    }
}

// The callback and bounded frontier are kept explicit at this internal I/O boundary.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn activate_graph(
    store: &Store,
    key: &Key,
    _metadata: &State,
    query: &str,
    seed_tokens: Option<&[String]>,
    resume: Option<&TraversalCheckpoint>,
    progress: &mut dyn FnMut(&ActivationStatus),
    page_limit: usize,
) -> Result<(
    GraphSlice,
    ActivationStatus,
    Vec<String>,
    Vec<String>,
    Vec<serde_json::Value>,
)> {
    let mut records = GraphSlice::default();
    let mut pages_loaded = 0usize;
    let mut visited = std::collections::BTreeSet::new();
    let mut frontier = std::collections::VecDeque::new();
    if let Some(checkpoint) = resume {
        visited.extend(checkpoint.visited.iter().cloned());
        frontier.extend(checkpoint.frontier.iter().cloned());
    }

    // Rehydrate only the query seeds and the prior provisional path. Processed
    // history stays as compact IDs in the checkpoint; it never consumes the
    // next invocation's node budget.
    let mut anchors = Vec::new();
    let input_tokens = seed_tokens
        .map(<[String]>::to_vec)
        .unwrap_or_else(|| graph::tokens(query));
    for token in input_tokens.into_iter().take(ACTIVATION_BUDGET) {
        let id = format!("lex:{token}");
        if !anchors.contains(&id) {
            anchors.push(id);
        }
    }
    if let Some(checkpoint) = resume {
        for token in graph::tokens(&checkpoint.provisional).into_iter().take(32) {
            let id = format!("lex:{token}");
            if !anchors.contains(&id) {
                anchors.push(id);
            }
        }
        frontier.extend(anchors.iter().filter(|id| !visited.contains(*id)).cloned());
    } else {
        frontier.extend(anchors.iter().cloned());
    }
    for (chunk_index, chunk) in anchors.chunks(ACTIVATION_PAGE).enumerate() {
        if pages_loaded >= page_limit {
            let pending: Vec<String> = anchors
                .iter()
                .skip((chunk_index * ACTIVATION_PAGE).min(anchors.len()))
                .filter(|id| !visited.contains(*id) && !frontier.contains(*id))
                .cloned()
                .collect();
            frontier.extend(pending);
            break;
        }
        if records.nodes.len() >= ACTIVATION_BUDGET {
            break;
        }
        let page = store.graph_nodes(key, chunk)?;
        pages_loaded += 1;
        merge_slice(&mut records, page, ACTIVATION_BUDGET);
        progress(&ActivationStatus {
            phase: "provisional".into(),
            nodes_loaded: records.nodes.len(),
            node_budget: ACTIVATION_BUDGET,
            pages_loaded,
            source_refs_seen: count_source_refs(&records),
            sources_loaded: 0,
            frontier_remaining: frontier.len(),
            can_continue: true,
            source_matches: Vec::new(),
        });
    }

    // A missing keyed record was checked and is genuinely absent, so it does
    // not need to occupy the continuation frontier. It may be learned as a new
    // node later in this message.
    let missing_anchors: Vec<String> = anchors
        .iter()
        .filter(|id| !records.nodes.contains_key(*id))
        .cloned()
        .collect();
    visited.extend(missing_anchors.iter().cloned());
    frontier.retain(|id| !missing_anchors.contains(id));

    let reserved_unknown = anchors
        .iter()
        .filter(|id| !records.nodes.contains_key(*id))
        .count();
    let activation_cap = ACTIVATION_BUDGET.saturating_sub(reserved_unknown);
    while !frontier.is_empty() && records.nodes.len() < activation_cap && pages_loaded < page_limit
    {
        let remaining = activation_cap - records.nodes.len();
        let batch_limit = remaining.clamp(1, ACTIVATION_PAGE / 2);
        let mut batch = Vec::with_capacity(batch_limit);
        while batch.len() < batch_limit {
            let Some(id) = frontier.pop_front() else {
                break;
            };
            if !visited.contains(&id) && !batch.contains(&id) {
                batch.push(id);
            }
        }
        if batch.is_empty() {
            break;
        }
        if visited.len().saturating_add(batch.len()) > 2048 {
            for id in batch.into_iter().rev() {
                frontier.push_front(id);
            }
            break;
        }
        let page = store.graph_neighbors(key, &batch, remaining.min(ACTIVATION_PAGE))?;
        pages_loaded += 1;
        let loaded_ids: Vec<String> = page.nodes.keys().cloned().collect();
        merge_slice(&mut records, page, ACTIVATION_BUDGET);
        visited.extend(batch);
        for id in loaded_ids {
            // Newly loaded nodes are queued even when their record was returned
            // as a neighbor: the next page must inspect their outgoing links.
            if let Some(edges) = records.edges.get(&id) {
                for target in edges.keys() {
                    if !visited.contains(target) && !frontier.contains(target) {
                        frontier.push_back(target.clone());
                    }
                }
            }
        }
        let provisional = ActivationStatus {
            phase: if frontier.is_empty() {
                "complete"
            } else {
                "provisional"
            }
            .into(),
            nodes_loaded: records.nodes.len(),
            node_budget: ACTIVATION_BUDGET,
            pages_loaded,
            source_refs_seen: count_source_refs(&records),
            sources_loaded: 0,
            frontier_remaining: frontier.len(),
            can_continue: true,
            source_matches: Vec::new(),
        };
        progress(&provisional);
    }

    let phase = if frontier.is_empty() {
        "complete"
    } else {
        "budget_limited"
    };
    let source_refs_seen = count_source_refs(&records);
    let interaction_ids = records
        .nodes
        .values()
        .filter_map(|node| {
            node.extra
                .get("source_episode_ids")
                .and_then(serde_json::Value::as_array)
        })
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter_map(|id| id.strip_prefix("episode:")?.parse::<u64>().ok())
        .collect::<std::collections::BTreeSet<_>>();
    let archive_ids: Vec<u64> = interaction_ids
        .into_iter()
        .flat_map(|user_event| [user_event, user_event.saturating_add(1)])
        .filter(|id| *id > 0)
        .take(32)
        .collect();
    let episodes = store.archive_by_ids(key, &archive_ids)?;
    let status = ActivationStatus {
        phase: phase.into(),
        nodes_loaded: records.nodes.len(),
        node_budget: ACTIVATION_BUDGET,
        pages_loaded,
        source_refs_seen,
        sources_loaded: episodes.len(),
        frontier_remaining: frontier.len(),
        can_continue: !frontier.is_empty()
            && anchors.len() < ACTIVATION_BUDGET
            && visited.len() < 2048
            && checkpoint_size_ok(query, &frontier, &visited),
        source_matches: source_matches(query, &episodes),
    };
    progress(&status);
    Ok((
        records,
        status,
        frontier.into_iter().collect(),
        visited.into_iter().collect(),
        episodes,
    ))
}

fn count_source_refs(records: &GraphSlice) -> usize {
    records
        .nodes
        .values()
        .filter_map(|node| {
            node.extra
                .get("source_episode_ids")
                .and_then(serde_json::Value::as_array)
        })
        .map(Vec::len)
        .sum()
}

fn checkpoint_size_ok(
    query: &str,
    frontier: &std::collections::VecDeque<String>,
    visited: &std::collections::BTreeSet<String>,
) -> bool {
    // The encrypted checkpoint has a fixed 32 KiB storage limit.
    query.len()
        + frontier.iter().map(String::len).sum::<usize>()
        + visited.iter().map(String::len).sum::<usize>()
        < 24 * 1024
}

fn checkpoint_fits(checkpoint: &TraversalCheckpoint) -> bool {
    checkpoint.visited.len() <= 2048
        && checkpoint.frontier.len() <= 128
        && checkpoint_size_ok(
            &checkpoint.query,
            &checkpoint.frontier.iter().cloned().collect(),
            &checkpoint.visited.iter().cloned().collect(),
        )
}

fn source_matches(query: &str, episodes: &[serde_json::Value]) -> Vec<SourceMatch> {
    let query_set: std::collections::BTreeSet<_> = graph::tokens(query).into_iter().collect();
    let mut matches = Vec::new();
    for episode in episodes {
        let (Some(id), Some(speaker), Some(text)) = (
            episode["id"].as_str(),
            episode["speaker"].as_str(),
            episode["text"].as_str(),
        ) else {
            continue;
        };
        let token_overlap = graph::tokens(text)
            .iter()
            .filter(|token| query_set.contains(*token))
            .count();
        if token_overlap > 0 {
            matches.push(SourceMatch {
                id: id.to_owned(),
                speaker: speaker.to_owned(),
                unix_time: episode["unix_time"].as_u64().unwrap_or(0),
                token_overlap,
            });
        }
    }
    matches.sort_by(|a, b| {
        b.token_overlap
            .cmp(&a.token_overlap)
            .then(b.unix_time.cmp(&a.unix_time))
    });
    matches.truncate(8);
    matches
}
