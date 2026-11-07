//! Adaptive tutoring bridge for the Saugling: one Unix-socket protocol in
//! front of THE ONLY App session on a training vault copy.
//!
//! Usage: `training_bridge SOCKET VAULT [USER]` (USER defaults to "Lumen").
//! The socket binds only if the path is free (an existing file is never
//! unlinked) and is set to mode 0600. Login is interactive at start — hidden
//! password, optional hidden 2FA prompt, empty code means no 2FA (the same
//! pattern as src/main.rs:201-212) — and the single line printed to stdout is
//! `READY`. One client is served at a time; every request line gets exactly
//! one response line of JSON.
//!
//! Protocol (one JSON request per line, one JSON response per request):
//! * `{"id","op":"train","text"}` — `app.message(text)`: trains and issues a
//!   reply id. The reply is rateable only while the engine reports
//!   `feedback_available`.
//! * `{"id","op":"read_only","text"}` — `app.infer_read_only(text)`: the
//!   interactions counter must not change; read-only answers never get a
//!   reply id of their own and never become rateable targets (see the
//!   engine finding in [`Bridge::op_read_only`]).
//! * `{"id","op":"rate","target_reply_id","value":"plus"|"minus"}` —
//!   exactly-once rating of the last rateable reply, sent to the engine as
//!   a message whose text is exactly "+" or "-". The interactions counter
//!   grows by exactly one (a rating is a learning-adjacent turn, Kap. 18);
//!   that is correct and expected.
//! * `{"id","op":"correction","target_reply_id","value":"plus"|"minus",
//!   "text"}` — Raphael correction form: `text` is the CORRECTED answer.
//!   Exactly-once and staleness are checked like the rate op (one rated set
//!   covers both ops), then a text that itself starts with +/- is refused
//!   ("prefixed_text"). The turn is one `app.message()` whose text is the
//!   rating prefix + the corrected answer (e.g. `"+regenbogen"`): the engine
//!   rates the previous reply in its own context AND learns the text as a
//!   normal training turn — the native split_feedback path
//!   (src/language.rs:57-66). The response carries the turn's own new
//!   `reply_id` (which becomes the next rateable target when the engine
//!   reports `feedback_available`), the reply `text`, and the interactions
//!   count.
//! * `{"id","op":"status"}` — `app.status()`.
//! * `{"id","op":"consolidate"}` — `app.consolidate()`: one consolidation
//!   round; `result` is the engine's own JSON summary. On Err the engine
//!   has already restored its exact pre-round state (the Kap.-24 gate
//!   reject persists a rejection marker and bails,
//!   src/app/message.rs:383-401), so the error is passed through and a
//!   retry is legitimate.
//! * `{"id","op":"checkpoint","path"}` — refused: App exposes no
//!   consistent-snapshot API; checkpointing happens exclusively by clean
//!   shutdown (see [`Bridge::op_checkpoint`]).
//! * `{"id","op":"shutdown"}` — responds ok, the App is dropped (flush and
//!   close), the socket file is removed, the process exits 0.
//! * Any other op or malformed line answers
//!   `{"id": <id or 0>, "ok": false, "error": "..."}`.
//!
//! No request can run a shell command: the socket only reaches the App
//! engine calls above, `message`/`infer_read_only` learn lexical tokens
//! only (src/graph.rs:386), and the engine itself refuses '/' command texts
//! (src/engine.rs:331-334).
//!
//! Every train/rate/correction/read_only request+result is appended to
//! `<VAULT>.events.jsonl` at occurrence, one flushed line per event.
use anyhow::{Context, Result};
use saeugling::{App, Mode, SessionPolicy};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

/// A request line beyond this cap is rejected and the client connection is
/// dropped (the byte stream cannot be resynced to line boundaries afterwards).
/// The engine caps a message at 16384 bytes (src/engine.rs:327-335); the cap
/// leaves room for the JSON envelope.
const MAX_REQUEST_BYTES: usize = 64 * 1024;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn error_response(id: u64, error: &str) -> Value {
    json!({"id": id, "ok": false, "error": error})
}

/// The append-only event log next to the vault: `<vault>.events.jsonl`.
struct EventLog {
    file: File,
}

impl EventLog {
    fn open(vault: &Path) -> Result<Self> {
        let path = events_path(vault);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("events log {}", path.display()))?;
        Ok(Self { file })
    }

    /// One line per event, flushed at occurrence.
    fn append(&mut self, event: &Value) -> io::Result<()> {
        let mut line = serde_json::to_string(event).map_err(io::Error::other)?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.flush()
    }
}

fn events_path(vault: &Path) -> PathBuf {
    PathBuf::from(format!("{}.events.jsonl", vault.display()))
}

/// One read line from the socket: bounded, newline-terminated.
#[derive(Debug)]
enum LineIn {
    Eof,
    Line(String),
    TooLarge,
}

fn read_request(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> io::Result<LineIn> {
    buf.clear();
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Ok(if buf.is_empty() {
                LineIn::Eof
            } else {
                LineIn::Line(String::from_utf8_lossy(buf).into_owned())
            });
        }
        let end = chunk
            .iter()
            .position(|&b| b == b'\n')
            .map_or(chunk.len(), |i| i + 1);
        if buf.len() + end > MAX_REQUEST_BYTES {
            return Ok(LineIn::TooLarge);
        }
        buf.extend_from_slice(&chunk[..end]);
        reader.consume(end);
        if buf.last() == Some(&b'\n') {
            buf.pop();
            return Ok(LineIn::Line(String::from_utf8_lossy(buf).into_owned()));
        }
    }
}

