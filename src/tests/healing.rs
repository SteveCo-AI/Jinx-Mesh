//! Self-healing (Kap. 21/22): folding with ring/alias records, thresholds
//! from konfig data, quarantine, suspicion damping, and the journaled
//! /falsch and /echtes_wort dialog heals.
use super::{engine, engine_mit, JOURNAL_PASS};
use crate::{
    engine::Engine,
    graph,
    model::{Node, State},
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

// ---- Phase-2 Block 4: Selbstheilung (Kap. 21/22) ----

/// Registry view of one Kap.-22 verification level.
fn ebene(e: &Engine, name: &str) -> serde_json::Value {
    e.state.extra["verifikations_ebenen"][name].clone()
}

#[test]
fn aehnliches_wort_wird_als_hilfsneuron_gefoldet_ring_und_alias_gesetzt() {
    let mut e = engine();
    e.train("hund bellt laut").unwrap();
    // A different sentence, not a repetition: repeating the same question
    // would trip the Kap.-18 implicit channel instead of raising nutzt.
    e.train("hund katze sache").unwrap();
    // (a) a similar word never becomes an independent node: it exists only
    // as a folded helper in the ring of the canonical word.
    let reply = e.train("hunde bellt heute").unwrap();
    assert_eq!(reply.neue_neuronen, 2); // the helper plus the unrelated "heute"
    let helper = &e.state.neuronen["lex:hunde"];
    assert_eq!(helper.typ, "LEX");
    assert_eq!(helper.ring.as_deref(), Some("lex:hund"));
    assert_eq!(helper.extra["folding_origin"], json!(true));
    assert_eq!(
        e.state.extra["neuron_aliases"]["lex:hunde"],
        json!("lex:hund")
    );
    // The origin stays visible by name; word() resolves through the alias...
    assert_eq!(
        graph::wort_namentlich(&e.state, "hunde").as_deref(),
        Some("lex:hunde")
    );
    assert_eq!(graph::word(&e.state, "hunde").as_deref(), Some("lex:hund"));
    // ...the token's edges route through the alias onto the canonical node.
    assert!(!e.state.kanten.contains_key("lex:hunde"));
    let w = graph::edge(&e.state, "lex:hund", "lex:bellt").unwrap();
    assert!((w - 0.3).abs() < 1e-9);
    // The confirmed fold grows the word level's verification credit (Kap. 22).
    assert_eq!(ebene(&e, "wort")["verification_credit"], json!(1));
    e.state.validate().unwrap();
}

#[test]
fn folding_nutzt_die_schwellen_aus_der_konfig() {
    // (b) distance cap: dist 3 (gartenzelt/gartenzaun) never folds at the
    // default cap 2...
    let mut e = engine();
    e.train("gartenzelt sache").unwrap();
    e.train("gartenzelt anderes").unwrap();
    e.train("gartenzaun sache").unwrap();
    assert!(e.state.neuronen["lex:gartenzaun"].ring.is_none());
    assert!(e
        .state
        .extra
        .get("neuron_aliases")
        .is_none_or(|a| a.get("lex:gartenzaun").is_none()));
    // ...but folds when the konfig data raises the cap.
    let mut e = engine_mit(&[("folding_schwelle", json!(4))]);
    e.train("gartenzelt sache").unwrap();
    e.train("gartenzelt anderes").unwrap();
    e.train("gartenzaun sache").unwrap();
    assert_eq!(
        e.state.neuronen["lex:gartenzaun"].ring.as_deref(),
        Some("lex:gartenzelt")
    );
    // Use floor: a similar word against a barely used node stays independent.
    let mut e = engine_mit(&[("folding_min_nutzt", json!(9))]);
    e.train("hund bellt").unwrap();
    e.train("hund anderes").unwrap();
    e.train("hunde bellt").unwrap();
    assert!(e.state.neuronen["lex:hunde"].ring.is_none());
    assert_eq!(ebene(&e, "wort")["verification_credit"], json!(0));
}

#[test]
fn verbotener_fold_bleibt_eigenstaendig_und_strafft_die_heil_schwelle() {
    // dist 2 on a three-letter word: distance and use conditions hold, but
    // the verification evidence (1 - 2/3) stays under the initial heal
    // threshold 0.5 — the source forbids the fold (Kap. 22) and the word
    // level's threshold rises.
    let mut e = engine();
    e.train("abc sache").unwrap();
    e.train("abc zeug").unwrap();
    e.train("axy sache").unwrap();
    assert!(e.state.neuronen["lex:axy"].ring.is_none());
    assert_eq!(ebene(&e, "wort")["verification_credit"], json!(0));
    assert!((ebene(&e, "wort")["heil_schwelle"].as_f64().unwrap() - 0.6).abs() < 1e-9);
}

#[test]
fn wiederholte_erfolgreiche_heile_senken_die_heil_schwelle_messbar() {
    // (f) three confirmed folds sink the word level's threshold measurably.
    let mut e = engine();
    e.train("hund katze maus sache").unwrap();
    e.train("hund katze maus mehr").unwrap();
    let reply = e.train("hunde katzee mause sache").unwrap();
    assert_eq!(reply.neue_neuronen, 3); // three helpers, no independent nodes
    assert_eq!(ebene(&e, "wort")["verification_credit"], json!(3));
    assert!((ebene(&e, "wort")["heil_schwelle"].as_f64().unwrap() - 0.2).abs() < 1e-9);
    // The loosened threshold now admits the previously forbidden short-word
    // candidate (evidence 1 - 2/3 ≈ 0.33 ≥ 0.2)...
    e.train("abc zeug").unwrap();
    e.train("abc sachen").unwrap();
    e.train("axy zeug").unwrap();
    assert_eq!(e.state.neuronen["lex:axy"].ring.as_deref(), Some("lex:abc"));
    e.state.validate().unwrap();
}

#[test]
fn alias_zykel_wird_quarantaeniert_und_zaehlt_als_heil_misserfolg() {
    let mut e = engine();
    for wort in ["a", "b"] {
        let id = format!("lex:{wort}");
        e.state.neuronen.insert(
            id.clone(),
            Node {
                id,
                typ: "LEX".into(),
                label: wort.into(),
                staerke: 0.5,
                alter: "reif".into(),
                nutzt: 1,
                geboren: 0,
                ring: None,
                extra: BTreeMap::new(),
            },
        );
    }
    let mut alias = serde_json::Map::new();
    alias.insert("lex:a".into(), json!("lex:b"));
    alias.insert("lex:b".into(), json!("lex:a"));
    e.state
        .extra
        .insert("neuron_aliases".into(), Value::Object(alias));
    let report = crate::healing::heal(&mut e.state);
    assert_eq!(report.quarantined, 1);
    assert!(e.state.extra["quarantine"]
        .as_array()
        .unwrap()
        .iter()
        .any(|q| q["kind"] == "alias_cycle"));
    // The broken half is gone, the valid remainder stays (non-destructive).
    assert!(e.state.extra["neuron_aliases"]
        .as_object()
        .unwrap()
        .get("lex:a")
        .is_none());
    assert_eq!(e.state.extra["neuron_aliases"]["lex:b"], json!("lex:a"));
    // Every broken cycle is a failed heal on the word level: the threshold rises.
    assert!((ebene(&e, "wort")["heil_schwelle"].as_f64().unwrap() - 0.6).abs() < 1e-9);
    e.state.validate().unwrap();
}

#[test]
fn verdachtsverzeichnis_daempft_kante_und_loescht_nichts() {
    // (d) a heavily used edge the path weights contradict gets damped and
    // recorded — never deleted.
    let mut e = engine();
    for wort in ["x", "y"] {
        let id = format!("lex:{wort}");
        e.state.neuronen.insert(
            id.clone(),
            Node {
                id,
                typ: "LEX".into(),
                label: wort.into(),
                staerke: 0.5,
                alter: "reif".into(),
                nutzt: 5,
                geboren: 0,
                ring: None,
                extra: BTreeMap::new(),
            },
        );
    }
    graph::set_edge(&mut e.state, "lex:x", "lex:y", -0.8);
    graph::consolidate(&mut e.state);
    let neu = graph::edge(&e.state, "lex:x", "lex:y").unwrap();
    assert!((neu - -0.8 * 0.7).abs() < 1e-9);
    assert!(e.state.neuronen.contains_key("lex:x") && e.state.neuronen.contains_key("lex:y"));
    let verzeichnis = e.state.extra["verdachtsverzeichnis"].as_array().unwrap();
    assert_eq!(verzeichnis.len(), 1);
    assert_eq!(verzeichnis[0]["kind"], json!("edge"));
    assert_eq!(verzeichnis[0]["a"], json!("lex:x"));
    assert_eq!(verzeichnis[0]["b"], json!("lex:y"));
    assert_eq!(verzeichnis[0]["alt"], json!(-0.8));
    assert!((verzeichnis[0]["neu"].as_f64().unwrap() - neu).abs() < 1e-12);
    assert_eq!(verzeichnis[0]["zeit"], json!(0));
    // The damped edge is a confirmed heal on the structure level.
    assert_eq!(ebene(&e, "struktur")["verification_credit"], json!(1));
    e.state.validate().unwrap();
}

#[test]
fn verdacht_unter_der_nutzungsschwelle_bleibt_ungedaempft() {
    let mut e = engine();
    for wort in ["x", "y"] {
        let id = format!("lex:{wort}");
        e.state.neuronen.insert(
            id.clone(),
            Node {
                id,
                typ: "LEX".into(),
                label: wort.into(),
                staerke: 0.5,
                alter: "reif".into(),
                nutzt: 2,
                geboren: 0,
                ring: None,
                extra: BTreeMap::new(),
            },
        );
    }
    graph::set_edge(&mut e.state, "lex:x", "lex:y", -0.8);
    graph::consolidate(&mut e.state);
    assert_eq!(graph::edge(&e.state, "lex:x", "lex:y"), Some(-0.8));
    assert!(!e.state.extra.contains_key("verdachtsverzeichnis"));
    assert_eq!(ebene(&e, "struktur")["verification_credit"], json!(0));
}

#[test]
fn falsch_und_echtes_wort_werden_als_heal_journiert_und_exakt_rueckholbar() {
    // (c) the dialog heals journal as art "heal" and roll back exactly.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("heal-dialog.db");
    let mut app = crate::App::create_empty(&db, "jinx", JOURNAL_PASS, None, None).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    app.message("hunde bellt nacht").unwrap();
    app.message("hund bellt oft").unwrap();
    // The last message anchors both words, so the resident working set the
    // journal diff compares already carries them.
    app.message("hunde bellt oft hund").unwrap();
    let abschnitt_vor = app.test_abschnitt();

    let report = app.heal_befehl("falsch", "hunde ist hund").unwrap();
    assert_eq!(report["befehl"], json!("falsch"));
    assert_eq!(report["kanten_umgebogen"], json!(1));
    let status = app.status().unwrap();
    assert_eq!(status["journal_letzter"]["art"], "heal");
    let abschnitt = app.test_abschnitt();
    assert_eq!(
        abschnitt["neuronen"]["lex:hunde"]["ring"],
        json!("lex:hund")
    );
    assert_eq!(
        abschnitt["extra"]["neuron_aliases"]["lex:hunde"],
        json!("lex:hund")
    );
    // node extra is flattened into the node JSON
    assert_eq!(
        abschnitt["neuronen"]["lex:hunde"]["folding_origin"],
        json!(true)
    );
    // every edge of the folded word moved onto the canonical word
    assert!(abschnitt["kanten"].get("lex:hunde").is_none());
    let w = abschnitt["kanten"]["lex:hund"]["lex:bellt"]
        .as_f64()
        .unwrap();
    assert!((w - 0.3).abs() < 1e-9);
    assert_eq!(
        abschnitt["extra"]["verifikations_ebenen"]["wort"]["verification_credit"],
        json!(1)
    );

    // /undo rolls the heal back exactly.
    let report = app.undo(1).unwrap();
    assert_eq!(report["zurueckgerollt"], 1);
    assert_eq!(app.test_abschnitt(), abschnitt_vor);
    // The heal entry is gone; the previous learning step is the last one again.
    assert_eq!(
        app.status().unwrap()["journal_letzter"]["art"],
        json!("learn")
    );

    // /freeze forbids heals like the DEV feedback (write mode only).
    app.freeze().unwrap();
    assert!(app.heal_befehl("falsch", "hunde ist hund").is_err());
    assert!(app.heal_befehl("echtes_wort", "hunde").is_err());
    app.login("jinx", JOURNAL_PASS, None).unwrap();

    // /echtes_wort on a folded word: the Rück-Folding makes it standalone.
    app.heal_befehl("falsch", "hunde ist hund").unwrap();
    let abschnitt_gefoldet = app.test_abschnitt();
    let report = app.heal_befehl("echtes_wort", "hunde").unwrap();
    assert_eq!(report["befehl"], json!("echtes_wort"));
    assert_eq!(report["ring_entfernt"], json!(true));
    assert_eq!(report["alias_entfernt"], json!(true));
    assert_eq!(app.status().unwrap()["journal_letzter"]["art"], "heal");
    let abschnitt = app.test_abschnitt();
    assert_eq!(abschnitt["neuronen"]["lex:hunde"]["ring"], Value::Null);
    assert!(abschnitt["extra"]["neuron_aliases"]
        .as_object()
        .unwrap()
        .get("lex:hunde")
        .is_none());
    assert_eq!(
        abschnitt["extra"]["verifikations_ebenen"]["wort"]["verification_credit"],
        json!(2)
    );
    // ...and that promotion rolls back exactly too.
    app.undo(1).unwrap();
    assert_eq!(app.test_abschnitt(), abschnitt_gefoldet);

    // The fold persists in the vault and stays rollbackable after a reopen.
    drop(app);
    let mut app = crate::App::open(&db).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    app.freeze().unwrap();
    app.message("hunde hund").unwrap();
    let abschnitt = app.test_abschnitt();
    assert_eq!(
        abschnitt["neuronen"]["lex:hunde"]["ring"],
        json!("lex:hund")
    );
    assert_eq!(
        abschnitt["extra"]["neuron_aliases"]["lex:hunde"],
        json!("lex:hund")
    );
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    let report = app.undo(1).unwrap();
    assert_eq!(report["zurueckgerollt"], 1);
    assert_eq!(
        app.test_abschnitt()["neuronen"]["lex:hunde"]["ring"],
        Value::Null
    );
}

#[test]
fn heil_ohne_verifikationsquelle_scheitert_fail_loud() {
    // (e) a vault whose word level lost its registered verification source
    // refuses every heal on that level — commands fail loudly, the
    // automatic folding stays off.
    let dir = tempfile::tempdir().unwrap();
    let mut zustand = State::empty();
    let ohne_wort: serde_json::Map<String, Value> = zustand.extra["verifikations_ebenen"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(ebene, _)| ebene.as_str() != "wort")
        .map(|(ebene, eintrag)| (ebene.clone(), eintrag.clone()))
        .collect();
    assert!(!ohne_wort.is_empty());
    zustand
        .extra
        .insert("verifikations_ebenen".into(), Value::Object(ohne_wort));
    let state_path = dir.path().join("state.json");
    let actions_path = dir.path().join("actions.json");
    std::fs::write(&state_path, serde_json::to_vec(&zustand).unwrap()).unwrap();
    std::fs::write(&actions_path, b"{\"aktionen\":{}}").unwrap();
    let db = dir.path().join("heal-ohne-quelle.db");
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
    let fehler = app.heal_befehl("falsch", "hunde ist hund").unwrap_err();
    assert!(fehler.to_string().contains("Verifikationsquelle"));
    let fehler = app.heal_befehl("echtes_wort", "hunde").unwrap_err();
    assert!(fehler.to_string().contains("Verifikationsquelle"));
    // ...and the automatic folding is off: a similar word stays independent.
    app.message("hund bellt").unwrap();
    app.message("hund bellt").unwrap();
    app.message("hunde bellt").unwrap();
    let abschnitt = app.test_abschnitt();
    assert_eq!(abschnitt["neuronen"]["lex:hunde"]["ring"], Value::Null);
    assert!(abschnitt["extra"].get("neuron_aliases").is_none());
    assert_eq!(app.status().unwrap()["journal_länge"], 3);
}

#[test]
fn nutzer_korrektur_hebt_die_gelernte_folding_schwelle() {
    // Kap. 21 "lernbar": a correction beyond the effective threshold
    // persists the raised threshold as learned state; afterwards grow
    // folds that distance automatically — and undo rolls it back.
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("heal-lernbar.db");
    let mut app = crate::App::create_empty(&db, "jinx", JOURNAL_PASS, None, None).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    app.message("gartenzelt sache").unwrap();
    app.message("gartenzelt anderes").unwrap();
    app.message("gartenzaun sache").unwrap();
    assert_eq!(
        app.test_abschnitt()["neuronen"]["lex:gartenzaun"]["ring"],
        Value::Null
    );
    app.heal_befehl("falsch", "gartenzaun ist gartenzelt")
        .unwrap();
    assert_eq!(
        app.test_abschnitt()["extra"]["folding_schwelle"],
        json!(3.0)
    );
    // A fresh word at the raised distance folds automatically now; the
    // canonical word rides along in the message so paging loads its record.
    app.message("gartenzoom gartenzelt").unwrap();
    assert_eq!(
        app.test_abschnitt()["neuronen"]["lex:gartenzoom"]["ring"],
        json!("lex:gartenzelt")
    );
    // ...and undoing the correction and the fold removes the learned threshold
    app.undo(2).unwrap();
    assert!(app.test_abschnitt()["extra"]
        .get("folding_schwelle")
        .is_none());
}

#[test]
fn heil_befehl_muster_kommen_aus_der_konfig_nicht_aus_dem_code() {
    // Kap. 20: editing the pattern data redirects the command — the code
    // carries no formulation.
    let dir = tempfile::tempdir().unwrap();
    let mut zustand = State::empty();
    zustand.konfig.insert(
        "heal_befehl_muster".into(),
        json!({"falsch": "{x} meint {y}", "echtes_wort": "{x}"}),
    );
    let state_path = dir.path().join("state.json");
    let actions_path = dir.path().join("actions.json");
    std::fs::write(&state_path, serde_json::to_vec(&zustand).unwrap()).unwrap();
    std::fs::write(&actions_path, b"{\"aktionen\":{}}").unwrap();
    let db = dir.path().join("heal-muster.db");
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
    app.message("hunde bellt").unwrap();
    app.message("hund bellt").unwrap();
    // the old formulation no longer separates the words...
    assert!(app.heal_befehl("falsch", "hunde ist hund").is_err());
    // ...the configured one does.
    app.heal_befehl("falsch", "hunde meint hund").unwrap();
    assert_eq!(
        app.test_abschnitt()["neuronen"]["lex:hunde"]["ring"],
        json!("lex:hund")
    );
    // unknown words fail honestly instead of healing anything
    let fehler = app
        .heal_befehl("falsch", "unbekannteswort meint hund")
        .unwrap_err();
    assert!(fehler.to_string().contains("Wort ist nicht im Netz"));
}

#[test]
fn heil_konfig_wird_validiert_und_alte_vaults_fallen_auf_die_startwerte_zurueck() {
    // Range checks for the new konfig keys and registry shapes.
    let mut s = State::empty();
    s.konfig.insert("folding_schwelle".into(), json!(10.5));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert("folding_min_nutzt".into(), json!(-1.0));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert("verdacht_schwelle".into(), json!(0.1));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert("heil_schritt".into(), json!(1.5));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert("heil_schwelle_min".into(), json!(0.8));
    s.konfig.insert("heil_schwelle_max".into(), json!(0.2));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig
        .insert("heal_befehl_muster".into(), json!("kein objekt"));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert(
        "heal_befehl_muster".into(),
        json!({"falsch": "{x} {y}", "echtes_wort": "{x}"}),
    );
    assert!(s.validate().is_err()); // empty separator is ambiguous
    let mut s = State::empty();
    s.konfig.insert(
        "heal_befehl_muster".into(),
        json!({"falsch": "{x} ist {y}", "echtes_wort": "{x} {y}"}),
    );
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert(
        "heal_befehl_muster".into(),
        json!({"falsch": "{x} ist {y}"}),
    );
    assert!(s.validate().is_err()); // echtes_wort pattern missing
    let mut s = State::empty();
    s.extra.insert("verifikations_ebenen".into(), json!([]));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.extra.insert(
        "verifikations_ebenen".into(),
        json!({"wort": {"quelle": "x", "verification_credit": -1, "heil_schwelle": 0.5}}),
    );
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.extra.insert(
        "verifikations_ebenen".into(),
        json!({"wort": {"quelle": "x", "verification_credit": 0, "heil_schwelle": 1.5}}),
    );
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.extra.insert("folding_schwelle".into(), json!(12.0));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.extra.insert("neuron_aliases".into(), json!({"lex:a": 5}));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.extra.insert("verdachtsverzeichnis".into(), json!({}));
    assert!(s.validate().is_err());
    // An empty registry is honest: no level registered, no heal anywhere.
    let mut s = State::empty();
    s.extra.insert("verifikations_ebenen".into(), json!({}));
    assert!(s.validate().is_ok());

    // Vault compatibility: a state without every Kap.-21/22 key (every
    // vault predating the mechanism) still validates...
    let mut s = State::empty();
    for key in [
        "folding_min_nutzt",
        "folding_schwelle",
        "heal_befehl_muster",
        "heil_schritt",
        "heil_schwelle_max",
        "heil_schwelle_min",
        "verdacht_min_nutzt",
        "verdacht_schwelle",
    ] {
        s.konfig.remove(key);
    }
    s.extra.remove("verifikations_ebenen");
    assert!(s.validate().is_ok());
    // ...and Engine::new restores the configured defaults from the fixture.
    let mut e = Engine::new(s, json!({"aktionen":{}})).unwrap();
    assert_eq!(e.state.konfig["folding_schwelle"], json!(2));
    assert_eq!(e.state.konfig["verdacht_schwelle"], json!(-0.5));
    assert_eq!(
        e.state.konfig["heal_befehl_muster"]["falsch"],
        json!("{x} ist {y}")
    );
    assert_eq!(
        e.state.konfig["heal_befehl_muster"]["echtes_wort"],
        json!("{x}")
    );
    // The registry falls back to the fixture data at read time and only
    // materializes in the state once a heal confirms something.
    assert!(!e.state.extra.contains_key("verifikations_ebenen"));
    e.train("hund sache").unwrap();
    e.train("hund anderes").unwrap();
    e.train("hunde sache").unwrap();
    assert_eq!(
        e.state.neuronen["lex:hunde"].ring.as_deref(),
        Some("lex:hund")
    );
    assert_eq!(
        e.state.extra["verifikations_ebenen"]["wort"]["verification_credit"],
        json!(1)
    );
    assert_eq!(
        e.state.extra["verifikations_ebenen"]["wort"]["quelle"],
        json!("kontext_aehnlichkeit")
    );
    assert_eq!(
        e.state.extra["verifikations_ebenen"]["struktur"]["quelle"],
        json!("konkurrierende_pfade")
    );
}
