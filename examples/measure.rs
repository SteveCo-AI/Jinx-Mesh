use saeugling::App;
use std::{path::PathBuf, time::Instant};

fn main() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let db = PathBuf::from(temp.path()).join("jinx-empty.db");
    let password = "Disposable benchmark passphrase 314159!";

    let start = Instant::now();
    let mut app = App::create_empty(&db, "benchmark", password, None, None)?;
    let create_ms = start.elapsed().as_millis();

    let start = Instant::now();
    app.login("benchmark", password, None)?;
    let login_ms = start.elapsed().as_millis();

    let start = Instant::now();
    for index in 0..100 {
        app.message(&format!("Benchmark turn {index} Jinx learns language"))?;
    }
    let chat_ms = start.elapsed().as_millis();
    let status = app.status()?;
    drop(app);

    let conn = rusqlite::Connection::open(&db)?;
    let snapshot: u64 = conn.query_row("SELECT length(sealed) FROM snapshot", [], |r| r.get(0))?;
    let journal: u64 = conn.query_row("SELECT SUM(length(sealed)) FROM audit", [], |r| r.get(0))?;
    println!(
        "{}",
        serde_json::json!({
            "vault_create_ms": create_ms,
            "login_ms": login_ms,
            "steady_chat_turns": 100,
            "steady_chat_total_ms": chat_ms,
            "steady_chat_mean_ms": chat_ms as f64 / 100.0,
            "encrypted_snapshot_bytes": snapshot,
            "encrypted_journal_bytes": journal,
            "sqlite_file_bytes": std::fs::metadata(&db)?.len(),
            "status": status
        })
    );
    Ok(())
}
