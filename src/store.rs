use crate::{
    crypto::{self, Key},
    journal::{JournalEintrag, JournalVermerk},
    model::{GraphSlice, Node, State},
};
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
};
use zeroize::{Zeroize, Zeroizing};

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
fn limited(path: &Path) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    File::open(path)?
        .take(crypto::MAX_PLAIN as u64 + 1)
        .read_to_end(&mut out)?;
    if out.len() > crypto::MAX_PLAIN {
        bail!("Import zu groß");
    }
    Ok(out)
}
fn lock(path: &Path) -> Result<File> {
    let lockpath = path.with_extension("lock");
    if lockpath.is_symlink() {
        bail!("Symlink als Sperrdatei abgelehnt");
    }
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let f = opts.open(lockpath)?;
    f.try_lock_exclusive()
        .context("Vault bereits durch einen anderen Prozess geöffnet")?;
    Ok(f)
}
impl Store {
    pub fn create(
        path: &Path,
        user: &str,
        password: &str,
        totp: Option<String>,
        import: Option<Import>,
    ) -> Result<Self> {
        if user.is_empty()
            || user.len() > 64
            || !user
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            bail!("Nutzername: 1–64 ASCII-Buchstaben, Ziffern, _ oder -");
        }
        if password.chars().count() < 12 || password.len() > 1024 {
            bail!("Passwort: mindestens 12 Zeichen, höchstens 1024 Bytes");
        }
        let (state, actions, legacy) = if let Some(import) = import {
            let raw = Zeroizing::new(limited(&import.state)?);
            let bytes = if import.state.extension().is_some_and(|e| e == "gz") {
                let mut out = Zeroizing::new(Vec::new());
                flate2::read::GzDecoder::new(&raw[..])
                    .take(crypto::MAX_PLAIN as u64 + 1)
                    .read_to_end(&mut out)?;
                out
            } else {
                raw
            };
            if bytes.len() > crypto::MAX_PLAIN {
                bail!("Import zu groß");
            }
            let state: State = serde_json::from_slice(&bytes)?;
            state.validate()?;
            let actions: Value = serde_json::from_slice(&limited(&import.actions)?)?;
            let state = crate::engine::Engine::new(state, actions.clone())?.state;
            let legacy = import.legacy_audit.map(|p| limited(&p)).transpose()?;
            (state, actions, legacy)
        } else {
            (State::empty(), json!({"aktionen":{}}), None)
        };
        let guard = lock(path)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(path)
            .context("Vault existiert bereits oder Ziel ist nicht beschreibbar")?;
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;
            CREATE TABLE metadata(id INTEGER PRIMARY KEY CHECK(id=1),version INTEGER NOT NULL,username TEXT NOT NULL,salt BLOB NOT NULL);
            CREATE TABLE snapshot(id INTEGER PRIMARY KEY CHECK(id=1),sealed BLOB NOT NULL);
            CREATE TABLE audit(seq INTEGER PRIMARY KEY,sealed BLOB NOT NULL);
            CREATE TABLE baseline(id INTEGER PRIMARY KEY CHECK(id=1),sealed BLOB NOT NULL);
            CREATE TABLE legacy(id INTEGER PRIMARY KEY,sealed BLOB NOT NULL);
            CREATE TABLE conversation_archive(seq INTEGER PRIMARY KEY, sealed BLOB NOT NULL);
            CREATE TABLE archive_manifest(id INTEGER PRIMARY KEY CHECK(id=1), episode_count INTEGER NOT NULL, root BLOB NOT NULL);
            CREATE TABLE graph_shards(id INTEGER PRIMARY KEY CHECK(id BETWEEN 0 AND 255), sealed BLOB NOT NULL);
            CREATE TABLE graph_records(id_hash BLOB PRIMARY KEY, sealed BLOB NOT NULL);
            CREATE TABLE graph_manifest(id INTEGER PRIMARY KEY CHECK(id=1), node_count INTEGER NOT NULL, root BLOB NOT NULL);
            CREATE TABLE traversal_checkpoints(id_hash BLOB PRIMARY KEY, revision INTEGER NOT NULL, sealed BLOB NOT NULL);
            CREATE TABLE throttle(id INTEGER PRIMARY KEY CHECK(id=1),failures INTEGER NOT NULL,next_allowed INTEGER NOT NULL);
            CREATE TABLE op_journal(seq INTEGER PRIMARY KEY, sealed BLOB NOT NULL);")?;
        let salt = crypto::random::<16>().to_vec();
        let key = crypto::derive(password, &salt)?;
        let graph_records = seal_graph_records(&state, &key)?;
        let graph_count = state.neuronen.len() as u64;
        let graph_root = graph_records_root(&key, &graph_records);
        let mut store = Self {
            conn,
            _lock: guard,
            username: user.into(),
            salt,
            version: 3,
            #[cfg(test)]
            crash_vor_commit: false,
        };
        let mut payload = Payload {
            state,
            actions,
            totp,
            last_totp: None,
            sequence: 0,
            archive_count: 0,
            archive_root: vec![0; 32],
            graph_count,
            graph_root: graph_root.clone(),
            op_journal_seq: 0,
        };
        payload.state = state_projection(&payload.state, store.version);
        let sealed = crypto::seal(
            &key,
            &Zeroizing::new(serde_json::to_vec(&payload)?),
            &store.aad("snapshot"),
        )?;
        let baseline = crypto::seal(
            &key,
            &Zeroizing::new(serde_json::to_vec(&payload.state)?),
            &store.aad("baseline"),
        )?;
        let graph_pages: Vec<(u8, Vec<u8>)> = Vec::new();
        let legacy = legacy
            .map(|b| crypto::seal(&key, &Zeroizing::new(b), &store.aad("legacy")))
            .transpose()?;
        let tx = store.conn.transaction()?;
        tx.execute(
            "INSERT INTO metadata VALUES(1,3,?1,?2)",
            params![store.username, store.salt],
        )?;
        tx.execute("INSERT INTO snapshot VALUES(1,?1)", [sealed])?;
        tx.execute("INSERT INTO baseline VALUES(1,?1)", [baseline])?;
        for (id, sealed) in graph_pages {
            tx.execute(
                "INSERT INTO graph_shards(id,sealed) VALUES(?1,?2)",
                params![id, sealed],
            )?;
        }
        for (lookup, sealed) in graph_records {
            tx.execute(
                "INSERT INTO graph_records(id_hash,sealed) VALUES(?1,?2)",
                params![lookup, sealed],
            )?;
        }
        tx.execute(
            "INSERT INTO graph_manifest VALUES(1,?1,?2)",
            params![graph_count, graph_root],
        )?;
        tx.execute(
            "INSERT INTO archive_manifest VALUES(1,0,?1)",
            params![vec![0u8; 32]],
        )?;
        tx.execute("INSERT INTO throttle VALUES(1,0,0)", [])?;
        if let Some(legacy) = legacy {
            tx.execute("INSERT INTO legacy VALUES(1,?1)", [legacy])?;
        }
        tx.commit()?;
        Ok(store)
    }
    pub fn open(path: &Path) -> Result<Self> {
        if path.is_symlink() {
            bail!("Symlink als Vault abgelehnt");
        }
        let guard = lock(path)?;
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.execute_batch(
            "PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA trusted_schema=OFF;",
        )?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS conversation_archive(seq INTEGER PRIMARY KEY, sealed BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS archive_manifest(id INTEGER PRIMARY KEY CHECK(id=1), episode_count INTEGER NOT NULL, root BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS graph_shards(id INTEGER PRIMARY KEY CHECK(id BETWEEN 0 AND 255), sealed BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS graph_records(id_hash BLOB PRIMARY KEY, sealed BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS graph_manifest(id INTEGER PRIMARY KEY CHECK(id=1), node_count INTEGER NOT NULL, root BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS traversal_checkpoints(id_hash BLOB PRIMARY KEY, revision INTEGER NOT NULL, sealed BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS op_journal(seq INTEGER PRIMARY KEY, sealed BLOB NOT NULL);")?;
        let (version, username, salt): (u64, String, Vec<u8>) = conn.query_row(
            "SELECT version,username,salt FROM metadata WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if !(2..=3).contains(&version) || salt.len() != 16 {
            bail!("Unbekanntes Vault-Format");
        }
        Ok(Self {
            conn,
            _lock: guard,
            username,
            salt,
            version,
            #[cfg(test)]
            crash_vor_commit: false,
        })
    }
    pub(crate) fn version(&self) -> u64 {
        self.version
    }
    fn aad(&self, domain: &str) -> Vec<u8> {
        format!("saeugling-v{}:{}:{domain}", self.version, self.username).into_bytes()
    }
    pub fn check_throttle(&self, now: u64) -> Result<()> {
        let next: u64 =
            self.conn
                .query_row("SELECT next_allowed FROM throttle WHERE id=1", [], |r| {
                    r.get(0)
                })?;
        if now < next {
            bail!("Anmeldung vorübergehend gesperrt; später erneut versuchen");
        }
        Ok(())
    }
    pub fn failure(&self, now: u64) -> Result<()> {
        self.conn.execute("UPDATE throttle SET failures=MIN(failures+1,6),next_allowed=?1+MIN(2 << MIN(failures,6),60) WHERE id=1",[now])?;
        Ok(())
    }
    pub fn unlock(&self, user: &str, password: &str) -> Result<(Payload, Key)> {
        let key = crypto::derive(password, &self.salt)?;
        let blob: Vec<u8> =
            self.conn
                .query_row("SELECT sealed FROM snapshot WHERE id=1", [], |r| r.get(0))?;
        let raw = crypto::unseal(&key, &blob, &self.aad("snapshot"))?;
        if user != self.username {
            bail!("Anmeldung fehlgeschlagen");
        }
        let payload: Payload = serde_json::from_slice(&raw)?;
        payload.state.validate()?;
        let last: u64 = self
            .conn
            .query_row("SELECT COALESCE(MAX(seq),0) FROM audit", [], |r| r.get(0))?;
        if payload.sequence != last {
            bail!("Journal und Zustand widersprechen sich");
        }
        if self.version >= 3 {
            self.verify_archive_manifest(payload.archive_count, &payload.archive_root)?;
        } else {
            self.verify_archive_full(&key, payload.archive_count, &payload.archive_root)?;
        }
        if self.version >= 3 {
            let (count, root): (u64, Vec<u8>) = self.conn.query_row(
                "SELECT node_count,root FROM graph_manifest WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if count != payload.graph_count || root != payload.graph_root {
                bail!("Graph-Manifest und Datensätze widersprechen sich");
            }
        }
        // Verify every committed delta, including missing/moved/corrupted entries.
        let baseline: Vec<u8> =
            self.conn
                .query_row("SELECT sealed FROM baseline WHERE id=1", [], |r| r.get(0))?;
        let mut replay: Value =
            serde_json::from_slice(&crypto::unseal(&key, &baseline, &self.aad("baseline"))?)?;
        let mut stmt = self
            .conn
            .prepare("SELECT seq,sealed FROM audit ORDER BY seq")?;
        let mut rows = stmt.query([])?;
        let mut expected = 1u64;
        while let Some(row) = rows.next()? {
            let seq: u64 = row.get(0)?;
            if seq != expected {
                bail!("Lücke im Journal");
            }
            let blob: Vec<u8> = row.get(1)?;
            let event: Value = serde_json::from_slice(&crypto::unseal(
                &key,
                &blob,
                &self.aad(&format!("audit:{seq}")),
            )?)?;
            let patch: json_patch::Patch = serde_json::from_value(event["patch"].clone())?;
            json_patch::patch(&mut replay, &patch)?;
            expected += 1;
        }
        if replay != serde_json::to_value(&payload.state)? {
            bail!("Journal-Replay stimmt nicht mit Zustand überein");
        }
        Ok((payload, key))
    }
    pub fn authenticated(&mut self, payload: &Payload, key: &Key) -> Result<()> {
        let blob = crypto::seal(
            key,
            &Zeroizing::new(serde_json::to_vec(payload)?),
            &self.aad("snapshot"),
        )?;
        let tx = self.conn.transaction()?;
        let shard_count: u64 =
            tx.query_row("SELECT COUNT(*) FROM graph_shards", [], |r| r.get(0))?;
        if shard_count == 0 {
            for (id, page) in seal_graph_pages(&payload.state, key, &self.username)? {
                tx.execute(
                    "INSERT INTO graph_shards(id,sealed) VALUES(?1,?2)",
                    params![id, page],
                )?;
            }
        }
        tx.execute("UPDATE snapshot SET sealed=?1 WHERE id=1", [blob])?;
        tx.execute(
            "UPDATE throttle SET failures=0,next_allowed=0 WHERE id=1",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub fn commit(
        &mut self,
        payload: &mut Payload,
        old: &State,
        graph_new: &GraphSlice,
        key: &Key,
        event: &str,
        episodes_to_append: &[Value],
        vermerk: &JournalVermerk,
    ) -> Result<()> {
        #[cfg(test)]
        {
            if self.crash_vor_commit {
                bail!("Simulierter Absturz vor dem Commit");
            }
        }
        if self.version < 3
            && old
                .neuronen
                .keys()
                .any(|id| !payload.state.neuronen.contains_key(id))
        {
            bail!("Neuronen dürfen nicht gelöscht werden");
        }
        let seq = payload
            .sequence
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Journal voll"))?;
        let previous = serde_json::to_value(state_projection(old, self.version))?;
        let next = serde_json::to_value(state_projection(&payload.state, self.version))?;
        let delta = json!({"event":event,"patch":json_patch::diff(&previous,&next)});
        // Confirming a write-ahead op-journal entry happens in the same
        // transaction as the state it describes; anything else is a crash
        // remnant that the next unlock discards.
        if let Some(bestaetigt) = vermerk.bestaetigt {
            payload.op_journal_seq = bestaetigt;
        }
        let journal = crypto::seal(
            key,
            &Zeroizing::new(serde_json::to_vec(&delta)?),
            &self.aad(&format!("audit:{seq}")),
        )?;
        payload.sequence = seq;
        let archive_domain = format!("saeugling-v{}:{}:episode:", self.version, self.username);
        let snapshot_aad = self.aad("snapshot");
        // Prepare dirty records before opening the transaction, so the bounded
        // read path does not borrow the connection while a write transaction is live.
        let mut graph_upserts = Vec::new();
        let mut graph_new_count = 0u64;
        let entfernte_lookups: Vec<Vec<u8>> = if self.version >= 3 {
            vermerk
                .graph_entfernungen
                .iter()
                .map(|id| graph_lookup(key, id))
                .collect()
        } else {
            Vec::new()
        };
        if self.version >= 3 {
            for (id, node) in &graph_new.nodes {
                // The update map is the authoritative post-commit state of the
                // record's outgoing edges (changed_graph always carries the
                // complete map), so a retracted pair — an undone edge creation
                // or a heal's edge rewrite — disappears from the vault too
                // instead of surviving a union with the stored map.
                let edges = match graph_new.edges.get(id) {
                    Some(updates) => updates.clone(),
                    None => self
                        .graph_nodes(key, std::slice::from_ref(id))?
                        .edges
                        .remove(id)
                        .unwrap_or_default(),
                };
                let record = GraphRecord {
                    node: node.clone(),
                    edges,
                };
                let sealed = crypto::seal(
                    key,
                    &Zeroizing::new(serde_json::to_vec(&record)?),
                    &graph_aad(&graph_lookup(key, id)),
                )?;
                let lookup = graph_lookup(key, id);
                let exists: bool = self.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM graph_records WHERE id_hash=?1)",
                    [lookup.as_slice()],
                    |r| r.get(0),
                )?;
                if !exists {
                    graph_new_count += 1;
                }
                graph_upserts.push((lookup, sealed));
            }
        }
        let next_graph_root = if self.version >= 3 {
            graph_mutation_root(key, &payload.graph_root, &graph_upserts, &entfernte_lookups)
        } else {
            Vec::new()
        };
        let tx = self.conn.transaction()?;
        tx.execute("INSERT INTO audit VALUES(?1,?2)", params![seq, journal])?;
        // Undo removes a journal suffix atomically with the state restoration.
        if let Some(streichen_ab) = vermerk.streichen_ab {
            tx.execute(
                "DELETE FROM op_journal WHERE seq >= ?1",
                params![streichen_ab],
            )?;
        }
        // Append only newly observed episodes as individually authenticated blocks.
        // Readers can fetch bounded pages without loading this archive into memory.
        {
            for episode in episodes_to_append {
                let archive_seq = payload.archive_count + 1;
                let aad = format!("{archive_domain}{archive_seq}").into_bytes();
                let block = crypto::seal(key, &Zeroizing::new(serde_json::to_vec(episode)?), &aad)?;
                payload.archive_root =
                    archive_next_root(&payload.archive_root, archive_seq, &block);
                payload.archive_count = archive_seq;
                tx.execute(
                    "INSERT INTO conversation_archive(seq,sealed) VALUES(?1,?2)",
                    params![archive_seq, block],
                )?;
            }
        }
        if self.version >= 3 {
            tx.execute(
                "UPDATE archive_manifest SET episode_count=?1,root=?2 WHERE id=1",
                params![payload.archive_count, payload.archive_root],
            )?;
        }
        if self.version >= 3 {
            for (lookup, sealed) in &graph_upserts {
                tx.execute("INSERT INTO graph_records(id_hash,sealed) VALUES(?1,?2) ON CONFLICT(id_hash) DO UPDATE SET sealed=excluded.sealed",params![lookup,sealed])?;
            }
            // Undo of a neuron creation removes its record; every edge into
            // it was already rolled back by the same journal suffix.
            let mut entfernt_count = 0u64;
            for lookup in &entfernte_lookups {
                entfernt_count += tx.execute(
                    "DELETE FROM graph_records WHERE id_hash=?1",
                    params![lookup],
                )? as u64;
            }
            payload.graph_count = payload
                .graph_count
                .saturating_add(graph_new_count)
                .saturating_sub(entfernt_count);
            payload.graph_root = next_graph_root.clone();
            tx.execute(
                "UPDATE graph_manifest SET node_count=?1,root=?2 WHERE id=1",
                params![payload.graph_count, payload.graph_root],
            )?;
            payload.state = state_projection(&payload.state, self.version);
        }
        let sealed = crypto::seal(
            key,
            &Zeroizing::new(serde_json::to_vec(payload)?),
            &snapshot_aad,
        )?;
        tx.execute("UPDATE snapshot SET sealed=?1 WHERE id=1", [sealed])?;
        let shards = if self.version < 3 {
            payload.state.graph_shards()
        } else {
            Vec::new()
        };
        for shard in shards {
            let id = shard.id;
            let aad = format!("saeugling-v2:{}:graph:{id}", self.username).into_bytes();
            let block = crypto::seal(key, &Zeroizing::new(serde_json::to_vec(&shard)?), &aad)?;
            tx.execute("INSERT INTO graph_shards(id,sealed) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET sealed=excluded.sealed", params![id, block])?;
        }
        for (lookup, sealed) in if self.version < 3 {
            changed_graph_records(old, &payload.state, key)?
        } else {
            Vec::new()
        } {
            tx.execute("INSERT INTO graph_records(id_hash,sealed) VALUES(?1,?2) ON CONFLICT(id_hash) DO UPDATE SET sealed=excluded.sealed", params![lookup,sealed])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn audit_len(&self) -> Result<u64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM audit", [], |r| r.get(0))?)
    }

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

    fn verify_archive_manifest(&self, expected_count: u64, expected_root: &[u8]) -> Result<()> {
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

fn seal_graph_pages(state: &State, key: &Key, username: &str) -> Result<Vec<(u8, Vec<u8>)>> {
    state
        .graph_shards()
        .into_iter()
        .map(|shard| {
            let aad = format!("saeugling-v2:{username}:graph:{}", shard.id).into_bytes();
            let sealed = crypto::seal(key, &Zeroizing::new(serde_json::to_vec(&shard)?), &aad)?;
            Ok((shard.id, sealed))
        })
        .collect()
}

fn graph_lookup(key: &Key, id: &str) -> Vec<u8> {
    keyed_lookup(key, b"saeugling-v3-graph-index", id)
}

fn checkpoint_lookup(key: &Key, id: &str) -> Vec<u8> {
    keyed_lookup(key, b"saeugling-v3-checkpoint-index", id)
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

fn graph_records_root(key: &Key, records: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};
    let mut records = records.to_vec();
    records.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = Hmac::<Sha256>::new_from_slice(key.as_ref()).expect("HMAC accepts any key");
    h.update(b"saeugling-v3-graph-root-initial");
    for (lookup, sealed) in &records {
        h.update(lookup);
        h.update(&Sha256::digest(sealed));
    }
    h.finalize().into_bytes().to_vec()
}

fn graph_mutation_root(
    key: &Key,
    previous: &[u8],
    records: &[(Vec<u8>, Vec<u8>)],
    entfernte: &[Vec<u8>],
) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};
    if records.is_empty() && entfernte.is_empty() {
        return previous.to_vec();
    }
    let mut records = records.to_vec();
    records.sort_by(|a, b| a.0.cmp(&b.0));
    let mut entfernt = entfernte.to_vec();
    entfernt.sort();
    let mut h = Hmac::<Sha256>::new_from_slice(key.as_ref()).expect("HMAC accepts any key");
    h.update(b"saeugling-v3-graph-root-update");
    h.update(previous);
    for (lookup, sealed) in records {
        h.update(&lookup);
        h.update(&Sha256::digest(sealed));
    }
    for lookup in entfernt {
        h.update(b"removed:");
        h.update(&lookup);
    }
    h.finalize().into_bytes().to_vec()
}

fn state_projection(state: &State, version: u64) -> State {
    if version < 3 {
        return state.clone();
    }
    State {
        neuronen: BTreeMap::new(),
        kanten: BTreeMap::new(),
        konfig: state.konfig.clone(),
        interaktionen: state.interaktionen,
        letzte_slaap: state.letzte_slaap,
        extra: state.extra.clone(),
    }
}

fn hex_prefix(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn seal_graph_records(state: &State, key: &Key) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    state
        .neuronen
        .iter()
        .map(|(id, node)| {
            let record = GraphRecord {
                node: node.clone(),
                edges: state.kanten.get(id).cloned().unwrap_or_default(),
            };
            let sealed = crypto::seal(
                key,
                &Zeroizing::new(serde_json::to_vec(&record)?),
                &graph_aad(&graph_lookup(key, id)),
            )?;
            Ok((graph_lookup(key, id), sealed))
        })
        .collect()
}

fn changed_graph_records(old: &State, new: &State, key: &Key) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut changed = Vec::new();
    for (id, node) in &new.neuronen {
        let edges = new.kanten.get(id).cloned().unwrap_or_default();
        if old.neuronen.get(id).is_some_and(|previous| {
            previous == node && old.kanten.get(id).cloned().unwrap_or_default() == edges
        }) {
            continue;
        }
        let record = GraphRecord {
            node: node.clone(),
            edges,
        };
        let sealed = crypto::seal(
            key,
            &Zeroizing::new(serde_json::to_vec(&record)?),
            &graph_aad(&graph_lookup(key, id)),
        )?;
        changed.push((graph_lookup(key, id), sealed));
    }
    Ok(changed)
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
    use super::*;
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
