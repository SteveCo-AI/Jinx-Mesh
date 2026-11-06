//! Vault creation and authentication: create/open with the process lock,
//! throttled unlock with full audit replay, the confirming commit
//! transaction, and the format-2/3 record sealing helpers.
use super::{archive_next_root, graph_aad, graph_lookup, GraphRecord, Import, Payload, Store};
use crate::{
    crypto::{self, Key},
    journal::JournalVermerk,
    model::{GraphSlice, State},
};
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use rusqlite::{params, Connection, OpenFlags};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Read,
    path::Path,
};
use zeroize::Zeroizing;

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