/// The bridge state: the sole App session, its own reply/rating bookkeeping,
/// and the credentials needed for the unattended re-login paths.
struct Bridge {
    app: App,
    user: String,
    password: Zeroizing<String>,
    /// Ids of train replies; read-only answers never consume one.
    reply_seq: u64,
    /// The reply id the engine would rate next, if any.
    rateable: Option<u64>,
    rated: HashSet<u64>,
    events: EventLog,
    /// Set after a failed re-login; no later request retries (each failed
    /// attempt would grow the vault's login throttle,
    /// src/store/access.rs:242-245).
    relogin_failed: bool,
}

impl Bridge {
    fn new(app: App, user: String, password: Zeroizing<String>, events: EventLog) -> Self {
        Self {
            app,
            user,
            password,
            reply_seq: 0,
            rateable: None,
            rated: HashSet::new(),
            events,
            relogin_failed: false,
        }
    }

    /// One request line in, one response value out plus the shutdown flag.
    fn handle(&mut self, line: &str) -> (Value, bool) {
        let line = line.trim();
        if line.is_empty() {
            return (error_response(0, "malformed_request: empty line"), false);
        }
        let request: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => return (error_response(0, &format!("malformed_request: {e}")), false),
        };
        let id = request.get("id").and_then(Value::as_u64).unwrap_or(0);
        let Some(op) = request.get("op").and_then(Value::as_str) else {
            return (
                error_response(id, "malformed_request: missing string field \"op\""),
                false,
            );
        };
        match op {
            "train" => (self.op_train(id, &request), false),
            "read_only" => (self.op_read_only(id, &request), false),
            "rate" => (self.op_rate(id, &request), false),
            "correction" => (self.op_correction(id, &request), false),
            "status" => (self.op_status(id), false),
            "consolidate" => (self.op_consolidate(id), false),
            "checkpoint" => (self.op_checkpoint(id, &request), false),
            // Shutdown answers first; serve() then returns and main drops the
            // App (flush/close), removes the socket file, exits 0.
            "shutdown" => (json!({"id": id, "ok": true}), true),
            other => (error_response(id, &format!("unknown_op: {other}")), false),
        }
    }

    /// Unattended re-login after idle expiry or a fail-closed session death
    /// (src/app/message.rs:221-224). One attempt only: a failure marks the
    /// bridge dead so no later request burns another throttle slot. 2FA
    /// vaults cannot be re-logged-in unattended (each login consumes one
    /// TOTP step, src/app/auth.rs:60-78) and fail here honestly.
    fn ensure_session(&mut self) -> Result<(), String> {
        if self.app.mode() != Mode::Locked {
            return Ok(());
        }
        if self.relogin_failed {
            return Err("session_locked: an earlier re-login failed — restart the bridge".into());
        }
        match self.app.login(&self.user, &self.password, None) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.relogin_failed = true;
                Err(format!(
                    "session_locked: re-login failed ({e}) — restart the bridge"
                ))
            }
        }
    }

    fn current_status(&mut self) -> Result<Value, String> {
        self.app.status().map_err(|e| format!("status_failed: {e}"))
    }

    /// Best-effort interactions read for rejection responses; never triggers
    /// a re-login (a rejection must have no side effects, and a failed login
    /// attempt would grow the vault's throttle).
    fn session_interactions(&mut self) -> Option<u64> {
        if self.app.mode() == Mode::Locked {
            return None;
        }
        self.app.status().ok()?["interactions"].as_u64()
    }

    /// Append the request+result event (append-only, flushed per line) and
    /// surface an append failure as an extra response field — the engine
    /// effect the response reports already happened, so ok never flips here.
    fn log(
        &mut self,
        response: &mut Value,
        req_id: u64,
        op: &str,
        args: Value,
        reply_id: Option<u64>,
    ) {
        let event = json!({
            "ts": now_secs(),
            "req_id": req_id,
            "op": op,
            "args": args,
            "reply_id": reply_id,
            "ok": response["ok"].as_bool().unwrap_or(false),
            "error": response["error"].as_str(),
            "interactions": response["interactions"].as_u64(),
        });
        if let Err(e) = self.events.append(&event) {
            response["event_log_error"] = json!(format!("events append failed: {e}"));
        }
    }

    /// Uniform failure path: error response with a best-effort interactions
    /// field, logged as the op's request+result.
    fn fail_response(
        &mut self,
        id: u64,
        op: &str,
        args: &Value,
        reply_id: Option<u64>,
        error: String,
    ) -> Value {
        let mut response = error_response(id, &error);
        response["interactions"] = self
            .session_interactions()
            .map(Value::from)
            .unwrap_or(Value::Null);
        self.log(&mut response, id, op, args.clone(), reply_id);
        response
    }

    fn op_train(&mut self, id: u64, request: &Value) -> Value {
        let args = json!({"text": request.get("text").cloned().unwrap_or(Value::Null)});
        let Some(text) = request.get("text").and_then(Value::as_str) else {
            return self.fail_response(
                id,
                "train",
                &args,
                None,
                "malformed_request: train needs a string field \"text\"".into(),
            );
        };
        // Ratings are the rate op's exclusive job. A train text that
        // split_feedback reads as a +/- rating (src/language.rs:57-66) would
        // consume the rateable target inside the engine, outside this
        // bridge's exactly-once accounting — refuse it instead.
        let stripped = text.trim_start();
        if stripped.starts_with('+') || stripped.starts_with('-') {
            return self.fail_response(
                id,
                "train",
                &args,
                None,
                "train_text_is_rating: use the rate op to rate a reply".into(),
            );
        }
        if let Err(e) = self.ensure_session() {
            return self.fail_response(id, "train", &args, None, e);
        }
        // A failed train produces no reply, so no reply id is consumed.
        let reply = match self.app.message(text) {
            Ok(r) => r,
            Err(e) => {
                return self.fail_response(id, "train", &args, None, format!("train_failed: {e}"))
            }
        };
        self.reply_seq += 1;
        let reply_id = self.reply_seq;
        let status = match self.current_status() {
            Ok(s) => s,
            Err(e) => {
                return self.fail_response(
                    id,
                    "train",
                    &args,
                    Some(reply_id),
                    format!("train_committed_but_status_failed: {e}"),
                )
            }
        };
        // A train reply with a non-empty text is the engine's rateable target
        // (src/language.rs:184-188); an empty or fallback reply is not
        // (src/engine.rs:233-244).
        self.rateable = status["feedback_available"]
            .as_bool()
            .unwrap_or(false)
            .then_some(reply_id);
        let mut response = json!({
            "id": id,
            "ok": true,
            "reply_id": reply_id,
            "text": reply.text,
            "art": reply.art,
            "feedback": reply.feedback,
            "neue_neuronen": reply.neue_neuronen,
            "interactions": status["interactions"].clone(),
            "status": status,
        });
        self.log(&mut response, id, "train", args, Some(reply_id));
        response
    }

    fn op_read_only(&mut self, id: u64, request: &Value) -> Value {
        let args = json!({"text": request.get("text").cloned().unwrap_or(Value::Null)});
        let Some(text) = request.get("text").and_then(Value::as_str) else {
            return self.fail_response(
                id,
                "read_only",
                &args,
                None,
                "malformed_request: read_only needs a string field \"text\"".into(),
            );
        };
        if let Err(e) = self.ensure_session() {
            return self.fail_response(id, "read_only", &args, None, e);
        }
        let before = match self.current_status() {
            Ok(s) => s,
            Err(e) => return self.fail_response(id, "read_only", &args, None, e),
        };
        let interactions_before = before["interactions"].clone();
        let available_before = before["feedback_available"].as_bool().unwrap_or(false);
        let reply = match self.app.infer_read_only(text) {
            Ok(r) => r,
            Err(e) => {
                return self.fail_response(
                    id,
                    "read_only",
                    &args,
                    None,
                    format!("read_only_failed: {e}"),
                )
            }
        };
        let after = match self.current_status() {
            Ok(s) => s,
            Err(e) => return self.fail_response(id, "read_only", &args, None, e),
        };
        if after["interactions"] != interactions_before {
            // infer_read_only never trains (src/app/message.rs:24-67), so this
            // guard cannot fire under the current engine; it refuses honestly
            // if that ever changes.
            let mut response = error_response(id, "state_changed_during_read");
            response["interactions"] = after["interactions"].clone();
            self.log(&mut response, id, "read_only", args, None);
            return response;
        }
        // VERIFIED ENGINE FINDING (checked per the bridge contract): contrary
        // to the assumption that infer_read_only "only updates the session
        // target" without making it rateable, src/app/message.rs:61-65 DOES
        // set a rateable target — the session's feedback_target is overwritten
        // with language::target(text, &r.text), or cleared to None for an
        // empty reply. The persisted extra["feedback_target"] is NOT touched
        // (read-only commits nothing), but App::message copies the session
        // field into the engine state before the next training turn
        // (src/app/message.rs:124-131), so an unprotected bridge would rate
        // the read-only answer on the next rate op — or, after an empty
        // read-only reply, lose the real target entirely.
        //
        // The task's literal "snapshot the feedback_target extra value before
        // and restore it after" is impossible from an example: Session and
        // its feedback_target field are private (src/lib.rs exports only
        // App/Mode/RecallResult/SessionPolicy/Reply/Import) and status()
        // exposes only the feedback_available boolean, never the target
        // value. The one public restore path is logout + login: login
        // reloads the session's rateable target from the persisted extra
        // (src/app/auth.rs:112), which the read-only turn never changed.
        //
        // Costs, accepted deliberately: one argon2 unlock per protected
        // read-only answer, the password kept for the bridge's lifetime (in
        // a Zeroizing, zeroed at exit), and no unattended restore on 2FA
        // vaults (every login consumes a TOTP step, src/app/auth.rs:60-78) —
        // those fail honestly below instead of corrupting the rateable
        // target.
        //
        // The clobber can only matter when a target was pending before the
        // read or the read-only reply was non-empty; an empty reply over a
        // target-less session is the one provably safe case (None stays
        // None), which keeps early-training drills free of re-login cost.
        let mut status = after;
        if available_before || !reply.text.is_empty() {
            if let Err(e) = self.restore_rateable_target(available_before) {
                return self.fail_response(id, "read_only", &args, None, e);
            }
            status = match self.current_status() {
                Ok(s) => s,
                Err(e) => return self.fail_response(id, "read_only", &args, None, e),
            };
        }
        let mut response = json!({
            "id": id,
            "ok": true,
            // Read-only answers never get a reply id of their own and never
            // become rateable targets: this is the last train reply's id.
            "reply_id": self.reply_seq,
            "text": reply.text,
            "art": reply.art,
            "feedback": reply.feedback,
            "interactions": status["interactions"].clone(),
            "status": status,
        });
        self.log(&mut response, id, "read_only", args, Some(self.reply_seq));
        response
    }

    /// The read-only target protection (details in op_read_only's comment):
    /// login() reloads the session's rateable target from the persisted
    /// extra (src/app/auth.rs:112) — the state the read-only turn never
    /// committed. login() fail-closes any previous session itself
    /// (src/app/auth.rs:54-56).
    fn restore_rateable_target(&mut self, expected_available: bool) -> Result<(), String> {
        match self.app.login(&self.user, &self.password, None) {
            Ok(()) => {}
            Err(e) => {
                self.relogin_failed = true;
                return Err(format!(
                    "target_restore_failed: re-login failed ({e}); the session is locked — restart the bridge"
                ));
            }
        }
        let status = self
            .app
            .status()
            .map_err(|e| format!("target_restore_failed: status read failed ({e})"))?;
        let restored = status["feedback_available"].as_bool().unwrap_or(false);
        if restored != expected_available {
            self.relogin_failed = true;
            return Err(format!(
                "target_restore_failed: feedback_available is {restored} after the restore, expected {expected_available}"
            ));
        }
        Ok(())
    }

    fn op_rate(&mut self, id: u64, request: &Value) -> Value {
        let args = json!({
            "target_reply_id": request.get("target_reply_id").cloned().unwrap_or(Value::Null),
            "value": request.get("value").cloned().unwrap_or(Value::Null),
        });
        let Some(target) = request.get("target_reply_id").and_then(Value::as_u64) else {
            return self.fail_response(
                id,
                "rate",
                &args,
                None,
                "malformed_request: rate needs an integer field \"target_reply_id\"".into(),
            );
        };
        let value = match request.get("value").and_then(Value::as_str) {
            Some(v) => v,
            None => return self.fail_response(
                id,
                "rate",
                &args,
                Some(target),
                "malformed_request: rate needs a string field \"value\" (\"plus\" or \"minus\")"
                    .into(),
            ),
        };
        // src/language.rs:57-66: a leading '+' parses as Some(+1), a leading
        // '-' as Some(-1); the empty remainder makes the turn a pure rating.
        let rating = match value {
            "plus" => "+",
            "minus" => "-",
            other => {
                return self.fail_response(
                    id,
                    "rate",
                    &args,
                    Some(target),
                    format!("invalid_value: expected \"plus\" or \"minus\", got {other:?}"),
                )
            }
        };
        // Exactly-once, checked before any engine call: a rejection has no
        // side effects at all.
        if self.rated.contains(&target) {
            return self.fail_response(id, "rate", &args, Some(target), "already_rated".into());
        }
        if self.rateable != Some(target) {
            return self.fail_response(id, "rate", &args, Some(target), "stale_target".into());
        }
        if let Err(e) = self.ensure_session() {
            return self.fail_response(id, "rate", &args, Some(target), e);
        }
        match self.app.message(rating) {
            Ok(reply) => {
                let status = match self.current_status() {
                    Ok(s) => s,
                    Err(e) => {
                        return self.fail_response(
                            id,
                            "rate",
                            &args,
                            Some(target),
                            format!("rating_committed_but_status_failed: {e}"),
                        )
                    }
                };
                // Kap. 18: the rating consumed the stored target
                // (src/language.rs:246-258), and a bare "+"/"-" turn has an
                // empty reply, so no new target was stored either
                // (src/engine.rs:242-244). The bridge conservatively maps no
                // reply as rateable now; a future rate answers stale_target.
                self.rateable = None;
                self.rated.insert(target);
                let mut response = json!({
                    "id": id,
                    "ok": true,
                    "rated_reply_id": target,
                    "value": value,
                    "feedback": reply.feedback,
                    "text": reply.text,
                    "interactions": status["interactions"].clone(),
                    "status": status,
                });
                self.log(&mut response, id, "rate", args, Some(target));
                response
            }
            // The engine restored its state exactly (gate reject or fail
            // closed, src/app/message.rs:189-224): the id stays rateable and
            // unrated, so a retry is legitimate.
            Err(e) => {
                self.fail_response(id, "rate", &args, Some(target), format!("rate_failed: {e}"))
            }
        }
    }

    /// Raphael correction form: `text` is the CORRECTED answer. One engine
    /// message whose text is the rating prefix + the corrected answer makes
    /// the engine rate the previous reply in its own context AND learn the
    /// text as a normal training turn (the native split_feedback path,
    /// src/language.rs:57-66). Exactly-once is the rate op's: one rated set
    /// covers both ops, so a target rated by either cannot be rated again.
    fn op_correction(&mut self, id: u64, request: &Value) -> Value {
        let args = json!({
            "target_reply_id": request.get("target_reply_id").cloned().unwrap_or(Value::Null),
            "value": request.get("value").cloned().unwrap_or(Value::Null),
            "text": request.get("text").cloned().unwrap_or(Value::Null),
        });
        let Some(target) = request.get("target_reply_id").and_then(Value::as_u64) else {
            return self.fail_response(
                id,
                "correction",
                &args,
                None,
                "malformed_request: correction needs an integer field \"target_reply_id\"".into(),
            );
        };
        let value = match request.get("value").and_then(Value::as_str) {
            Some(v) => v,
            None => return self.fail_response(
                id,
                "correction",
                &args,
                Some(target),
                "malformed_request: correction needs a string field \"value\" (\"plus\" or \"minus\")"
                    .into(),
            ),
        };
        let rating = match value {
            "plus" => "+",
            "minus" => "-",
            other => {
                return self.fail_response(
                    id,
                    "correction",
                    &args,
                    Some(target),
                    format!("invalid_value: expected \"plus\" or \"minus\", got {other:?}"),
                )
            }
        };
        let Some(text) = request.get("text").and_then(Value::as_str) else {
            return self.fail_response(
                id,
                "correction",
                &args,
                Some(target),
                "malformed_request: correction needs a string field \"text\" (the corrected answer)"
                    .into(),
            );
        };
        // Exactly-once and staleness first, checked exactly like the rate op:
        // a rejection has no side effects at all.
        if self.rated.contains(&target) {
            return self.fail_response(
                id,
                "correction",
                &args,
                Some(target),
                "already_rated".into(),
            );
        }
        if self.rateable != Some(target) {
            return self.fail_response(
                id,
                "correction",
                &args,
                Some(target),
                "stale_target".into(),
            );
        }
        // The corrected answer must not carry its own rating prefix: the
        // engine splits the combined message once and trims the remainder
        // (src/language.rs:57-66), so a leading +/- would survive into the
        // learned text and blur the correction form's meaning.
        let stripped = text.trim_start();
        if stripped.starts_with('+') || stripped.starts_with('-') {
            return self.fail_response(
                id,
                "correction",
                &args,
                Some(target),
                "prefixed_text".into(),
            );
        }
        if let Err(e) = self.ensure_session() {
            return self.fail_response(id, "correction", &args, Some(target), e);
        }
        let message = format!("{rating}{text}");
        match self.app.message(&message) {
            Ok(reply) => {
                self.reply_seq += 1;
                let reply_id = self.reply_seq;
                // The engine consumed the stored target with this move (the
                // rating half, src/language.rs:246-258), so the id counts as
                // rated from here on even if the status read below fails —
                // a retry would otherwise rate whatever new target the
                // turn stored.
                self.rated.insert(target);
                // The corrected-answer turn usually leaves its own reply as
                // the engine's new rateable target; until the status read
                // confirms it, the bridge conservatively maps no reply as
                // rateable (a future rate answers stale_target).
                self.rateable = None;
                let status = match self.current_status() {
                    Ok(s) => s,
                    Err(e) => {
                        return self.fail_response(
                            id,
                            "correction",
                            &args,
                            Some(reply_id),
                            format!("correction_committed_but_status_failed: {e}"),
                        )
                    }
                };
                self.rateable = status["feedback_available"]
                    .as_bool()
                    .unwrap_or(false)
                    .then_some(reply_id);
                let mut response = json!({
                    "id": id,
                    "reply_id": reply_id,
                    "ok": true,
                    "text": reply.text,
                    "interactions": status["interactions"].clone(),
                });
                self.log(&mut response, id, "correction", args, Some(reply_id));
                response
            }
            // The engine restored its state exactly (gate reject or fail
            // closed, src/app/message.rs:189-224): the id stays rateable and
            // unrated, so a retry is legitimate.
            Err(e) => self.fail_response(
                id,
                "correction",
                &args,
                Some(target),
                format!("correction_failed: {e}"),
            ),
        }
    }

    fn op_status(&mut self, id: u64) -> Value {
        if let Err(e) = self.ensure_session() {
            return error_response(id, &e);
        }
        match self.current_status() {
            Ok(status) => json!({"id": id, "ok": true, "status": status}),
            Err(e) => error_response(id, &e),
        }
    }

    fn op_consolidate(&mut self, id: u64) -> Value {
        if let Err(e) = self.ensure_session() {
            return error_response(id, &e);
        }
        match self.app.consolidate() {
            Ok(result) => json!({"id": id, "ok": true, "result": result}),
            Err(e) => error_response(id, &format!("consolidate_failed: {e}")),
        }
    }

    fn op_checkpoint(&mut self, id: u64, request: &Value) -> Value {
        // "path" is accepted but deliberately unused: no copy is ever made.
        let _path = request.get("path");
        if let Err(e) = self.ensure_session() {
            return error_response(id, &e);
        }
        // tick() + status() per the checkpoint contract; tick() only expires
        // an idle session, it flushes nothing — see the consistency note.
        let status = match self.current_status() {
            Ok(s) => s,
            Err(e) => return error_response(id, &e),
        };
        // Honest consistency statement: App owns the vault's sqlite
        // connection (journal_mode=DELETE, synchronous=FULL,
        // src/store/access.rs:105) and every learning move is one durable
        // single transaction, so no transaction is open between requests. But
        // the connection stays open for the App's lifetime and App exposes
        // none of sqlite's safe-copy routes (backup API, VACUUM INTO); an
        // OS-level file copy while the database is open is not a
        // guaranteed-consistent snapshot. The bridge therefore refuses to
        // guess and checkpoints exclusively by clean shutdown:
        // {"op":"shutdown"} responds, drops App (closing the connection),
        // removes the socket, exits 0 — only then is the vault file safe to
        // copy.
        json!({
            "id": id,
            "ok": false,
            "error": "no_consistent_checkpoint_api",
            "hint": "checkpoint by clean shutdown: send {\"op\":\"shutdown\"}, then copy the vault file after the process exits",
            "interactions": status["interactions"].clone(),
            "status": status,
        })
    }
}

