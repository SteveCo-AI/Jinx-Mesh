//! Op-journal mechanics (Kap. 14): step/round rollback of journaled learning,
//! crash recovery, and cap expiry — all through the full App/vault path.
use super::JOURNAL_PASS;
use crate::model::State;
use serde_json::{json, Value};

// ---- Phase-2 Block 1: Op-Journal + exakter Rollback (Kap. 14) ----

#[test]
fn op_journal_undo_eines_lernzugs_stellt_den_abschnitt_exakt_wieder_her() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("undo-eins.db");
    let mut app = crate::App::create_empty(&db, "jinx", JOURNAL_PASS, None, None).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    let vorher = app.test_abschnitt();
    let reply = app.message("alpha beta gamma").unwrap();
    assert_eq!(reply.neue_neuronen, 3);
    let status = app.status().unwrap();
    assert_eq!(status["journal_länge"], 1);
    assert_eq!(status["journal_letzter"]["art"], "learn");
    assert_eq!(status["journal_letzter"]["nummer"], 1);
    assert_ne!(app.test_abschnitt(), vorher);

    let report = app.undo(1).unwrap();
    assert_eq!(report["zurueckgerollt"], 1);
    assert!(report["ops"].as_u64().unwrap() > 0);
    assert_eq!(app.test_abschnitt(), vorher);
    let status = app.status().unwrap();
    assert_eq!(status["journal_länge"], 0);
    assert_eq!(status["journal_letzter"], Value::Null);
    // Dialog bookkeeping is history and deliberately survives the rollback.
    assert_eq!(status["interactions"], 1);

    drop(app);
    let mut app = crate::App::open(&db).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    assert_eq!(app.status().unwrap()["neurons"], 0);
    assert_eq!(app.verify_graph_full().unwrap(), 0);
    assert_eq!(app.verify_archive_full().unwrap(), 2);
}

#[test]
fn op_journal_mehrstufiger_rollback_trifft_jede_stufe_exakt() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("undo-mehrfach.db");
    let mut app = crate::App::create_empty(&db, "jinx", JOURNAL_PASS, None, None).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    let anfangs = app.test_abschnitt();
    // Later moves keep reusing the earlier words so the affected neighborhood
    // stays resident; cold records are untouched by undo either way.
    app.message("eins zwei drei").unwrap();
    let nach_eins = app.test_abschnitt();
    app.message("eins zwei drei vier").unwrap();
    let nach_zwei = app.test_abschnitt();
    app.message("eins zwei drei fuenf").unwrap();
    assert_ne!(app.test_abschnitt(), nach_zwei);
    assert_eq!(app.status().unwrap()["journal_länge"], 3);

    let report = app.undo(2).unwrap();
    assert_eq!(report["zurueckgerollt"], 2);
    assert_eq!(report["journal_länge"], 1);
    assert_eq!(app.test_abschnitt(), nach_eins);

    let report = app.undo(99).unwrap();
    assert_eq!(report["zurueckgerollt"], 1);
    assert_eq!(report["journal_länge"], 0);
    assert_eq!(app.test_abschnitt(), anfangs);
    let report = app.undo(1).unwrap();
    assert_eq!(report["zurueckgerollt"], 0);
}

