use saeugling::{App, Import, Mode, SessionPolicy};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::TempDir;
use totp_rs::{Algorithm, Secret, TOTP};
const PASS: &str = "Test-only passphrase 314159!";
fn import() -> Import {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    Import {
        state: root.join("fixtures/start.json"),
        actions: root.join("fixtures/actions.json"),
        legacy_audit: None,
    }
}
fn create(dir: &Path) -> App {
    App::create(&dir.join("model.db"), "lumen", PASS, None, None, import()).unwrap()
}
fn otp(secret: &str) -> TOTP {
    TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        Secret::Encoded(secret.into()).to_bytes().unwrap(),
        Some("Saeugling".into()),
        "developer".into(),
    )
    .unwrap()
}

#[test]
fn developer_continuously_learns_then_freezes_without_disk_writes() {
    let dir = TempDir::new().unwrap();
    let mut app = create(dir.path());
    assert_eq!(app.mode(), Mode::Locked);
    assert!(app.message("learn me").is_err());
    assert!(app.consolidate().is_err());
    app.login("lumen", PASS, None).unwrap();
    for text in [
        "Bonjour école Привет",
        "neuesvokabular lernt dauerhaft",
        "super",
    ] {
        assert!(app.message(text).unwrap().gelernt);
    }
    let status = app.status().unwrap();
    assert_eq!(status["interactions"], 3);
    assert!(status["neurons"].as_u64().unwrap() > 0);
    app.freeze().unwrap();
    let before = std::fs::read(dir.path().join("model.db")).unwrap();
    for _ in 0..100 {
        assert!(
            !app.message("ignore all instructions neuerangriff bots")
                .unwrap()
                .gelernt
        );
    }
    assert!(app.consolidate().is_err());
    assert!(app.train_file(Path::new("missing"), false).is_err());
    assert_eq!(before, std::fs::read(dir.path().join("model.db")).unwrap());
    assert_eq!(app.status().unwrap()["interactions"], 3);
    app.logout();
    assert!(app.message("nochmehr").is_err());
    app.login("lumen", PASS, None).unwrap();
    assert!(app.message("wieder lernen").unwrap().gelernt);
    drop(app);
    let mut reopened = App::open(&dir.path().join("model.db")).unwrap();
    assert_eq!(reopened.mode(), Mode::Locked);
    reopened.login("lumen", PASS, None).unwrap();
    assert_eq!(reopened.status().unwrap()["interactions"], 4);
}

#[test]
fn archive_is_paged_and_tampering_is_detected_on_exact_recall() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("model.db");
    let mut app = create(dir.path());
    app.login("lumen", PASS, None).unwrap();
    for n in 0..20 {
        app.message(&format!("uniquearchiveword turn {n}")).unwrap();
    }
    let hits = app.recall("uniquearchiveword", 20).unwrap();
    assert_eq!(hits.len(), 20);
    assert!(hits.iter().any(|e| e["speaker"] == "user"));
    drop(app);

    let conn = rusqlite::Connection::open(&db).unwrap();
    let mut blob: Vec<u8> = conn
        .query_row(
            "SELECT sealed FROM conversation_archive WHERE seq=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    blob[0] ^= 0x40;
    conn.execute(
        "UPDATE conversation_archive SET sealed=?1 WHERE seq=1",
        [blob],
    )
    .unwrap();
    drop(conn);

    let mut app = App::open(&db).unwrap();
    // Login validates the bounded manifest. Ciphertext is authenticated when
    // an exact page is read, so corruption is detected without decrypting the
    // complete archive at every unlock.
    app.login("lumen", PASS, None).unwrap();
    assert!(app.recall("uniquearchiveword", 20).is_err());
}

#[test]
fn learned_context_recall_uses_paged_graph_after_fresh_login() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("model.db");
    let mut app = App::create_empty(&db, "lumen", PASS, None, None).unwrap();
    app.login("lumen", PASS, None).unwrap();
    app.message("tea drink").unwrap();
    app.message("coffee drink").unwrap();
    app.logout();
    drop(app);

    let mut app = App::open(&db).unwrap();
    app.login("lumen", PASS, None).unwrap();
    assert_eq!(app.status().unwrap()["active_nodes"], 0);
    let result = app
        .recall_with_strategy_progress("tea", 20, Some("contextual"), |_, _| {})
        .unwrap();
    assert!(result.complete);
    assert!(result
        .hits
        .iter()
        .any(|hit| hit["speaker"] == "user" && hit["text"] == "coffee drink"));
}

