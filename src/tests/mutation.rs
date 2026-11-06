//! Novelty operator (Phase 2.5): the disarmed-by-default mutation layer,
//! deterministic proposals, selection, external-only consolidation, the
//! wegweiser landscape, and freeze/lock refusal.
use super::{engine, engine_mit};
use crate::{engine::Engine, graph, model::State};
use serde_json::json;

// ---- Phase 2.5: Novelty-Operator / Mutationsschicht ----

/// Environment key of one attack point — the same serialization the layer
/// writes into the wegweiser.
fn mutations_umgebung(prev: Option<&str>, next: Option<&str>) -> String {
    serde_json::to_string(&(prev, next)).unwrap()
}

/// Pre-seed one attack point's wegweiser landscape entry by hand.
fn wegweiser_sehen(e: &mut Engine, umgebung: &str, versuche: u64, erfolge: u64) {
    let tabelle = e
        .state
        .extra
        .entry("mutations_wegweiser".into())
        .or_insert(json!({}));
    if let Some(map) = tabelle.as_object_mut() {
        map.insert(
            umgebung.into(),
            json!({"versuche": versuche, "erfolge": erfolge}),
        );
    }
}

#[test]
fn mutation_ist_standardmaessig_aus_und_fuegt_nie_etwas_ein() {
    // SICHERHEIT (binding): the layer ships disarmed — only the vault's
    // konfig data arms it. The default fixture state never mutates.
    let mut e = engine();
    for text in ["a b c", "a b c", "x y", "a b c d"] {
        let reply = e.train(text).unwrap();
        assert_eq!(reply.mutation, None);
    }
    assert!(!e.state.extra.contains_key("mutation_bilanz"));
    assert!(!e.state.extra.contains_key("mutation_kandidat"));
    assert!(!e.state.extra.contains_key("mutations_wegweiser"));
    assert!(!e.state.extra.contains_key("mutations_memory"));
}

#[test]
fn mutation_greift_nur_aktiviert_und_nur_mit_wahrscheinlichkeit() {
    // Armed but rate zero: the dice never passes the rate gate.
    let mut e = engine_mit(&[
        ("mutation_aktivier", json!(true)),
        ("mutation_rate", json!(0.0)),
    ]);
    e.train("a b").unwrap();
    e.train("x y").unwrap();
    for _ in 0..3 {
        assert_eq!(e.train("a b").unwrap().mutation, None);
    }
    assert!(!e.state.extra.contains_key("mutation_bilanz"));
    // Disarmed despite full rate: the switch gates independently.
    let mut e = engine_mit(&[
        ("mutation_aktivier", json!(false)),
        ("mutation_rate", json!(1.0)),
    ]);
    e.train("a b").unwrap();
    e.train("x y").unwrap();
    for _ in 0..3 {
        assert_eq!(e.train("a b").unwrap().mutation, None);
    }
    assert!(!e.state.extra.contains_key("mutation_bilanz"));
}

