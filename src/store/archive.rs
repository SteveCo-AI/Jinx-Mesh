//! Conversation archive: bounded pages of individually authenticated
//! episode blocks, exact-ID fetches, and the full/manifest integrity
//! checks with the rolling archive root.
use super::{archive_next_root, Store};
use crate::crypto::{self, Key};
use anyhow::{bail, Result};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

impl Store {
    pub fn archive_page(&self, key: &Key, offset: u64, limit: usize) -> Result<Vec<Value>> {
        let limit = limit.clamp(1, 32) as u64;
        let mut stmt = self.conn.prepare(
            "SELECT seq,sealed FROM conversation_archive ORDER BY seq LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit, offset], |r| {
            Ok((r.get::<_, u64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, sealed) = row?;
            let plain = crypto::unseal(key, &sealed, &self.aad(&format!("episode:{seq}")))?;
            out.push(serde_json::from_slice(&plain)?);
        }
        Ok(out)
    }

    /// Fetch authenticated source episodes by their exact monotonically assigned IDs.
    /// At most 32 records are read; no archive scan or broad range is performed.
    pub(crate) fn archive_by_ids(&self, key: &Key, ids: &[u64]) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for seq in ids.iter().copied().take(32) {
            if seq == 0 {
                continue;
            }
            let blob: Option<Vec<u8>> = self
                .conn
                .query_row(
                    "SELECT sealed FROM conversation_archive WHERE seq=?1",
                    [seq],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(blob) = blob else {
                continue;
            };
            let plain = crypto::unseal(key, &blob, &self.aad(&format!("episode:{seq}")))?;
            out.push(serde_json::from_slice(&plain)?);
        }
        Ok(out)
    }

    pub fn archive_len(&self) -> Result<u64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM conversation_archive", [], |r| {
                r.get(0)
            })?)
    }

    pub(crate) fn verify_archive_full(
        &self,
        key: &Key,
        expected_count: u64,
        expected_root: &[u8],
    ) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare("SELECT seq,sealed FROM conversation_archive ORDER BY seq")?;
        let mut rows = stmt.query([])?;
        let mut count = 0u64;
        let mut root = vec![0; 32];
        while let Some(row) = rows.next()? {
            let seq: u64 = row.get(0)?;
            let sealed: Vec<u8> = row.get(1)?;
            count += 1;
            if seq != count {
                bail!("Lücke im Gesprächsarchiv");
            }
            root = archive_next_root(&root, seq, &sealed);
            // Verify authenticity without retaining plaintext or history in memory.
            let aad = self.aad(&format!("episode:{seq}"));
            let _ = crypto::unseal(key, &sealed, &aad)?;
        }
        if count != expected_count || root != expected_root {
            bail!("Gesprächsarchiv und Prüfstand widersprechen sich");
        }
        Ok(())
    }

    pub(super) fn verify_archive_manifest(
        &self,
        expected_count: u64,
        expected_root: &[u8],
    ) -> Result<()> {
        let (count, root): (u64, Vec<u8>) = self.conn.query_row(
            "SELECT episode_count,root FROM archive_manifest WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if count != expected_count || root != expected_root {
            bail!("Gesprächsarchiv und Prüfstand widersprechen sich");
        }
        let max_seq: u64 = self.conn.query_row(
            "SELECT COALESCE(MAX(seq),0) FROM conversation_archive",
            [],
            |r| r.get(0),
        )?;
        if max_seq != count {
            bail!("Gesprächsarchiv hat fehlende oder zusätzliche Blöcke");
        }
        Ok(())
    }
}