/// Serve clients sequentially, one at a time, until a shutdown op answers.
fn serve(listener: &UnixListener, bridge: &mut Bridge) {
    loop {
        let client = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(_) => continue,
        };
        match handle_client(client, bridge) {
            Ok(true) => return,
            Ok(false) => {}
            Err(e) => eprintln!("client connection error: {e}"),
        }
    }
}

/// One client: every request line gets exactly one response line, in order.
fn handle_client(stream: UnixStream, bridge: &mut Bridge) -> Result<bool> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        match read_request(&mut reader, &mut buf)? {
            LineIn::Eof => return Ok(false),
            LineIn::TooLarge => {
                write_response(&mut writer, &error_response(0, "request_too_large"))?;
                return Ok(false);
            }
            LineIn::Line(line) => {
                let (response, shutdown) = bridge.handle(&line);
                write_response(&mut writer, &response)?;
                if shutdown {
                    return Ok(true);
                }
            }
        }
    }
}

fn write_response(writer: &mut UnixStream, response: &Value) -> Result<()> {
    let mut line = serde_json::to_string(response)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()?;
    Ok(())
}

/// Bind the socket (failing if the path exists — foreign files are never
/// unlinked) and restrict it to the owner.
fn bind_socket(path: &Path) -> Result<UnixListener> {
    let listener = UnixListener::bind(path)
        .with_context(|| format!("bind socket {} (path already in use?)", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod 0600 socket {}", path.display()))?;
    Ok(listener)
}

/// The existing interactive pattern (src/main.rs:201-212): hidden password,
/// hidden optional 2FA prompt, empty code = None. Returns the password
/// because the bridge keeps it for the unattended re-login paths.
fn login_interactive(app: &mut App, user: &str) -> Result<Zeroizing<String>> {
    let password = Zeroizing::new(rpassword::prompt_password("Passwort (verdeckt): ")?);
    let code = Zeroizing::new(rpassword::prompt_password(
        "2FA-Code (leer falls deaktiviert): ",
    )?);
    app.login(user, &password, (!code.is_empty()).then_some(code.as_str()))
        .context("interactive login failed")?;
    // A TOTP code is single-step (each login consumes one,
    // src/app/auth.rs:60-78); keep only the password for re-login.
    Ok(password)
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Fehler: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: training_bridge SOCKET VAULT [USER]";
    let socket_path = PathBuf::from(args.next().context(usage)?);
    let vault_path = PathBuf::from(args.next().context(usage)?);
    let user = args.next().unwrap_or_else(|| "Lumen".into());
    let app =
        App::open(&vault_path).with_context(|| format!("open vault {}", vault_path.display()))?;
    let listener = bind_socket(&socket_path)?;
    let served = run_session(app, &listener, &user, &vault_path);
    // The bridge always removes its own socket file, success or failure.
    let _ = std::fs::remove_file(&socket_path);
    served
}

fn run_session(app: App, listener: &UnixListener, user: &str, vault: &Path) -> Result<()> {
    // A tutoring cadence can pause for hours between requests; the default
    // 30-minute idle expiry (src/app/auth.rs:20-27) would lock the session
    // between tutor turns. The bridge is the vault's only session owner, and
    // the unattended re-login safety net covers the remaining cases.
    let mut app = app.with_policy(SessionPolicy {
        idle: Duration::from_secs(24 * 3600),
        maximum: Duration::from_secs(7 * 24 * 3600),
    });
    let password = login_interactive(&mut app, user)?;
    let events = EventLog::open(vault)?;
    println!("READY");
    io::stdout().flush()?;
    let mut bridge = Bridge::new(app, user.to_owned(), password, events);
    serve(listener, &mut bridge);
    // Shutdown flush: App::drop closes the sqlite connection cleanly; every
    // engine commit was already a durable transaction (synchronous=FULL),
    // so nothing is pending at this point.
    drop(bridge);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Bridge contract tests need a vault: App::create_empty is public
    /// (src/app/mod.rs:98) and needs no fixture import, so no test is
    /// skipped. The interactive login is replaced by the direct engine call
    /// with the known password; the hidden prompts are not exercised here.
    const PASS: &str = "bridge contract test passphrase 161803!";

    fn bridge(dir: &TempDir) -> Bridge {
        let vault = dir.path().join("training.db");
        let app = App::create_empty(&vault, "Lumen", PASS, None, None).unwrap();
        let events = EventLog::open(&vault).unwrap();
        let mut bridge = Bridge::new(app, "Lumen".into(), Zeroizing::new(PASS.into()), events);
        bridge.app.login("Lumen", PASS, None).unwrap();
        bridge
    }

    fn call(bridge: &mut Bridge, request: Value) -> Value {
        let line = serde_json::to_string(&request).unwrap();
        bridge.handle(&line).0
    }

    fn train(bridge: &mut Bridge, id: u64, text: &str) -> Value {
        let response = call(bridge, json!({"id": id, "op": "train", "text": text}));
        assert!(
            response["ok"].as_bool().unwrap(),
            "train failed: {response}"
        );
        response
    }

    #[test]
    fn rate_is_exactly_once_a_second_rate_has_no_engine_side_effects() {
        let dir = TempDir::new().unwrap();
        let mut bridge = bridge(&dir);
        let trained = train(&mut bridge, 1, "hallo welt");
        let reply_id = trained["reply_id"].as_u64().unwrap();
        assert!(
            trained["status"]["feedback_available"]
                .as_bool()
                .unwrap_or(false),
            "a fresh vault's first train must mirror and become rateable: {trained}"
        );
        let first = call(
            &mut bridge,
            json!({"id": 2, "op": "rate", "target_reply_id": reply_id, "value": "plus"}),
        );
        assert!(first["ok"].as_bool().unwrap(), "first rate failed: {first}");
        // A rating is a learning-adjacent turn: exactly one interaction more.
        assert_eq!(
            first["interactions"].as_u64().unwrap(),
            trained["interactions"].as_u64().unwrap() + 1
        );
        let after_first = first["interactions"].as_u64().unwrap();
        let second = call(
            &mut bridge,
            json!({"id": 3, "op": "rate", "target_reply_id": reply_id, "value": "plus"}),
        );
        assert_eq!(second["ok"].as_bool(), Some(false));
        assert_eq!(second["error"].as_str().unwrap(), "already_rated");
        // No second rating message was sent: the counter did not move.
        assert_eq!(second["interactions"].as_u64(), Some(after_first));
        let status = call(&mut bridge, json!({"id": 4, "op": "status"}));
        assert_eq!(
            status["status"]["feedback_counts"]["positive"].as_u64(),
            Some(1),
            "exactly one rating reached the engine: {status}"
        );
    }

    #[test]
    fn rate_with_stale_target_reply_id_is_rejected_without_side_effects() {
        let dir = TempDir::new().unwrap();
        let mut bridge = bridge(&dir);
        let first = train(&mut bridge, 1, "eins zwei");
        let second = train(&mut bridge, 2, "drei vier");
        let stale_id = first["reply_id"].as_u64().unwrap();
        let current_id = second["reply_id"].as_u64().unwrap();
        let before = second["interactions"].as_u64().unwrap();
        let rejected = call(
            &mut bridge,
            json!({"id": 3, "op": "rate", "target_reply_id": stale_id, "value": "plus"}),
        );
        assert_eq!(rejected["ok"].as_bool(), Some(false));
        assert_eq!(rejected["error"].as_str().unwrap(), "stale_target");
        // No engine side effect: the counter did not move ...
        assert_eq!(rejected["interactions"].as_u64(), Some(before));
        // ... and the CURRENT target is still rateable after the rejection.
        let accepted = call(
            &mut bridge,
            json!({"id": 4, "op": "rate", "target_reply_id": current_id, "value": "minus"}),
        );
        assert!(
            accepted["ok"].as_bool().unwrap(),
            "rating the current target must still work: {accepted}"
        );
        assert_eq!(
            accepted["status"]["feedback_counts"]["negative"].as_u64(),
            Some(1)
        );
    }

    #[test]
    fn correction_rates_the_target_once_and_learns_the_corrected_text() {
        let dir = TempDir::new().unwrap();
        let mut bridge = bridge(&dir);
        let trained = train(&mut bridge, 1, "hallo welt");
        let reply_id = trained["reply_id"].as_u64().unwrap();
        let before = trained["interactions"].as_u64().unwrap();
        let corrected = call(
            &mut bridge,
            json!({"id": 2, "op": "correction", "target_reply_id": reply_id, "value": "plus", "text": "regenbogen libelle"}),
        );
        assert!(
            corrected["ok"].as_bool().unwrap(),
            "correction failed: {corrected}"
        );
        // The correction message is a training turn of its own: it consumes
        // the next reply id.
        assert_eq!(corrected["reply_id"].as_u64(), Some(reply_id + 1));
        // A correction is one learning-adjacent turn: exactly one
        // interaction more.
        assert_eq!(corrected["interactions"].as_u64(), Some(before + 1));
        // (1) Exactly-once: the same target cannot be corrected again ...
        let second = call(
            &mut bridge,
            json!({"id": 3, "op": "correction", "target_reply_id": reply_id, "value": "plus", "text": "noch eine korrektur"}),
        );
        assert_eq!(second["ok"].as_bool(), Some(false));
        assert_eq!(second["error"].as_str().unwrap(), "already_rated");
        // ... with zero side effects: the counter did not move ...
        assert_eq!(second["interactions"].as_u64(), Some(before + 1));
        // ... and the rate op on the same target is refused too (one rated
        // set covers both ops).
        let rated = call(
            &mut bridge,
            json!({"id": 4, "op": "rate", "target_reply_id": reply_id, "value": "plus"}),
        );
        assert_eq!(rated["error"].as_str().unwrap(), "already_rated");
        // The rating half landed in the engine exactly once, and the
        // correction's own reply is the new rateable target.
        let status = call(&mut bridge, json!({"id": 5, "op": "status"}));
        assert_eq!(
            status["status"]["feedback_counts"]["positive"].as_u64(),
            Some(1),
            "exactly one correction rating reached the engine: {status}"
        );
        assert_eq!(
            status["status"]["feedback_available"].as_bool(),
            Some(true),
            "the correction turn's own reply must be the new rateable target: {status}"
        );
        // (3) The learning half landed too: a read-only query over the
        // distinctive correction words returns them — render_path outputs
        // only learned graph nodes (src/language.rs:147-176), so the words
        // in the reply are neurons grown by the correction turn.
        let read = call(
            &mut bridge,
            json!({"id": 6, "op": "read_only", "text": "regenbogen"}),
        );
        assert!(read["ok"].as_bool().unwrap(), "read_only failed: {read}");
        let text = read["text"].as_str().unwrap();
        assert!(
            text.contains("regenbogen") && text.contains("libelle"),
            "the corrected text must be learned: {read}"
        );
    }

    #[test]
    fn read_only_does_not_change_the_interactions_counter() {
        let dir = TempDir::new().unwrap();
        let mut bridge = bridge(&dir);
        let trained = train(&mut bridge, 1, "hallo welt");
        let before = trained["interactions"].as_u64().unwrap();
        let read = call(
            &mut bridge,
            json!({"id": 2, "op": "read_only", "text": "was ist hallo"}),
        );
        assert!(read["ok"].as_bool().unwrap(), "read_only failed: {read}");
        assert_eq!(read["interactions"].as_u64().unwrap(), before);
        // Read-only answers never get a reply id of their own.
        assert_eq!(read["reply_id"], trained["reply_id"]);
    }

    #[test]
    fn read_only_does_not_clobber_the_rateable_target() {
        let dir = TempDir::new().unwrap();
        let mut bridge = bridge(&dir);
        let trained = train(&mut bridge, 1, "alpha beta");
        let reply_id = trained["reply_id"].as_u64().unwrap();
        // Unknown words cannot produce a path, so the read-only reply is
        // empty: without the bridge's restore, infer_read_only would clear
        // the session's rateable target (src/app/message.rs:61-65) and the
        // rating below would die with "Keine vorherige Modellantwort zum
        // Bewerten".
        let read = call(
            &mut bridge,
            json!({"id": 2, "op": "read_only", "text": "zzz qqq"}),
        );
        assert!(read["ok"].as_bool().unwrap(), "read_only failed: {read}");
        assert!(
            read["text"].as_str().unwrap().is_empty(),
            "test premise: unknown words yield an empty read-only reply: {read}"
        );
        // The train answer's rateable target survived the read-only turn ...
        assert_eq!(
            read["status"]["feedback_available"].as_bool(),
            Some(true),
            "the read-only turn must not clobber the rateable target: {read}"
        );
        // ... and rating it still reaches the engine.
        let rated = call(
            &mut bridge,
            json!({"id": 3, "op": "rate", "target_reply_id": reply_id, "value": "minus"}),
        );
        assert!(
            rated["ok"].as_bool().unwrap(),
            "the original target must still be rateable after read_only: {rated}"
        );
        assert_eq!(
            rated["status"]["feedback_counts"]["negative"].as_u64(),
            Some(1)
        );
    }

    #[test]
    fn consolidate_returns_the_engine_summary_without_an_interaction_turn() {
        let dir = TempDir::new().unwrap();
        let mut bridge = bridge(&dir);
        let trained = train(&mut bridge, 1, "hallo welt");
        let before = trained["interactions"].as_u64().unwrap();
        let consolidated = call(&mut bridge, json!({"id": 2, "op": "consolidate"}));
        assert!(
            consolidated["ok"].as_bool().unwrap(),
            "consolidate failed: {consolidated}"
        );
        // The engine's own round summary arrives unmodified under "result"
        // (src/app/message.rs:429-431).
        let result = &consolidated["result"];
        assert_eq!(result["scope"].as_str(), Some("active_neighborhood"));
        assert!(result["active_nodes"].is_u64(), "result: {result}");
        assert!(result["aliases_added"].is_u64(), "result: {result}");
        assert!(result["edges_merged"].is_u64(), "result: {result}");
        assert!(result["quarantined"].is_u64(), "result: {result}");
        // Consolidation is not a learning-adjacent turn: the interactions
        // counter did not move (src/graph.rs:338-377 only reads it).
        let status = call(&mut bridge, json!({"id": 3, "op": "status"}));
        assert_eq!(status["status"]["interactions"].as_u64(), Some(before));
    }

    #[test]
    fn unknown_and_malformed_requests_answer_id_0_or_the_given_id() {
        let dir = TempDir::new().unwrap();
        let mut bridge = bridge(&dir);
        let (malformed, shutdown) = bridge.handle("");
        assert_eq!(malformed["ok"].as_bool(), Some(false));
        assert_eq!(malformed["id"].as_u64(), Some(0));
        assert!(!shutdown);
        let (malformed, _) = bridge.handle("{not json");
        assert_eq!(malformed["ok"].as_bool(), Some(false));
        assert_eq!(malformed["id"].as_u64(), Some(0));
        let (unknown, _) = bridge.handle("{\"id\":7,\"op\":\"exec rm -rf\"}");
        assert_eq!(unknown["ok"].as_bool(), Some(false));
        assert_eq!(unknown["id"].as_u64(), Some(7));
        let (shutdown, shutdown_flag) = bridge.handle("{\"id\":8,\"op\":\"shutdown\"}");
        assert_eq!(shutdown["ok"].as_bool(), Some(true));
        assert!(shutdown_flag);
    }

    #[test]
    fn request_lines_are_read_bounded_and_trimmed() {
        let mut reader = io::Cursor::new(b"{\"id\":1}\noverflow".to_vec());
        let mut buf = Vec::new();
        match read_request(&mut reader, &mut buf).unwrap() {
            LineIn::Line(line) => assert_eq!(line, "{\"id\":1}"),
            other => panic!("expected a line, got {other:?}"),
        }
        match read_request(&mut reader, &mut buf).unwrap() {
            // No trailing newline: the tail is still one line at EOF.
            LineIn::Line(line) => assert_eq!(line, "overflow"),
            other => panic!("expected a line, got {other:?}"),
        }
        match read_request(&mut reader, &mut buf).unwrap() {
            LineIn::Eof => {}
            other => panic!("expected EOF, got {other:?}"),
        }
        let mut oversized = io::Cursor::new(vec![b'a'; MAX_REQUEST_BYTES + 10]);
        match read_request(&mut oversized, &mut buf).unwrap() {
            LineIn::TooLarge => {}
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }
}