#[test]
fn mutation_fuegt_hoechstens_ein_gelerntes_wort_deterministisch_ein() {
    let mutator = || {
        let mut e = engine_mit(&[
            ("mutation_rate", json!(1.0)),
            ("mutation_schwellwert", json!(0.0)),
        ]);
        e.train("a b").unwrap();
        e.train("x y").unwrap();
        e.state
            .konfig
            .insert("mutation_aktivier".into(), json!(true));
        e
    };
    let mut e = mutator();
    let reply = e.train("a b").unwrap();
    let wort = reply.mutation.clone().expect("genau eine mutation");
    // The word comes from learned data, never from code.
    assert!(graph::word(&e.state, &wort).is_some());
    // At most one insertion: exactly one word more than the unmutated answer.
    let worte = crate::language::parse(&reply.text).words;
    assert_eq!(worte.len(), 3);
    assert!(worte.contains(&wort));
    // Determinism: the identical state proposes the identical mutation.
    let mut zwilling = mutator();
    let zwilling_reply = zwilling.train("a b").unwrap();
    assert_eq!(zwilling_reply.text, reply.text);
    assert_eq!(zwilling_reply.mutation, reply.mutation);
    // The accepted proposal is pending for an external signal; nothing is
    // festigt yet (model-collapse protection).
    assert_eq!(e.state.extra["mutation_bilanz"]["festigungen"], json!(0));
    assert_eq!(e.state.extra["mutation_bilanz"]["versuche"], json!(1));
    assert_eq!(e.state.extra["mutation_bilanz"]["verworfen"], json!(0));
    let kandidat = &e.state.extra["mutation_kandidat"];
    assert_eq!(kandidat["wort"], json!(wort));
    assert_eq!(kandidat["kontext"], json!(crate::language::context("a b")));
    // And the word does not belong at that spot per the learned patterns.
    let stelle = kandidat["stelle"].as_u64().unwrap() as usize;
    if let Some(prev) = stelle.checked_sub(1).map(|i| worte[i].clone()) {
        let (a, b) = (
            graph::word(&e.state, &prev).unwrap(),
            graph::word(&e.state, &wort).unwrap(),
        );
        assert!(graph::edge(&e.state, &a, &b).is_none_or(|w| w <= e.state.number("paar_basis")));
    }
    assert!(e.state.validate().is_ok());
}

#[test]
fn selektion_verwirft_schlechte_mutanten_und_behaelt_sie_als_memory() {
    // The threshold at 1.0 demands the mutant be at least as good as the
    // original; the isolated word breaks the confirmed pairs and loses.
    let mut e = engine_mit(&[
        ("mutation_rate", json!(1.0)),
        ("mutation_schwellwert", json!(1.0)),
    ]);
    e.train("a b").unwrap();
    e.train("x y").unwrap();
    e.state
        .konfig
        .insert("mutation_aktivier".into(), json!(true));
    let reply = e.train("a b").unwrap();
    assert_eq!(reply.mutation, None);
    assert_eq!(reply.text, "a b.");
    let bilanz = &e.state.extra["mutation_bilanz"];
    assert_eq!(bilanz["versuche"], json!(1));
    assert_eq!(bilanz["verworfen"], json!(1));
    assert_eq!(bilanz["festigungen"], json!(0));
    assert!(!e.state.extra.contains_key("mutation_kandidat"));
    // The failed attempt stays as damped memory (Kap.-5: failures remain).
    let memory = e.state.extra["mutations_memory"].as_array().unwrap();
    assert_eq!(memory.len(), 1);
    assert_eq!(memory[0]["schicksal"], json!("verworfen"));
    assert!(memory[0]["original"].as_f64().unwrap() > memory[0]["urteil"].as_f64().unwrap());
    // ...and dampens the attack point in the wegweiser.
    let umgebung = mutations_umgebung(memory[0]["prev"].as_str(), memory[0]["next"].as_str());
    let eintrag = &e.state.extra["mutations_wegweiser"][umgebung.as_str()];
    assert_eq!(eintrag["versuche"], json!(1));
    assert_eq!(eintrag["erfolge"], json!(0));
    assert!(e.state.validate().is_ok());
}