#[test]
fn op_journal_runden_rollback_nach_consolidate_rollt_die_runde_zurueck() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("undo-runde.db");
    let mut app = crate::App::create_empty(&db, "jinx", JOURNAL_PASS, None, None).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    let anfangs = app.test_abschnitt();
    for _ in 0..3 {
        app.message("wort eins").unwrap();
    }
    let nach_zuegen = app.test_abschnitt();
    app.consolidate().unwrap();
    assert_eq!(app.status().unwrap()["journal_länge"], 4);
    assert_eq!(
        app.status().unwrap()["journal_letzter"]["art"],
        "consolidate"
    );
    assert_ne!(app.test_abschnitt(), nach_zuegen);
    app.message("neue sache").unwrap();
    assert_eq!(app.status().unwrap()["journal_länge"], 5);

    // /undo round: the consolidation round and everything learned after it.
    let report = app.undo_runde().unwrap();
    assert_eq!(report["zurueckgerollt"], 2);
    assert_eq!(report["journal_länge"], 3);
    assert_eq!(app.test_abschnitt(), nach_zuegen);

    // Without a boundary the whole journal is the open round.
    let report = app.undo_runde().unwrap();
    assert_eq!(report["zurueckgerollt"], 3);
    assert_eq!(app.test_abschnitt(), anfangs);

    // Every consolidation round leaves an entry, even one that changes
    // nothing beyond its boundary marker; the first round here only
    // introduces the healing bookkeeping keys.
    app.consolidate().unwrap();
    app.consolidate().unwrap();
    assert_eq!(app.status().unwrap()["journal_länge"], 2);
    let report = app.undo(1).unwrap();
    assert_eq!(report["zurueckgerollt"], 1);
    assert_eq!(report["ops"], 0);
    assert_eq!(app.status().unwrap()["journal_länge"], 1);
}

#[test]
fn op_journal_absturz_recovery_verwirft_unbestaetigte_und_meldet_es_ehrlich() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("undo-crash.db");
    let mut app = crate::App::create_empty(&db, "jinx", JOURNAL_PASS, None, None).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    app.message("haltbar wort").unwrap();
    app.test_crash_vor_commit();
    assert!(app.message("vergessen wort").is_err());
    assert_eq!(app.mode(), crate::Mode::Locked);
    drop(app);

    let mut app = crate::App::open(&db).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    let status = app.status().unwrap();
    assert_eq!(status["systemhinweis"], "1 unbestätigter Schritt verworfen");
    assert_eq!(status["interactions"], 1);
    assert_eq!(status["neurons"], 2);
    assert_eq!(status["journal_länge"], 1);
    let status = app.status().unwrap();
    assert_eq!(status["systemhinweis"], Value::Null);

    // The committed step remains exactly rollbackable after recovery.
    let report = app.undo(1).unwrap();
    assert_eq!(report["zurueckgerollt"], 1);
    assert_eq!(app.status().unwrap()["neurons"], 0);
}

#[test]
fn op_journal_kap_verfaellt_die_aeltesten_laesst_die_neuesten_rueckholbar() {
    let dir = tempfile::tempdir().unwrap();
    let mut start = State::empty();
    start.konfig.insert("journal_kap".into(), json!(3));
    let state_path = dir.path().join("state.json");
    let actions_path = dir.path().join("actions.json");
    std::fs::write(&state_path, serde_json::to_vec(&start).unwrap()).unwrap();
    std::fs::write(&actions_path, b"{\"aktionen\":{}}").unwrap();
    let db = dir.path().join("undo-kap.db");
    let mut app = crate::App::create(
        &db,
        "jinx",
        JOURNAL_PASS,
        None,
        None,
        crate::Import {
            state: state_path,
            actions: actions_path,
            legacy_audit: None,
        },
    )
    .unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    for i in 1..=5 {
        app.message(&format!("wort{i} begleiter{i}")).unwrap();
    }
    assert_eq!(app.status().unwrap()["journal_länge"], 3);

    // The newest step is rollbackable; the oldest two have expired.
    let report = app.undo(1).unwrap();
    assert_eq!(report["zurueckgerollt"], 1);
    assert_eq!(app.status().unwrap()["neurons"], 8);
    let report = app.undo(2).unwrap();
    assert_eq!(report["zurueckgerollt"], 2);
    assert_eq!(app.status().unwrap()["neurons"], 4);
    assert_eq!(app.status().unwrap()["journal_länge"], 0);
    let report = app.undo(5).unwrap();
    assert_eq!(report["zurueckgerollt"], 0);

    // Expired learning persists honestly — it is just no longer rollbackable.
    drop(app);
    let mut app = crate::App::open(&db).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    assert_eq!(app.status().unwrap()["neurons"], 4);
    assert_eq!(app.status().unwrap()["journal_länge"], 0);
}
