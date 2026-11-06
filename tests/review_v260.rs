use saeugling::{App, Mode};
use tempfile::TempDir;
const PASS: &str = "Review v260 disposable passphrase!";
fn app(dir: &TempDir) -> App {
    let mut app =
        App::create_empty(&dir.path().join("review.db"), "review", PASS, None, None).unwrap();
    app.login("review", PASS, None).unwrap();
    app
}
#[test]
fn journal_append_failure_revokes_mutated_session() {
    let dir = TempDir::new().unwrap();
    let mut app = app(&dir);
    let conn = rusqlite::Connection::open(dir.path().join("review.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_journal BEFORE INSERT ON op_journal BEGIN SELECT RAISE(ABORT,'review injected journal failure'); END;").unwrap();
    let error = app.message("nevercommitted vocabulary").unwrap_err();
    println!(
        "Error: {error}; mode={:?}; status={:?}",
        app.mode(),
        app.status()
    );
    assert_eq!(
        app.mode(),
        Mode::Locked,
        "A failed write-ahead append must discard the mutated session"
    );
}
#[test]
fn undo_keeps_unchanged_outgoing_edges_of_cold_nodes() {
    let dir = TempDir::new().unwrap();
    let mut app = app(&dir);
    app.message("alpha bravo").unwrap();
    app.message("alpha charlie").unwrap();
    println!("baseline: {}", app.graph_view());
    app.message("alpha bravo").unwrap();
    app.message("unrelated zebra").unwrap();
    app.undo(2).unwrap();
    drop(app);
    let mut app = App::open(&dir.path().join("review.db")).unwrap();
    app.login("review", PASS, None).unwrap();
    app.freeze().unwrap();
    app.message("alpha charlie").unwrap();
    let view = app.graph_view();
    println!("after undo/reopen: {view}");
    assert!(
        view["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["from"] == "lex:alpha" && e["to"] == "lex:charlie"),
        "Undo must not delete an unchanged edge"
    );
}
#[test]
fn undo_preserves_resident_node_budget() {
    let dir = TempDir::new().unwrap();
    let mut app = app(&dir);
    let blocks: Vec<String> = (0..35)
        .map(|i| {
            (0..5)
                .map(|j| {
                    let bytes = sha2::Sha256::digest(format!("review-{i}-{j}"));
                    format!(
                        "w{}",
                        bytes[..8]
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>()
                    )
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    for b in &blocks {
        app.message(b).unwrap();
    }
    for b in &blocks {
        app.message(b).unwrap();
    }
    app.undo(35).unwrap();
    let s = app.status().unwrap();
    println!("After undo: {s}");
    assert!(
        s["active_nodes"].as_u64().unwrap() <= 128,
        "Undo must not hydrate the entire history"
    );
}
use sha2::Digest;

#[test]
fn continuing_after_journal_failure_does_not_corrupt_audit_history() {
    let dir = TempDir::new().unwrap();
    let mut app = app(&dir);
    let conn = rusqlite::Connection::open(dir.path().join("review.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_journal BEFORE INSERT ON op_journal BEGIN SELECT RAISE(ABORT,'review injected journal failure'); END;").unwrap();
    assert!(app.message("nevercommitted vocabulary").is_err());
    conn.execute_batch("DROP TRIGGER fail_journal;").unwrap();
    drop(conn);
    let next = app.message("next successfulmessage");
    println!("Next message: {next:?}");
    drop(app);
    let mut app = App::open(&dir.path().join("review.db")).unwrap();
    let login = app.login("review", PASS, None);
    println!("Login after recovery: {login:?}");
    assert!(
        login.is_ok(),
        "A journal append failure must not make later committed history unloadable"
    );
}