#[test]
fn mutation_festigt_nur_durch_externes_signal_kein_self_reward() {
    let mut e = engine_mit(&[
        ("mutation_rate", json!(1.0)),
        ("mutation_schwellwert", json!(0.0)),
    ]);
    e.train("a b").unwrap();
    e.train("x y").unwrap();
    e.state
        .konfig
        .insert("mutation_aktivier".into(), json!(true));
    // The pending word's usage count before any proposal (the setup walks
    // through it once); the proposals must not move it.
    let wort_davor = e.state.neuronen["lex:x"].nutzt;
    // Several mutating runs WITHOUT any positive external signal: every
    // repetition of the same question rates the prior proposal negatively.
    for _ in 0..4 {
        let reply = e.train("a b").unwrap();
        assert!(reply.mutation.is_some());
        // The proposal itself festigt nothing — the counter stays equal
        // run after run (model-collapse protection, binding).
        assert_eq!(e.state.extra["mutation_bilanz"]["festigungen"], json!(0));
    }
    let bilanz = &e.state.extra["mutation_bilanz"];
    assert_eq!(bilanz["versuche"], json!(4));
    assert_eq!(bilanz["negativ"], json!(3));
    assert_eq!(bilanz["verworfen"], json!(0));
    // The four proposals changed nothing structural by themselves: the
    // pending word kept its node exactly as the setup left it.
    let wort = e.state.extra["mutation_kandidat"]["wort"]
        .as_str()
        .unwrap()
        .to_owned();
    let id = format!("lex:{wort}");
    let knoten = e.state.neuronen[&id].clone();
    assert_eq!(knoten.nutzt, wort_davor);
    assert!(!knoten.extra.contains_key("mutation_festigung"));
    for nachbar in ["prev", "next"] {
        if let Some(w) = e.state.extra["mutation_kandidat"][nachbar].as_str() {
            let nachbar_id = graph::word(&e.state, w).unwrap();
            assert!(graph::edge(&e.state, &nachbar_id, &id).is_none());
            assert!(graph::edge(&e.state, &id, &nachbar_id).is_none());
        }
    }
    // Only the external + festigt — the single path to consolidation.
    e.train("+").unwrap();
    assert_eq!(e.state.extra["mutation_bilanz"]["festigungen"], json!(1));
    assert!(!e.state.extra.contains_key("mutation_kandidat"));
    let knoten = &e.state.neuronen[&id];
    assert_eq!(knoten.nutzt, wort_davor + 1);
    assert!(knoten.extra.contains_key("mutation_festigung"));
    // The reward landed in the wegweiser at the attack point...
    let memory = e.state.extra["mutations_memory"].as_array().unwrap();
    let festigung = memory.last().unwrap();
    assert_eq!(festigung["schicksal"], json!("festigt"));
    let umgebung = mutations_umgebung(festigung["prev"].as_str(), festigung["next"].as_str());
    assert_eq!(
        e.state.extra["mutations_wegweiser"][umgebung.as_str()]["erfolge"],
        json!(1)
    );
    // ...and the confirmed pattern grew the local helper edges there.
    let verbunden = match (festigung["prev"].as_str(), festigung["next"].as_str()) {
        (Some(prev), _) => {
            graph::edge(&e.state, &graph::word(&e.state, prev).unwrap(), &id).is_some()
        }
        (None, Some(next)) => {
            graph::edge(&e.state, &id, &graph::word(&e.state, next).unwrap()).is_some()
        }
        _ => false,
    };
    assert!(verbunden);
    assert!(e.state.validate().is_ok());
}

#[test]
fn frisches_wort_wird_erst_durch_bestaetigung_zum_jungen_neuron() {
    // The import budget leaves echo without a neuron; the layer may propose
    // the fresh word, but only the external + may grow it.
    let mut e = engine_mit(&[
        ("mutation_rate", json!(1.0)),
        ("mutation_schwellwert", json!(0.0)),
        ("max_neu_pro_nachricht", json!(1)),
    ]);
    e.train("a b").unwrap();
    e.state
        .konfig
        .insert("mutation_aktivier".into(), json!(true));
    let reply = e.train("a b echo").unwrap();
    assert!(graph::word(&e.state, "echo").is_none());
    // echo is the only word neither in the answer nor in the net.
    let wort = reply.mutation.clone().expect("frisches wort vorgeschlagen");
    assert_eq!(wort, "echo");
    // The proposal creates no neuron by itself.
    assert!(!e.state.neuronen.contains_key("lex:echo"));
    e.train("+").unwrap();
    // The external confirmation grows it as a JUNGE NEURON at the point.
    let knoten = &e.state.neuronen["lex:echo"];
    assert_eq!(knoten.typ, "LEX");
    assert_eq!(knoten.alter, "jung");
    assert_eq!(knoten.nutzt, 1);
    assert_eq!(e.state.extra["mutation_bilanz"]["festigungen"], json!(1));
    assert!(e.state.validate().is_ok());
}

