//! Encrypted graph paging: bounded record and neighbor pages, the keyed
//! checkpoint spill slots, and the streaming full-graph integrity audit.
use super::{graph_aad, graph_lookup, keyed_lookup, GraphRecord, Store, TraversalCheckpoint};
use crate::{
    crypto::{self, Key},
    model::GraphSlice,
};
use anyhow::{bail, Result};
use rusqlite::{params, OptionalExtension};
use zeroize::Zeroizing;

fn checkpoint_lookup(key: &Key, id: &str) -> Vec<u8> {
    keyed_lookup(key, b"saeugling-v3-checkpoint-index", id)
}

fn hex_prefix(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn open_graph_record(key: &Key, id: &str, lookup: &[u8], blob: &[u8]) -> Result<GraphRecord> {
    if lookup != graph_lookup(key, id) {
        bail!("Graph-Index passt nicht zur Knoten-ID");
    }
    let plain = crypto::unseal(key, blob, &graph_aad(lookup))?;
    let record: GraphRecord = serde_json::from_slice(&plain)?;
    if record.node.id != id {
        bail!("Graph-Knoten-ID stimmt nicht");
    }
    Ok(record)
}

impl Store {
    /// Explicit full graph audit. It streams one encrypted record at a time,
    /// authenticates the keyed lookup/AAD binding, and confirms every edge
    /// target exists without materializing the whole graph in RAM.
    pub(crate) fn verify_graph_full(
        &self,
        key: &Key,
        expected_count: u64,
        expected_root: &[u8],
    ) -> Result<()> {
        let (manifest_count, manifest_root): (u64, Vec<u8>) = self.conn.query_row(
            "SELECT node_count,root FROM graph_manifest WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if manifest_count != expected_count || manifest_root != expected_root {
            bail!("Graph-Manifest und Prüfstand widersprechen sich");
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id_hash,sealed FROM graph_records ORDER BY id_hash")?;
        let mut rows = stmt.query([])?;
        let mut count = 0u64;
        while let Some(row) = rows.next()? {
            let lookup: Vec<u8> = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            let plain = crypto::unseal(key, &blob, &graph_aad(&lookup))?;
            let record: GraphRecord = serde_json::from_slice(&plain)?;
            if graph_lookup(key, &record.node.id) != lookup
                || record.node.id.len() > 1024
                || !["LEX", "RING", "KERN"].contains(&record.node.typ.as_str())
                || !record.node.staerke.is_finite()
                || !(0.0..=1.0).contains(&record.node.staerke)
            {
                bail!("Graph-Datensatz ungültig");
            }
            if let Some(ring) = &record.node.ring {
                if !self.graph_record_exists(key, ring)? {
                    bail!("Graph-Ring verweist auf fehlenden Datensatz");
                }
            }
            for (target, weight) in &record.edges {
                if !weight.is_finite()
                    || !(-1.0..=1.0).contains(weight)
                    || !self.graph_record_exists(key, target)?
                {
                    bail!("Graph-Kante ungültig oder ohne Ziel");
                }
            }
            count += 1;
        }
        if count != expected_count {
            bail!("Graph-Datensätze fehlen oder sind zusätzlich");
        }
        Ok(())
    }

    fn graph_record_exists(&self, key: &Key, id: &str) -> Result<bool> {
        let lookup = graph_lookup(key, id);
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM graph_records WHERE id_hash=?1)",
            [lookup.as_slice()],
            |r| r.get(0),
        )?)
    }

    /// Fetch explicitly requested encrypted graph records, capped to 128 nodes.
    /// A missing record is distinct from an unread page and is returned as absence.
    pub(crate) fn graph_nodes(&self, key: &Key, ids: &[String]) -> Result<GraphSlice> {
        let mut result = GraphSlice::default();
        for id in ids.iter().take(128) {
            let lookup = graph_lookup(key, id);
            let blob: Option<Vec<u8>> = self
                .conn
                .query_row(
                    "SELECT sealed FROM graph_records WHERE id_hash=?1",
                    [lookup.as_slice()],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(blob) = blob {
                let record = open_graph_record(key, id, &lookup, &blob)?;
                result.edges.insert(id.clone(), record.edges);
                result.nodes.insert(id.clone(), record.node);
            }
        }
        Ok(result)
    }

    /// Load seeds plus their immediate targets within a fixed node budget.
    pub(crate) fn graph_neighbors(
        &self,
        key: &Key,
        frontier: &[String],
        budget: usize,
    ) -> Result<GraphSlice> {
        let first = self.graph_nodes(key, &frontier[..frontier.len().min(128)])?;
        let mut ids: Vec<String> = first.nodes.keys().cloned().collect();
        for edges in first.edges.values() {
            for id in edges.keys() {
                if !ids.contains(id) && ids.len() < budget.clamp(1, 128) {
                    ids.push(id.clone());
                }
            }
        }
        let loaded = self.graph_nodes(key, &ids)?;
        Ok(loaded)
    }

    /// Persist a bounded traversal spill only for the caller's current model revision.
    /// Checkpoint identifiers are keyed; query text and session IDs stay out of SQLite.
    pub(crate) fn save_checkpoint(
        &mut self,
        key: &Key,
        id: &str,
        checkpoint: &TraversalCheckpoint,
    ) -> Result<bool> {
        checkpoint.validate()?;
        if checkpoint
            .frontier
            .iter()
            .chain(checkpoint.visited.iter())
            .any(|v| v.len() > 1024)
        {
            bail!("Checkpoint-ID zu lang");
        }
        let plain = serde_json::to_vec(checkpoint)?;
        if plain.len() > 32 * 1024 {
            bail!("Denkcheckpoint überschreitet 32-KiB-Budget");
        }
        let lookup = checkpoint_lookup(key, id);
        let aad = self.aad(&format!("checkpoint:{}", hex_prefix(&lookup)));
        let sealed = crypto::seal(key, &Zeroizing::new(plain), &aad)?;
        let tx = self.conn.transaction()?;
        let existing: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM traversal_checkpoints WHERE id_hash=?1)",
            [lookup.as_slice()],
            |r| r.get(0),
        )?;
        let count: u64 = tx.query_row("SELECT COUNT(*) FROM traversal_checkpoints", [], |r| {
            r.get(0)
        })?;
        if !existing && count >= 128 {
            return Ok(false);
        }
        tx.execute("INSERT INTO traversal_checkpoints(id_hash,revision,sealed) VALUES(?1,?2,?3) ON CONFLICT(id_hash) DO UPDATE SET revision=excluded.revision,sealed=excluded.sealed",params![lookup,checkpoint.revision,sealed])?;
        tx.commit()?;
        Ok(true)
    }

    pub(crate) fn load_checkpoint(
        &mut self,
        key: &Key,
        id: &str,
        current_revision: u64,
    ) -> Result<Option<TraversalCheckpoint>> {
        let lookup = checkpoint_lookup(key, id);
        let row: Option<(u64, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT revision,sealed FROM traversal_checkpoints WHERE id_hash=?1",
                [lookup.as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((revision, blob)) = row else {
            return Ok(None);
        };
        if revision != current_revision {
            self.conn.execute(
                "DELETE FROM traversal_checkpoints WHERE id_hash=?1",
                [lookup],
            )?;
            return Ok(None);
        }
        let aad = self.aad(&format!("checkpoint:{}", hex_prefix(&lookup)));
        let plain = crypto::unseal(key, &blob, &aad)?;
        let checkpoint: TraversalCheckpoint = serde_json::from_slice(&plain)?;
        checkpoint.validate()?;
        Ok(Some(checkpoint))
    }

    pub(crate) fn clear_checkpoint(&mut self, key: &Key, id: &str) -> Result<bool> {
        let lookup = checkpoint_lookup(key, id);
        Ok(self.conn.execute(
            "DELETE FROM traversal_checkpoints WHERE id_hash=?1",
            [lookup],
        )? > 0)
    }
}