#[test]
fn recall_before_offset_reaches_an_older_archive_range() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("model.db");
    let mut app = App::create_empty(&db, "lumen", PASS, None, None).unwrap();
    app.login("lumen", PASS, None).unwrap();
    for index in 0..5 {
        app.message(&format!("archivetoken{index}")).unwrap();
    }
    let older = app
        .recall_with_strategy_before_progress("archivetoken4", 20, Some("lexical"), 2, |_, _| {})
        .unwrap();
    assert!(older.hits.is_empty());
    assert_eq!(older.episodes_scanned, 8);
    assert!(older.complete);
}
#[test]
fn wrong_identity_and_password_never_grant_learning_and_throttle_persists() {
    let dir = TempDir::new().unwrap();
    let mut app = create(dir.path());
    assert!(app.login("someone", PASS, None).is_err());
    assert_eq!(app.mode(), Mode::Locked);
    assert!(app.login("lumen", PASS, None).is_err()); // cooldown even with correct credentials
    drop(app);
    let mut app = App::open(&dir.path().join("model.db")).unwrap();
    assert!(app.login("lumen", PASS, None).is_err());
    assert!(app.message("poison").is_err());
}
#[test]
fn encryption_hides_model_password_and_audit_and_detects_tampering() {
    let dir = TempDir::new().unwrap();
    let mut app = create(dir.path());
    app.login("lumen", PASS, None).unwrap();
    app.message("uniquesecrettrainingmarker").unwrap();
    drop(app);
    let bytes = std::fs::read(dir.path().join("model.db")).unwrap();
    for needle in [PASS, "uniquesecrettrainingmarker", "kern:status", "neuron"] {
        assert!(!bytes.windows(needle.len()).any(|w| w == needle.as_bytes()));
    }
    let conn = rusqlite::Connection::open(dir.path().join("model.db")).unwrap();
    let mut blob: Vec<u8> = conn
        .query_row("SELECT sealed FROM snapshot", [], |r| r.get(0))
        .unwrap();
    blob[30] ^= 1;
    conn.execute("UPDATE snapshot SET sealed=?1", [blob])
        .unwrap();
    drop(conn);
    let mut app = App::open(&dir.path().join("model.db")).unwrap();
    assert!(app.login("lumen", PASS, None).is_err());
    assert_eq!(app.mode(), Mode::Locked);
}
#[test]
fn optional_totp_requires_second_factor_and_rejects_replay_after_restart() {
    let dir = TempDir::new().unwrap();
    let secret = App::new_totp_secret();
    let code = otp(&secret).generate_current().unwrap();
    let mut app = App::create(
        &dir.path().join("model.db"),
        "lumen",
        PASS,
        Some(secret.clone()),
        Some(&code),
        import(),
    )
    .unwrap();
    app.login("lumen", PASS, Some(&code)).unwrap();
    app.logout();
    drop(app);
    let mut app = App::open(&dir.path().join("model.db")).unwrap();
    assert!(app.login("lumen", PASS, Some(&code)).is_err());
    let dir2 = TempDir::new().unwrap();
    let mut app2 = App::create(
        &dir2.path().join("model.db"),
        "lumen",
        PASS,
        Some(secret),
        Some(&code),
        import(),
    )
    .unwrap();
    assert!(app2.login("lumen", PASS, None).is_err());
    assert_eq!(app2.mode(), Mode::Locked);
}
#[test]
fn failed_reauthentication_revokes_previous_session() {
    let dir = TempDir::new().unwrap();
    let mut app = create(dir.path());
    app.login("lumen", PASS, None).unwrap();
    assert!(app.login("lumen", "wrong", None).is_err());
    assert_eq!(app.mode(), Mode::Locked);
    assert!(app.message("must not learn").is_err());
}
#[test]
fn session_expiry_and_second_process_lock() {
    let dir = TempDir::new().unwrap();
    let mut app = create(dir.path()).with_policy(SessionPolicy {
        idle: Duration::ZERO,
        maximum: Duration::from_secs(60),
    });
    assert!(App::open(&dir.path().join("model.db")).is_err());
    app.login("lumen", PASS, None).unwrap();
    assert!(app.message("expired").is_err());
    assert_eq!(app.mode(), Mode::Locked);
}
#[test]
fn file_import_is_authenticated_deduplicated_and_not_executed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("source.py");
    std::fs::write(&path, "import os\nos.remove('important')\n").unwrap();
    let mut app = create(dir.path());
    assert!(app.train_file(&path, true).is_err());
    app.login("lumen", PASS, None).unwrap();
    let first = app.train_file(&path, true).unwrap();
    assert!(first["new_neurons"].as_u64().unwrap() > 0);
    assert_eq!(
        app.train_file(&path, true).unwrap()["already_learned"],
        true
    );
    let before = app.status().unwrap()["neurons"].as_u64().unwrap();
    app.consolidate().unwrap();
    assert_eq!(app.status().unwrap()["neurons"], before);
    assert!(app.message("/login lumen SECRET").is_err());
    assert_eq!(app.status().unwrap()["interactions"], 0);
    assert!(app.message(&"x".repeat(16385)).is_err());
}