#[test]
fn der_wegweiser_lernt_umgebungen_nie_die_woerter() {
    let mut e = engine_mit(&[
        ("mutation_rate", json!(1.0)),
        ("mutation_schwellwert", json!(0.0)),
    ]);
    e.train("a b").unwrap();
    e.train("x y").unwrap();
    e.state
        .konfig
        .insert("mutation_aktivier".into(), json!(true));
    // Three externally rewarded mutants grow the success landscape.
    for _ in 0..3 {
        assert!(e.train("a b").unwrap().mutation.is_some());
        e.train("+").unwrap();
    }
    assert_eq!(e.state.extra["mutation_bilanz"]["festigungen"], json!(3));
    let erfolge: u64 = e.state.extra["mutations_wegweiser"]
        .as_object()
        .unwrap()
        .values()
        .map(|eintrag| eintrag["erfolge"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(erfolge, 3);
    // The entries learn the attack points' environments — never the word
    // itself: the concrete word choice stays a dice under temperature.
    for eintrag in e.state.extra["mutations_wegweiser"]
        .as_object()
        .unwrap()
        .values()
    {
        let felder: Vec<&str> = eintrag
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(felder, vec!["erfolge", "versuche"]);
    }
}

#[test]
fn langeweile_hebt_die_wildheit_und_weicht_dem_vorhersehbaren_punkt_aus() {
    let mut e = engine_mit(&[
        ("mutation_rate", json!(1.0)),
        ("mutation_schwellwert", json!(0.0)),
    ]);
    e.train("a b").unwrap();
    e.train("x y").unwrap();
    e.state
        .konfig
        .insert("mutation_aktivier".into(), json!(true));
    // The (a, b) attack point is predictably successful: 9 of 10 rewarded.
    wegweiser_sehen(&mut e, &mutations_umgebung(Some("a"), Some("b")), 10, 9);
    let reply = e.train("a b").unwrap();
    assert!(reply.mutation.is_some());
    // The wildness rose by the configured step...
    let wildheit = e.state.extra["mutation_wildheit"].as_f64().unwrap();
    assert!((wildheit - 0.2).abs() < 1e-9);
    // ...and the proposal moved AWAY from the predictable point.
    let kandidat = &e.state.extra["mutation_kandidat"];
    assert!((kandidat["prev"].clone(), kandidat["next"].clone()) != (json!("a"), json!("b")));
    // Without a predictable point the wildness relaxes again by the step.
    let mut zwilling = engine_mit(&[
        ("mutation_rate", json!(1.0)),
        ("mutation_schwellwert", json!(0.0)),
    ]);
    zwilling.train("a b").unwrap();
    zwilling.train("x y").unwrap();
    zwilling
        .state
        .konfig
        .insert("mutation_aktivier".into(), json!(true));
    zwilling
        .state
        .extra
        .insert("mutation_wildheit".into(), json!(0.5));
    zwilling.train("a b").unwrap();
    let wildheit = zwilling.state.extra["mutation_wildheit"].as_f64().unwrap();
    assert!((wildheit - 0.3).abs() < 1e-9);
}

#[test]
fn mutation_ist_in_freeze_und_gesperrtem_modus_verboten() {
    let dir = tempfile::tempdir().unwrap();
    // A trained vault with the layer armed through konfig data. The dev-mode
    // candidate (c) sits in the activated neighborhood: a's edges load it.
    let mut e = engine();
    e.train("a b").unwrap();
    e.train("a c").unwrap();
    e.state
        .konfig
        .insert("mutation_aktivier".into(), json!(true));
    e.state.konfig.insert("mutation_rate".into(), json!(1.0));
    e.state
        .konfig
        .insert("mutation_schwellwert".into(), json!(0.0));
    let state_path = dir.path().join("state.json");
    let actions_path = dir.path().join("actions.json");
    std::fs::write(&state_path, serde_json::to_vec(&e.state).unwrap()).unwrap();
    std::fs::write(&actions_path, b"{\"aktionen\":{}}").unwrap();
    let db = dir.path().join("mutation-freeze.db");
    let pass = "Mutations-Freeze test passphrase!";
    let mut app = crate::App::create(
        &db,
        "jinx",
        pass,
        None,
        None,
        crate::Import {
            state: state_path,
            actions: actions_path,
            legacy_audit: None,
        },
    )
    .unwrap();
    app.login("jinx", pass, None).unwrap();
    // Developer mode proposes (the same vault, the same move).
    let reply = app.message("a b").unwrap();
    assert!(reply.mutation.is_some());
    assert_eq!(
        app.test_abschnitt()["extra"]["mutation_bilanz"]["versuche"],
        json!(1)
    );
    // /freeze: read-only inference never reaches the layer — no proposal,
    // no attempt counted, no bookkeeping change.
    app.freeze().unwrap();
    let frozen = app.message("a b").unwrap();
    assert_eq!(frozen.mutation, None);
    assert_eq!(frozen.text, "a b.");
    assert_eq!(
        app.test_abschnitt()["extra"]["mutation_bilanz"]["versuche"],
        json!(1)
    );
    assert_eq!(
        app.test_abschnitt()["extra"]["mutation_bilanz"]["festigungen"],
        json!(0)
    );
    // Locked mode refuses the move outright.
    drop(app);
    let mut app = crate::App::open(&db).unwrap();
    assert!(app.message("a b").is_err());
}

#[test]
fn mutations_konfig_wird_validiert_und_alte_vaults_fallen_auf_die_startwerte() {
    for (key, wert) in [
        ("mutation_aktivier", json!("vielleicht")),
        ("mutation_rate", json!(1.5)),
        ("mutation_rate", json!(-0.1)),
        ("mutation_temperatur", json!(2.0)),
        ("mutation_schwellwert", json!(-1.0)),
        ("mutation_langeweile_schwelle", json!(1.5)),
        ("mutation_langeweile_ab_versuchen", json!(0.0)),
        ("mutation_wildheit_schritt", json!(-0.5)),
    ] {
        let mut s = State::empty();
        s.konfig.insert(key.into(), wert);
        assert!(s.validate().is_err(), "{key} muss abgelehnt werden");
    }
    // The bookkeeping shapes stay honest.
    let mut s = State::empty();
    s.extra.insert(
        "mutations_wegweiser".into(),
        json!({"[\"a\",\"b\"]": {"versuche": 1, "erfolge": 2}}),
    );
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.extra.insert("mutations_wegweiser".into(), json!([]));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.extra.insert("mutation_bilanz".into(), json!(3));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.extra
        .insert("mutation_kandidat".into(), json!({"stelle": 1}));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.extra.insert("mutation_wildheit".into(), json!(1.5));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.extra
        .insert("mutations_memory".into(), json!({"schicksal": "x"}));
    assert!(s.validate().is_err());
    // Vaults predating the layer (every key absent) load unchanged...
    let mut s = State::empty();
    for key in crate::mutation::KONFIG {
        s.konfig.remove(key);
    }
    assert!(s.validate().is_ok());
    // ...and Engine::new restores the fixture defaults — the layer stays
    // disarmed until the vault's konfig data arms it.
    let e = Engine::new(s, json!({"aktionen":{}})).unwrap();
    assert_eq!(e.state.konfig["mutation_aktivier"], json!(false));
    assert_eq!(e.state.konfig["mutation_rate"], json!(0.1));
    assert_eq!(e.state.konfig["mutation_temperatur"], json!(0.2));
    assert_eq!(e.state.konfig["mutation_schwellwert"], json!(0.8));
    assert_eq!(e.state.konfig["mutation_langeweile_schwelle"], json!(0.8));
    assert_eq!(e.state.konfig["mutation_langeweile_ab_versuchen"], json!(3));
    assert_eq!(e.state.konfig["mutation_wildheit_schritt"], json!(0.2));
}
