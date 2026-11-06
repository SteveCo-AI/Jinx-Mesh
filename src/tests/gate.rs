//! Integration gate (Kap. 24): rule data rejects or accepts learning moves,
//! imports, and consolidations — with honest rejection markers and
//! validated rule columns.
use super::JOURNAL_PASS;
use crate::{engine::Engine, model::State};
use serde_json::{json, Value};

// ---- Phase-2 Block 5: Integration Gate + Anti-Hardcode (Kap. 24/20) ----

/// A vault created from a state JSON — the owner's data path, so tests
/// configure gate rules exactly like a real configuration edit.
fn vault_mit_zustand(
    dir: &std::path::Path,
    name: &str,
    zustand: &State,
) -> (crate::App, std::path::PathBuf) {
    let state_path = dir.join(format!("{name}-state.json"));
    let actions_path = dir.join(format!("{name}-actions.json"));
    std::fs::write(&state_path, serde_json::to_vec(zustand).unwrap()).unwrap();
    std::fs::write(&actions_path, b"{\"aktionen\":{}}").unwrap();
    let db = dir.join(name);
    let app = crate::App::create(
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
    (app, db)
}

#[test]
fn gate_reject_verhindert_den_commit_und_vermerkt_die_verweigerung_ehrlich() {
    let dir = tempfile::tempdir().unwrap();
    // The rule data caps one move at a single new node — configuration,
    // never code.
    let mut zustand = State::empty();
    zustand.konfig.insert(
        "gate_regeln".into(),
        json!({
            "erwartet_entfernt": false,
            "erwartet_gewicht_range": [-1.0, 1.0],
            "erwartet_neue_knoten_max": 1,
            "pflichtfelder_extra": []
        }),
    );
    let (mut app, db) = vault_mit_zustand(dir.path(), "gate-reject.db", &zustand);
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    let vorher = app.test_abschnitt();
    // (a) the forced rule violation rejects the move instead of committing:
    // honest notice, no learning, no archive write.
    let fehler = app.message("alpha beta gamma").unwrap_err();
    let meldung = fehler.to_string();
    assert!(meldung.starts_with("Integration-Prüfung fehlgeschlagen"));
    assert!(meldung.contains("erwartet_neue_knoten_max"));
    assert_eq!(app.test_abschnitt(), vorher);
    let status = app.status().unwrap();
    assert_eq!(status["neurons"], 0);
    assert_eq!(status["interactions"], 0);
    assert_eq!(app.verify_graph_full().unwrap(), 0);
    assert_eq!(app.verify_archive_full().unwrap(), 0);
    // The rejection is journaled as its own art with the refused
    // interaction's number.
    assert_eq!(status["journal_länge"], 1);
    assert_eq!(status["journal_letzter"]["art"], "integration_reject");
    assert_eq!(status["journal_letzter"]["nummer"], 1);

    // The marker is a confirmed record, not a crash remnant: it survives a
    // reload without any discarded-step notice.
    drop(app);
    let mut app = crate::App::open(&db).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    let status = app.status().unwrap();
    assert_eq!(status["systemhinweis"], Value::Null);
    assert_eq!(status["journal_länge"], 1);
    assert_eq!(status["journal_letzter"]["art"], "integration_reject");
    assert_eq!(status["neurons"], 0);

    // Rollback possibility through the Block-1 mechanics: /undo removes the
    // marker (it changed nothing).
    let report = app.undo(1).unwrap();
    assert_eq!(report["zurueckgerollt"], 1);
    assert_eq!(report["ops"], 0);
    assert_eq!(app.status().unwrap()["journal_länge"], 0);

    // The session stays usable; the next conforming move commits normally.
    let reply = app.message("eins").unwrap();
    assert_eq!(reply.neue_neuronen, 1);
    let status = app.status().unwrap();
    assert_eq!(status["journal_letzter"]["art"], "learn");
    assert_eq!(status["neurons"], 1);
}

#[test]
fn gate_akzeptiert_normale_lernzuege_importe_und_konsolidierungen() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _db) = vault_mit_zustand(dir.path(), "gate-accept.db", &State::empty());
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    // (b) with the fixture rule data every ordinary move passes the gate.
    let reply = app.message("eins zwei drei").unwrap();
    assert_eq!(reply.neue_neuronen, 3);
    let status = app.status().unwrap();
    assert_eq!(status["journal_länge"], 1);
    assert_eq!(status["journal_letzter"]["art"], "learn");
    // A file import passes the same gate.
    let datei = dir.path().join("import.txt");
    std::fs::write(&datei, "vier fuenf").unwrap();
    let report = app.train_file(&datei, false).unwrap();
    assert_eq!(report["new_neurons"], 2);
    // A consolidation round passes and marks the round boundary.
    app.consolidate().unwrap();
    let status = app.status().unwrap();
    assert_eq!(status["journal_länge"], 3);
    assert_eq!(status["journal_letzter"]["art"], "consolidate");
    // /undo round removes the boundary round; the gate did not weaken the
    // journal mechanics.
    let report = app.undo_runde().unwrap();
    assert_eq!(report["zurueckgerollt"], 1);
    assert_eq!(app.status().unwrap()["journal_länge"], 2);
    let report = app.undo(2).unwrap();
    assert_eq!(report["zurueckgerollt"], 2);
    assert_eq!(app.status().unwrap()["neurons"], 0);
}