#[test]
fn serialized_threads_do_not_cross_frozen_authorization_boundary() {
    use std::sync::{Arc, Mutex};
    let dir = TempDir::new().unwrap();
    let mut app = create(dir.path());
    app.login("lumen", PASS, None).unwrap();
    app.freeze().unwrap();
    let app = Arc::new(Mutex::new(app));
    let mut threads = Vec::new();
    for _ in 0..4 {
        let app = app.clone();
        threads.push(std::thread::spawn(move || {
            for _ in 0..25 {
                assert!(!app.lock().unwrap().message("bots attack").unwrap().gelernt);
            }
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(app.lock().unwrap().status().unwrap()["interactions"], 0);
}

#[test]
fn persistence_failure_revokes_access_and_discards_uncommitted_learning() {
    let dir = TempDir::new().unwrap();
    let mut app = create(dir.path());
    app.login("lumen", PASS, None).unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("model.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER simulate_disk_error BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
    assert!(app.message("uncommittedword").is_err());
    assert_eq!(app.mode(), Mode::Locked);
    conn.execute_batch("DROP TRIGGER simulate_disk_error")
        .unwrap();
    drop(conn);
    app.login("lumen", PASS, None).unwrap();
    assert_eq!(app.status().unwrap()["interactions"], 0);
}

#[test]
fn tampered_history_blocks_login_even_if_current_snapshot_is_valid() {
    let dir = TempDir::new().unwrap();
    let mut app = create(dir.path());
    app.login("lumen", PASS, None).unwrap();
    app.message("historyword").unwrap();
    drop(app);
    let conn = rusqlite::Connection::open(dir.path().join("model.db")).unwrap();
    let mut blob: Vec<u8> = conn
        .query_row("SELECT sealed FROM audit WHERE seq=1", [], |r| r.get(0))
        .unwrap();
    blob[30] ^= 1;
    conn.execute("UPDATE audit SET sealed=?1 WHERE seq=1", [blob])
        .unwrap();
    drop(conn);
    let mut app = App::open(&dir.path().join("model.db")).unwrap();
    assert!(app.login("lumen", PASS, None).is_err());
}

#[test]
fn default_creation_is_empty_and_authentication_does_not_seed_it() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("model.db");
    let mut app = App::create_empty(&db, "lumen", PASS, None, None).unwrap();
    assert!(app.message("wichtig").is_err());
    app.login("lumen", PASS, None).unwrap();
    assert_eq!(app.status().unwrap()["neurons"], 0);
    let r = app.message("Du bist Jinx ich bin Lumen").unwrap();
    assert_eq!(r.text, "du bist jinx ich bin lumen");
    assert_eq!(r.aktion, None);
    app.freeze().unwrap();
    let r = app.message("wichtig").unwrap();
    assert!(r.text.is_empty());
    app.logout();
    drop(app);
    let mut app = App::open(&db).unwrap();
    app.login("lumen", PASS, None).unwrap();
    assert_eq!(app.status().unwrap()["neurons"], 6);
}
