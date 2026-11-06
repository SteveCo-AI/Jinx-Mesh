//! App session coordination: shared session state, lifecycle constructors,
//! and cross-domain helpers. The public API is assembled from the domain
//! submodules below.
mod activation;
mod auth;
mod message;
mod recall_checkpoint;
mod undo_heal;
mod view;

pub use auth::{Mode, SessionPolicy};
pub use recall_checkpoint::RecallResult;

use crate::{
    crypto::Key,
    engine::Engine,
    journal::JournalEintrag,
    model::{ActivationStatus, GraphSlice, State},
    store::{Import, Payload, Store},
};
use anyhow::{bail, Result};
#[cfg(test)]
use serde_json::json;
use std::{path::Path, time::Instant};
use totp_rs::{Algorithm, Secret, TOTP};
use zeroize::Zeroize;

const ACTIVATION_BUDGET: usize = 128;
const ACTIVATION_PAGE: usize = 32;

struct Session {
    engine: Engine,
    graph_cache: GraphSlice,
    prefetch_status: Option<ActivationStatus>,
    key: Option<Key>,
    read_key: Option<Key>,
    feedback_target: Option<serde_json::Value>,
    payload: Payload,
    started: Instant,
    last: Instant,
    frozen: bool,
    /// Committed, still rollbackable learning steps (Kap. 14 op-journal).
    journal: Vec<JournalEintrag>,
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
    /// Unconfirmed op-journal steps discarded by recovery, still waiting to be
    /// reported honestly at the first /status.
    verworfen_ungemeldet: u64,
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
            verworfen_ungemeldet: 0,
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
            verworfen_ungemeldet: 0,
        })
    }
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            store: Store::open(path)?,
            session: None,
            policy: SessionPolicy::default(),
            verworfen_ungemeldet: 0,
        })
    }
    pub fn with_policy(mut self, policy: SessionPolicy) -> Self {
        self.policy = policy;
        self
    }
    pub fn new_totp_secret() -> String {
        Secret::generate_secret().to_encoded().to_string()
    }
}

#[cfg(test)]
impl App {
    /// Affected learning section (neurons, edges, extra runs) for exact
    /// rollback assertions in the op-journal tests. Empty outgoing edge maps
    /// are a paging artifact of the resident slice, not a persisted
    /// difference, so the comparison view drops them.
    pub(crate) fn test_abschnitt(&self) -> serde_json::Value {
        let Some(s) = self.session.as_ref() else {
            return serde_json::Value::Null;
        };
        let kanten: std::collections::BTreeMap<&str, _> = s
            .engine
            .state
            .kanten
            .iter()
            .filter(|(_, ziele)| !ziele.is_empty())
            .map(|(id, ziele)| (id.as_str(), ziele))
            .collect();
        json!({
            "neuronen": s.engine.state.neuronen,
            "kanten": kanten,
            "extra": s.engine.state.extra,
        })
    }

    /// Simulate a crash after the write-ahead journal append, before the
    /// learning commit, leaving an unconfirmed entry for recovery.
    pub(crate) fn test_crash_vor_commit(&mut self) {
        self.store.crash_vor_commit = true;
    }
}

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