#[test]
fn gate_regeln_werden_als_reine_daten_gelesen_ohne_codeaenderung() {
    let dir = tempfile::tempdir().unwrap();
    // (c) Open/Closed: the same binary, the same moves — the verdict follows
    // the JSON rule columns alone; no code change is involved anywhere.
    // Default fixture data accepts the move.
    let (mut a, _) = vault_mit_zustand(dir.path(), "c-default.db", &State::empty());
    a.login("jinx", JOURNAL_PASS, None).unwrap();
    a.message("alpha beta gamma").unwrap();
    assert_eq!(a.status().unwrap()["neurons"], 3);

    // A tighter cap in the data rejects the very same move.
    let mut knapp = State::empty();
    knapp.konfig.insert(
        "gate_regeln".into(),
        json!({
            "erwartet_entfernt": false,
            "erwartet_gewicht_range": [-1.0, 1.0],
            "erwartet_neue_knoten_max": 2,
            "pflichtfelder_extra": []
        }),
    );
    let (mut b, _) = vault_mit_zustand(dir.path(), "c-kap.db", &knapp);
    b.login("jinx", JOURNAL_PASS, None).unwrap();
    let fehler = b.message("alpha beta gamma").unwrap_err();
    assert!(fehler.to_string().contains("erwartet_neue_knoten_max"));
    assert_eq!(b.status().unwrap()["neurons"], 0);

    // A data-declared mandatory extra field rejects a consolidation on a
    // fresh vault (the field never exists); the same command on default
    // data passes (Block-5 accept test).
    let mut pflicht = State::empty();
    pflicht.konfig.insert(
        "gate_regeln".into(),
        json!({
            "erwartet_entfernt": false,
            "erwartet_gewicht_range": [-1.0, 1.0],
            "erwartet_neue_knoten_max": 128,
            "pflichtfelder_extra": ["kanal2_letzte"]
        }),
    );
    let (mut c, _) = vault_mit_zustand(dir.path(), "c-pflicht.db", &pflicht);
    c.login("jinx", JOURNAL_PASS, None).unwrap();
    let fehler = c.consolidate().unwrap_err();
    assert!(fehler.to_string().contains("pflichtfelder_extra"));
    assert!(fehler.to_string().contains("kanal2_letzte"));
    // The round committed nothing: no healing bookkeeping, no boundary.
    let abschnitt = c.test_abschnitt();
    assert!(abschnitt["extra"].get("quarantine").is_none());
    assert!(abschnitt["extra"].get("neuron_aliases").is_none());
    assert_eq!(
        c.status().unwrap()["journal_letzter"]["art"],
        "integration_reject"
    );

    // A data-declared weight band rejects a move whose ordinary pair weight
    // lands outside it (the pair basis is 0.2, the band caps at 0.15).
    let mut band = State::empty();
    band.konfig.insert(
        "gate_regeln".into(),
        json!({
            "erwartet_entfernt": false,
            "erwartet_gewicht_range": [-1.0, 0.15],
            "erwartet_neue_knoten_max": 128,
            "pflichtfelder_extra": []
        }),
    );
    let (mut d, _) = vault_mit_zustand(dir.path(), "c-range.db", &band);
    d.login("jinx", JOURNAL_PASS, None).unwrap();
    let fehler = d.message("p q").unwrap_err();
    assert!(fehler.to_string().contains("erwartet_gewicht_range"));
    assert!(fehler.to_string().contains("0.2"));
    // Nothing committed: no nodes, no edge, a rejection marker instead.
    assert_eq!(d.status().unwrap()["neurons"], 0);
    let abschnitt = d.test_abschnitt();
    assert!(abschnitt["kanten"].as_object().unwrap().is_empty());
    assert_eq!(
        d.status().unwrap()["journal_letzter"]["art"],
        "integration_reject"
    );
}

#[test]
fn gate_konfig_wird_validiert_und_alte_vaults_fallen_auf_die_startwerte_zurueck() {
    // Range/shape checks for the rule data columns (State::validate runs the
    // gate registry's own konfig check).
    let mut s = State::empty();
    s.konfig.insert("gate_regeln".into(), json!([]));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert(
        "gate_regeln".into(),
        json!({"erwartet_neue_knoten_max": -1.0}),
    );
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert(
        "gate_regeln".into(),
        json!({"erwartet_neue_knoten_max": "zwei"}),
    );
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert(
        "gate_regeln".into(),
        json!({"erwartet_entfernt": "vielleicht"}),
    );
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert(
        "gate_regeln".into(),
        json!({"erwartet_gewicht_range": [-1.0]}),
    );
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert(
        "gate_regeln".into(),
        json!({"erwartet_gewicht_range": [0.5, -0.5]}),
    );
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig
        .insert("gate_regeln".into(), json!({"pflichtfelder_extra": [5]}));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig
        .insert("gate_regeln".into(), json!({"pflichtfelder_extra": [""]}));
    assert!(s.validate().is_err());
    // An unknown declared column never validates — fail loud, no fake safety.
    let mut s = State::empty();
    s.konfig.insert(
        "gate_regeln".into(),
        json!({"erwartet_neue_knoten_max": 128, "neue_spalte": 1}),
    );
    assert!(s.validate().is_err());

    // Vault compatibility: a state without the key (every vault predating
    // the gate) still validates...
    let mut s = State::empty();
    s.konfig.remove("gate_regeln");
    assert!(s.validate().is_ok());
    // ...and Engine::new restores the configured defaults from the fixture.
    let e = Engine::new(s, json!({"aktionen":{}})).unwrap();
    assert_eq!(
        e.state.konfig["gate_regeln"]["erwartet_neue_knoten_max"],
        json!(128)
    );
    assert_eq!(
        e.state.konfig["gate_regeln"]["erwartet_entfernt"],
        json!(false)
    );
    assert_eq!(
        e.state.konfig["gate_regeln"]["erwartet_gewicht_range"],
        json!([-1.0, 1.0])
    );
    assert_eq!(
        e.state.konfig["gate_regeln"]["pflichtfelder_extra"],
        json!([])
    );
}
