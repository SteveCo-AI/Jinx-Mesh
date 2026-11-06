//! Vault storage: sealed payload, store handle, and the shared keyed-lookup
//! and integrity-root helpers. The SQL and encryption work lives in the
//! domain submodules below.
mod access;
mod archive;
mod journal;
mod paging;

use crate::{
    crypto::Key,
    model::{Node, State},
};
use anyhow::{bail, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fs::File, path::PathBuf};
use zeroize::Zeroize;

/// Explicit owner-selected Python files. They are read, never overwritten.
pub struct Import {
    pub state: PathBuf,
    pub actions: PathBuf,
    pub legacy_audit: Option<PathBuf>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Payload {
    pub state: State,
    pub actions: Value,
    pub totp: Option<String>,
    pub last_totp: Option<u64>,
    pub sequence: u64,
    #[serde(default)]
    pub archive_count: u64,
    #[serde(default)]
    pub archive_root: Vec<u8>,
    #[serde(default)]
    pub graph_count: u64,
    #[serde(default)]
    pub graph_root: Vec<u8>,
    #[serde(default)]
    pub op_journal_seq: u64,
}

#[derive(Serialize, Deserialize)]
struct GraphRecord {
    node: Node,
    edges: BTreeMap<String, f64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct TraversalCheckpoint {
    pub revision: u64,
    pub frontier: Vec<String>,
    pub visited: Vec<String>,
    #[serde(default)]
    pub query: String,
    pub provisional: String,
}
impl TraversalCheckpoint {
    fn validate(&self) -> Result<()> {
        if self.frontier.len() > 128
            || self.visited.len() > 2048
            || self.query.len() > 16 * 1024
            || self.provisional.len() > 16 * 1024
        {
            bail!("Denkcheckpoint überschreitet sein Budget");
        }
        Ok(())
    }
}
impl Drop for Payload {
    fn drop(&mut self) {
        if let Some(secret) = &mut self.totp {
            secret.zeroize();
        }
    }
}

pub(crate) struct Store {
    conn: Connection,
    _lock: File,
    username: String,
    salt: Vec<u8>,
    version: u64,
    /// Test-only crash simulation: fail the next commit before it starts,
    /// leaving a write-ahead op-journal entry unconfirmed.
    #[cfg(test)]
    pub(crate) crash_vor_commit: bool,
}

impl Store {
    pub(crate) fn version(&self) -> u64 {
        self.version
    }
    fn aad(&self, domain: &str) -> Vec<u8> {
        format!("saeugling-v{}:{}:{domain}", self.version, self.username).into_bytes()
    }
}

fn graph_lookup(key: &Key, id: &str) -> Vec<u8> {
    keyed_lookup(key, b"saeugling-v3-graph-index", id)
}

fn keyed_lookup(key: &Key, domain: &[u8], id: &str) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_ref()).expect("HMAC accepts any key");
    mac.update(domain);
    mac.update(&(id.len() as u64).to_be_bytes());
    mac.update(id.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn graph_aad(lookup: &[u8]) -> Vec<u8> {
    let mut aad = b"saeugling-v3:graph-record:".to_vec();
    aad.extend_from_slice(lookup);
    aad
}

fn archive_next_root(previous: &[u8], seq: u64, sealed: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(previous);
    h.update(seq.to_be_bytes());
    h.update(sealed);
    h.finalize().to_vec()
}

#[cfg(test)]
mod storage_tests {
    use super::{Store, TraversalCheckpoint};
    use crate::model::{GraphSlice, Node};
    use serde_json::json;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    const PASS: &str = "Storage test passphrase 314159!";

    fn node(id: String) -> Node {
        Node {
            id,
            typ: "LEX".into(),
            label: "word".into(),
            staerke: 0.5,
            alter: "new".into(),
            nutzt: 0,
            geboren: 0,
            ring: None,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn format3_keeps_snapshot_small_and_reads_graph_and_archive_in_bounded_pages() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("vault.db");
        let mut store = Store::create(&path, "owner", PASS, None, None).unwrap();
        let (mut payload, key) = store.unlock("owner", PASS).unwrap();
        let old = payload.state.clone();
        let mut learned = old.clone();
        let mut updates = GraphSlice::default();
        for n in 0..140 {
            let id = format!("lex:word{n:03}");
            let node = node(id.clone());
            learned.neuronen.insert(id.clone(), node.clone());
            updates.nodes.insert(id.clone(), node);
        }
        let targets = learned
            .neuronen
            .keys()
            .filter(|id| id.as_str() != "lex:word000")
            .map(|id| (id.clone(), 0.25))
            .collect::<BTreeMap<_, _>>();
        learned.kanten.insert("lex:word000".into(), targets.clone());
        updates.edges.insert("lex:word000".into(), targets);
        learned.validate().unwrap();
        payload.state = learned;
        let episodes = vec![
            json!({"id":"episode:1","sequence":1,"speaker":"user","text":"first"}),
            json!({"id":"reply:1","sequence":1,"speaker":"assistant","text":"answer"}),
            json!({"id":"episode:2","sequence":2,"speaker":"user","text":"second"}),
        ];
        store
            .commit(
                &mut payload,
                &old,
                &updates,
                &key,
                "test",
                &episodes,
                &Default::default(),
            )
            .unwrap();
        assert_eq!(payload.graph_count, 140);
        assert_eq!(payload.archive_count, 3);

        let seeds = (0..140)
            .map(|n| format!("lex:word{n:03}"))
            .collect::<Vec<_>>();
        assert_eq!(store.graph_nodes(&key, &seeds).unwrap().nodes.len(), 128);
        let slice = store
            .graph_neighbors(&key, &["lex:word000".into()], 16)
            .unwrap();
        assert!(slice.nodes.len() <= 16);
        assert_eq!(store.archive_by_ids(&key, &[3, 1]).unwrap().len(), 2);

        store
            .verify_graph_full(&key, payload.graph_count, &payload.graph_root)
            .unwrap();
        store
            .verify_archive_full(&key, payload.archive_count, &payload.archive_root)
            .unwrap();

        let checkpoint = TraversalCheckpoint {
            revision: 7,
            frontier: vec!["lex:word000".into()],
            visited: vec!["lex:word001".into()],
            query: "continue this query".into(),
            provisional: "word".into(),
        };
        assert!(store.save_checkpoint(&key, "session", &checkpoint).unwrap());
        assert!(store.load_checkpoint(&key, "session", 7).unwrap().is_some());
        assert!(store.load_checkpoint(&key, "session", 8).unwrap().is_none());
        drop(store);

        let reopened = Store::open(&path).unwrap();
        let (metadata, key) = reopened.unlock("owner", PASS).unwrap();
        assert!(metadata.state.neuronen.is_empty());
        assert_eq!(metadata.graph_count, 140);
        assert!(reopened
            .graph_nodes(&key, &["lex:word000".into()])
            .unwrap()
            .nodes
            .contains_key("lex:word000"));
    }

    #[test]
    fn selected_graph_record_tampering_is_detected_on_load_and_full_audit() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("vault.db");
        let mut store = Store::create(&path, "owner", PASS, None, None).unwrap();
        let (mut payload, key) = store.unlock("owner", PASS).unwrap();
        let old = payload.state.clone();
        let n = node("lex:tamper-me".into());
        let mut dirty = GraphSlice::default();
        dirty.nodes.insert(n.id.clone(), n.clone());
        payload.state.neuronen.insert(n.id.clone(), n);
        store
            .commit(
                &mut payload,
                &old,
                &dirty,
                &key,
                "test",
                &[],
                &Default::default(),
            )
            .unwrap();
        let mut blob: Vec<u8> = store
            .conn
            .query_row("SELECT sealed FROM graph_records LIMIT 1", [], |r| r.get(0))
            .unwrap();
        blob[0] ^= 1;
        store
            .conn
            .execute("UPDATE graph_records SET sealed=?1", [blob])
            .unwrap();
        assert!(store.graph_nodes(&key, &["lex:tamper-me".into()]).is_err());
        assert!(store
            .verify_graph_full(&key, payload.graph_count, &payload.graph_root)
            .is_err());
    }
}
