//! Write-ahead op-journal persistence: the durable pre-commit append with
//! the configured cap, and crash recovery that discards unconfirmed
//! entries on load.
use super::Store;
use crate::{crypto, crypto::Key, journal::JournalEintrag};
use anyhow::{bail, Result};
use rusqlite::params;
use zeroize::Zeroizing;

impl Store {
    /// Write-ahead append of one learning step, durable before its state
    /// change commits. Applies the configured journal cap by dropping the
    /// oldest entries.
    pub(crate) fn journal_append(
        &mut self,
        key: &Key,
        eintrag: &JournalEintrag,
        kap: usize,
    ) -> Result<()> {
        let sealed = crypto::seal(
            key,
            &Zeroizing::new(serde_json::to_vec(eintrag)?),
            &self.aad(&format!("opjournal:{}", eintrag.seq)),
        )?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO op_journal VALUES(?1,?2)",
            params![eintrag.seq, sealed],
        )?;
        let count: u64 = tx.query_row("SELECT COUNT(*) FROM op_journal", [], |r| r.get(0))?;
        if count > kap as u64 {
            tx.execute(
                "DELETE FROM op_journal WHERE seq IN (SELECT seq FROM op_journal ORDER BY seq LIMIT ?1)",
                params![count - kap as u64],
            )?;
        }
        Ok(tx.commit()?)
    }

    /// Crash recovery and bounded load of the rollbackable learning steps.
    /// Entries beyond the committed confirmation seq were never part of a
    /// persisted state, so they are discarded and counted for the honest
    /// first /status notice.
    pub(crate) fn journal_sync(
        &mut self,
        key: &Key,
        bestaetigt_bis: u64,
        kap: usize,
    ) -> Result<(Vec<JournalEintrag>, u64)> {
        let verworfen = self.conn.execute(
            "DELETE FROM op_journal WHERE seq > ?1",
            params![bestaetigt_bis],
        )? as u64;
        let count: u64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM op_journal", [], |r| r.get(0))?;
        if count > kap as u64 {
            self.conn.execute(
                "DELETE FROM op_journal WHERE seq IN (SELECT seq FROM op_journal ORDER BY seq LIMIT ?1)",
                params![count - kap as u64],
            )?;
        }
        let mut stmt = self
            .conn
            .prepare("SELECT seq,sealed FROM op_journal ORDER BY seq")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, u64>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
        let mut journal = Vec::new();
        for row in rows {
            let (seq, blob) = row?;
            let plain = crypto::unseal(key, &blob, &self.aad(&format!("opjournal:{seq}")))?;
            let eintrag: JournalEintrag = serde_json::from_slice(&plain)?;
            if eintrag.seq != seq {
                bail!("Op-Journal-Sequenz passt nicht zum Eintrag");
            }
            journal.push(eintrag);
        }
        Ok((journal, verworfen))
    }
}
