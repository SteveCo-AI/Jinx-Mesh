//! Uncertainty fallback (Kap. 19): the disarmed scaffold flow, its konfig
//! data, throttling, channel-2 result values, and journal/rollback parity.
use super::{engine_mit, quelldateien, JOURNAL_PASS};
use crate::{
    engine::Engine,
    graph,
    model::{Node, State},
};
use serde_json::json;
use std::collections::BTreeMap;

// ---- Phase-2 Block 3: Unsicherheits-Fallback (Kap. 19) ----

/// Pair key of the rating channels for one context-feedback entry.
fn paar(prev: Option<&str>, wort: &str) -> String {
    serde_json::to_string(&(prev, wort)).unwrap()
}

/// Kap.-19 mechanism fixture: a small graph where "a b" walks the answer
/// path "a b c", with contradictory context-feedback ratings in the "a b"
/// theme (positive on the c step, negative on the input→answer pair a→c)
/// and a second, independent "b a" theme with its own contradiction. The
/// flow ships disarmed (V01), so the tests exercising its mechanics arm it
/// through the konfig data — the same opt-in the mutation tests use.
fn fallback_state() -> State {
    let mut s = State::empty();
    s.konfig.insert("fallback_aktivier".into(), json!(true));
    for wort in ["a", "b", "c"] {
        let id = format!("lex:{wort}");
        s.neuronen.insert(
            id.clone(),
            Node {
                id,
                typ: "LEX".into(),
                label: wort.into(),
                staerke: 0.5,
                alter: "reif".into(),
                nutzt: 3,
                geboren: 0,
                ring: None,
                extra: BTreeMap::new(),
            },
        );
    }
    graph::set_edge(&mut s, "lex:a", "lex:b", 0.5);
    graph::set_edge(&mut s, "lex:b", "lex:c", 0.5);
    let mut beobachtungen = serde_json::Map::new();
    beobachtungen.insert(paar(None, "a"), json!({"marks":{},"end":0,"continue":5}));
    beobachtungen.insert(
        paar(Some("a"), "b"),
        json!({"marks":{"":5},"end":1,"continue":5}),
    );
    beobachtungen.insert(
        paar(Some("b"), "c"),
        json!({"marks":{".":3},"end":3,"continue":0}),
    );
    s.extra.insert(
        "structure_observations".into(),
        serde_json::Value::Object(beobachtungen),
    );
    let mut ratings = serde_json::Map::new();
    for (thema, negativ) in [
        (crate::language::context("a b"), ("a", "c")),
        (crate::language::context("b a"), ("b", "a")),
    ] {
        let mut eintraege = serde_json::Map::new();
        eintraege.insert(paar(Some("b"), "c"), json!(0.3));
        eintraege.insert(paar(Some(negativ.0), negativ.1), json!(-0.4));
        ratings.insert(thema, serde_json::Value::Object(eintraege));
    }
    s.extra.insert(
        "context_feedback".into(),
        serde_json::Value::Object(ratings),
    );
    s
}

/// Composed question/status text of one configured variant with `wort` in
/// the placeholder slot — expectations are built from konfig data, never
/// from hardcoded sentences.
fn geruest_voll(key: &str, index: usize, wort: &str) -> String {
    State::empty().konfig[key][index]
        .as_str()
        .unwrap()
        .replace("{wort}", wort)
}

#[test]
fn fallback_ist_standardmaessig_aus_ohne_konfig_aktivierung() {
    // V01 (owner decision, binding): the scaffold flow ships disarmed —
    // `fallback_aktivier` defaults to false in the fixture data and only
    // the vault's konfig arms it (the mutation_aktivier pattern).
    assert_eq!(State::empty().konfig["fallback_aktivier"], json!(false));
    let mut s = fallback_state();
    s.konfig.insert("fallback_aktivier".into(), json!(false));
    let mut e = Engine::new(s, json!({})).unwrap();
    // The plain answer comes back untouched...
    let erste = e.train("a b").unwrap();
    assert_eq!(erste.fallback, None);
    assert_eq!(erste.text, "a b c.");
    // ...also when the question repeats (the repetition channel keeps
    // marking the start word, which may redirect the walked path — the
    // point here is that never a scaffold text is appended).
    for _ in 0..2 {
        let reply = e.train("a b").unwrap();
        assert_eq!(reply.fallback, None);
        for key in ["fallback_frage_geruest", "fallback_status_durchsage"] {
            let varianten = State::empty().konfig[key].as_array().unwrap().len();
            for variante in 0..varianten {
                assert!(!reply.text.contains(&geruest_voll(key, variante, "c")));
            }
        }
    }
    // The disarmed flow leaves no bookkeeping and opens no channel-2
    // question.
    assert!(!e.state.extra.contains_key("fallback_pro_thema"));
    assert!(!e.state.extra.contains_key("fallback_geruest_zaehler"));
    assert!(e.state.extra["kanal2_letzte"].get("offen").is_none());
}

#[test]
fn fallback_gemischtes_signal_ergaenzt_geruest_und_gelerntes_wort() {
    let mut e = Engine::new(fallback_state(), json!({})).unwrap();
    let reply = e.train("a b").unwrap();
    // The normal answer stays (not emptied) and one utterance is appended.
    assert!(reply.text.starts_with("a b c."));
    assert_eq!(reply.fallback.as_deref(), Some("nachfrage"));
    // Scaffold from the konfig data with the learned conversation word
    // carrying the strongest contradiction (the a→c pair, -0.4) in the
    // placeholder slot; rotation starts at the first variant.
    assert!(reply
        .text
        .contains(&geruest_voll("fallback_frage_geruest", 0, "c")));
    // Theme counter, rotation step, open channel-2 question.
    let thema = crate::language::context("a b");
    assert_eq!(
        e.state.extra["fallback_pro_thema"][thema.as_str()],
        json!(1)
    );
    assert_eq!(e.state.extra["fallback_geruest_zaehler"], json!(1));
    assert_eq!(
        e.state.extra["kanal2_letzte"]["offen"]["thema"],
        json!(thema)
    );
    // The utterance never becomes the rateable target.
    assert!(!e.state.extra.contains_key("feedback_target"));
}

#[test]
fn gemischtes_signal_entsteht_aus_natuerlichem_dialog_und_wird_gedrosselt() {
    // The flow is opt-in data (V01): arming it through the konfig, not code.
    let mut e = engine_mit(&[("fallback_aktivier", json!(true))]);
    // Natural conversation: the question "p q" is corrected negatively, its
    // answer flips, a continuation is taught, and a positive correction
    // follows — the "p q" theme then carries contradictory ratings and the
    // next identical question is honestly unclear.
    e.train("q a").unwrap();
    e.train("q a").unwrap(); // repetition → implicit negative in the "q a" theme
    e.train("p q").unwrap(); // answered from the q→a edge
    e.train("- stimmt nicht").unwrap(); // negative on the answer in the "p q" theme
    e.train("p q").unwrap(); // the rating flips the answer to the echo "p q."
    e.train("- nochmal nicht").unwrap(); // negative on the echo too
    e.train("q u a").unwrap(); // teach the q→u→a continuation
    e.train("p q").unwrap(); // the answer becomes "q u a."
    e.train("+ richtig so").unwrap(); // positive on "q u a." in the "p q" theme
                                      // Same question again: +0.25 and -0.25 in one theme → unclear → question.
    let r1 = e.train("p q").unwrap();
    assert_eq!(r1.fallback.as_deref(), Some("nachfrage"));
    assert_eq!(
        r1.text,
        format!("q u a. {}", geruest_voll("fallback_frage_geruest", 0, "a"))
    );
    // The user answers; a normal learning move without any repetition rating.
    e.train("so ist es gemeint").unwrap();
    // Still unclear → second question with the rotated variant.
    let r2 = e.train("p q").unwrap();
    assert_eq!(r2.fallback.as_deref(), Some("nachfrage"));
    assert_eq!(
        r2.text,
        format!("q u a. {}", geruest_voll("fallback_frage_geruest", 1, "a"))
    );
    e.train("ganz anders").unwrap();
    // Third case to the same theme: the status announcement replaces the
    // question.
    let r3 = e.train("p q").unwrap();
    assert_eq!(r3.fallback.as_deref(), Some("durchsage"));
    assert_eq!(
        r3.text,
        format!(
            "q u a. {}",
            geruest_voll("fallback_status_durchsage", 0, "a")
        )
    );
}

#[test]
fn fallback_drosselung_dritter_fall_zum_thema_bringt_die_status_durchsage() {
    let mut e = Engine::new(fallback_state(), json!({})).unwrap();
    let r1 = e.train("a b").unwrap();
    let r2 = e.train("a b").unwrap();
    let r3 = e.train("a b").unwrap();
    assert_eq!(r1.fallback.as_deref(), Some("nachfrage"));
    assert_eq!(r2.fallback.as_deref(), Some("nachfrage"));
    assert_eq!(r3.fallback.as_deref(), Some("durchsage"));
    // The announcement is konfig data composed with the learned word, and
    // none of the question scaffolds leaks into it.
    assert!(r3
        .text
        .contains(&geruest_voll("fallback_status_durchsage", 0, "c")));
    for variante in 0..3 {
        assert!(!r3
            .text
            .contains(&geruest_voll("fallback_frage_geruest", variante, "c")));
    }
    // The theme counter stops at the configured cap...
    let thema = crate::language::context("a b");
    assert_eq!(
        e.state.extra["fallback_pro_thema"][thema.as_str()],
        json!(2)
    );
    // ...and the announcement leaves no open channel-2 question; the last
    // recorded result value is the move that answered the second question.
    assert!(e.state.extra["kanal2_letzte"].get("offen").is_none());
    assert_eq!(
        e.state.extra["kanal2_letzte"]["ergebnis"]["worte"],
        json!(["a", "b"])
    );
}

#[test]
fn fallback_drosselung_zaehlt_pro_thema() {
    let mut s = fallback_state();
    // The "a b" theme is already exhausted to the question cap.
    let thema_a = crate::language::context("a b");
    let mut map = serde_json::Map::new();
    map.insert(thema_a.clone(), json!(2));
    s.extra
        .insert("fallback_pro_thema".into(), serde_json::Value::Object(map));
    let mut e = Engine::new(s, json!({})).unwrap();
    let r_a = e.train("a b").unwrap();
    assert_eq!(r_a.fallback.as_deref(), Some("durchsage"));
    // A different theme still gets its question, independent of the cap.
    let r_b = e.train("b a").unwrap();
    assert_eq!(r_b.fallback.as_deref(), Some("nachfrage"));
    let thema_b = crate::language::context("b a");
    assert_eq!(
        e.state.extra["fallback_pro_thema"][thema_a.as_str()],
        json!(2)
    );
    assert_eq!(
        e.state.extra["fallback_pro_thema"][thema_b.as_str()],
        json!(1)
    );
}

#[test]
fn fallback_gerueste_stehen_als_konfig_daten_und_rotieren_nach_nutzung() {
    let konfig = State::empty().konfig;
    for key in ["fallback_frage_geruest", "fallback_status_durchsage"] {
        let varianten = konfig[key].as_array().unwrap();
        assert!(varianten.len() >= 2, "{key} braucht mehrere Varianten");
        assert!(varianten
            .iter()
            .all(|v| v.as_str().is_some_and(|s| !s.is_empty())));
    }
    // Rotation by usage counting: consecutive questions cycle through the
    // variants — alternating themes keeps the throttle out of the way.
    let varianten = konfig["fallback_frage_geruest"].as_array().unwrap().len();
    let mut e = Engine::new(fallback_state(), json!({})).unwrap();
    let r1 = e.train("a b").unwrap();
    assert!(r1
        .text
        .contains(&geruest_voll("fallback_frage_geruest", 0, "c")));
    let r2 = e.train("b a").unwrap();
    assert!(r2
        .text
        .contains(&geruest_voll("fallback_frage_geruest", 1, "a")));
    let r3 = e.train("a b").unwrap();
    assert!(r3
        .text
        .contains(&geruest_voll("fallback_frage_geruest", 2, "c")));
    // The fourth utterance wraps the rotation back to the first variant.
    let r4 = e.train("b a").unwrap();
    assert!(r4
        .text
        .contains(&geruest_voll("fallback_frage_geruest", 3 % varianten, "a")));
    assert_eq!(e.state.extra["fallback_geruest_zaehler"], json!(4));
}

#[test]
fn fallback_geruesttexte_kommen_nicht_aus_dem_code() {
    // Kap. 20: every scaffold and status formulation lives as konfig data in
    // fixtures/start.json; no source file contains the full texts.
    let konfig = State::empty().konfig;
    let mut texte = Vec::new();
    for key in ["fallback_frage_geruest", "fallback_status_durchsage"] {
        texte.extend(
            konfig[key]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|v| v.as_str()),
        );
    }
    assert!(texte.len() >= 4);
    for pfad in quelldateien() {
        let inhalt = std::fs::read_to_string(&pfad).unwrap();
        for text in &texte {
            assert!(
                !inhalt.contains(text),
                "Verhaltenstext im Code: {text} ({})",
                pfad.display()
            );
        }
    }
}

#[test]
fn kanal2_letzte_wird_vom_fallback_aus_nutzung_befuellt() {
    let mut e = Engine::new(fallback_state(), json!({})).unwrap();
    e.train("a b").unwrap(); // question → open slot
    let thema = crate::language::context("a b");
    assert_eq!(
        e.state.extra["kanal2_letzte"]["offen"]["thema"],
        json!(thema)
    );
    // The user's answer is the recorded result value; the open slot closes.
    let reply = e.train("ganz anderes thema").unwrap();
    assert_eq!(reply.fallback, None);
    assert_eq!(
        e.state.extra["kanal2_letzte"]["ergebnis"]["thema"],
        json!(thema)
    );
    assert_eq!(
        e.state.extra["kanal2_letzte"]["ergebnis"]["worte"],
        json!(["ganz", "anderes", "thema"])
    );
    assert_eq!(
        e.state.extra["kanal2_letzte"]["ergebnis"]["runde"],
        json!(2)
    );
    assert!(e.state.extra["kanal2_letzte"].get("offen").is_none());
}

#[test]
fn die_nachfrage_wird_nie_zum_bewertungsziel() {
    let mut e = Engine::new(fallback_state(), json!({})).unwrap();
    e.train("a b").unwrap(); // question appended; no rateable target remains
    assert!(!e.state.extra.contains_key("feedback_target"));
    let feedback_vorher = e.state.extra["context_feedback"].clone();
    // Repeating the question as the answer — normally an implicit negative
    // on the previous response — rates nothing here: no target exists.
    let reply = e.train("a b").unwrap();
    assert_eq!(reply.fallback.as_deref(), Some("nachfrage")); // still unclear, asked again
    assert_eq!(e.state.extra["context_feedback"], feedback_vorher);
    assert!(!e.state.extra.contains_key("feedback_target"));
    // An explicit +/- on the question fails honestly: nothing rateable exists.
    assert!(e.train("+ doch").is_err());
    // A normal word in the answer still learns as an ordinary node, but
    // with no target it rates nothing.
    e.train("super vielleicht").unwrap();
    assert!(graph::word(&e.state, "super").is_some());
    assert_eq!(e.state.extra["context_feedback"], feedback_vorher);
    // No anchor bookkeeping ever appears (V01: the mechanism is gone).
    assert!(!e.state.extra.contains_key("anker_maturing"));
}

#[test]
fn ohne_widerspruch_oder_mit_bewertung_gibt_es_keinen_fallback() {
    // Only a positive value reaches the threshold: no contradiction.
    let mut s = fallback_state();
    let thema = crate::language::context("a b");
    s.extra.get_mut("context_feedback").unwrap()[&thema]
        .as_object_mut()
        .unwrap()
        .remove(&paar(Some("a"), "c"));
    let mut e = Engine::new(s, json!({})).unwrap();
    let reply = e.train("a b").unwrap();
    assert_eq!(reply.fallback, None);
    assert_eq!(reply.text, "a b c.");

    // Contradiction present, but an explicit +/- rating is this move's
    // definitive signal — no question.
    let mut s = fallback_state();
    s.extra.insert(
        "feedback_target".into(),
        crate::language::target("x y", "x y."),
    );
    let mut e = Engine::new(s, json!({})).unwrap();
    let reply = e.train("- a b").unwrap();
    assert_eq!(reply.feedback, Some(-1));
    assert_eq!(reply.fallback, None);
    assert_eq!(reply.text, "a b c.");
}

#[test]
fn fallback_konfig_wird_validiert_und_alte_vaults_fallen_auf_die_startwerte_zurueck() {
    // Range checks for the new konfig keys and learned bookkeeping shapes.
    let mut s = State::empty();
    s.konfig.insert("fallback_unklar_min".into(), json!(2.5));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert("fallback_drossel_max".into(), json!(0.0));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert("fallback_frage_geruest".into(), json!([]));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig
        .insert("fallback_frage_geruest".into(), json!(["gut", 5]));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig
        .insert("fallback_status_durchsage".into(), json!("kein array"));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig
        .insert("fallback_aktivier".into(), json!("vielleicht"));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.extra.insert("fallback_pro_thema".into(), json!([]));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.extra.insert("fallback_geruest_zaehler".into(), json!(-1));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.extra
        .insert("kanal2_letzte".into(), json!({"offen": {"thema": 7}}));
    assert!(s.validate().is_err());

    // Vault compatibility: a state without the Kap.-19 keys (every vault
    // predating the mechanism or its switch) still validates...
    let mut s = State::empty();
    for key in [
        "fallback_aktivier",
        "fallback_drossel_max",
        "fallback_frage_geruest",
        "fallback_status_durchsage",
        "fallback_unklar_min",
    ] {
        s.konfig.remove(key);
    }
    assert!(s.validate().is_ok());
    // ...and Engine::new restores the configured defaults from the fixture.
    let e = Engine::new(s, json!({"aktionen":{}})).unwrap();
    // The restored switch stays disarmed: only the vault's konfig data arms
    // the flow (V01).
    assert_eq!(e.state.konfig["fallback_aktivier"], json!(false));
    assert_eq!(e.state.konfig["fallback_unklar_min"], json!(0.25));
    assert_eq!(e.state.konfig["fallback_drossel_max"], json!(2));
    assert!(
        e.state.konfig["fallback_frage_geruest"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );
    assert!(
        e.state.konfig["fallback_status_durchsage"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );
}

#[test]
fn fallback_nachfrage_ist_als_learn_journiert_und_exakt_rueckholbar() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.json");
    let actions_path = dir.path().join("actions.json");
    std::fs::write(&state_path, serde_json::to_vec(&fallback_state()).unwrap()).unwrap();
    std::fs::write(&actions_path, b"{\"aktionen\":{}}").unwrap();
    let db = dir.path().join("fallback.db");
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
    let reply = app.message("a b").unwrap();
    assert_eq!(reply.fallback.as_deref(), Some("nachfrage"));
    assert!(reply
        .text
        .contains(&geruest_voll("fallback_frage_geruest", 0, "c")));
    // Fallback events journal as ordinary learn entries (Kap. 14/19)...
    let status = app.status().unwrap();
    assert_eq!(status["journal_letzter"]["art"], "learn");
    // ...and the question never becomes a rateable reply.
    assert_eq!(status["feedback_available"], false);
    // The user's answer runs as a normal learning move and fills kanal 2.
    let reply = app.message("ganz andere sache").unwrap();
    assert_eq!(reply.fallback, None);
    let abschnitt = app.test_abschnitt();
    assert_eq!(
        abschnitt["extra"]["kanal2_letzte"]["ergebnis"]["worte"],
        json!(["ganz", "andere", "sache"])
    );
    // Undo rolls the fallback bookkeeping back exactly.
    let report = app.undo(2).unwrap();
    assert_eq!(report["zurueckgerollt"], 2);
    let abschnitt = app.test_abschnitt();
    assert!(abschnitt["extra"].get("fallback_pro_thema").is_none());
    assert!(abschnitt["extra"].get("fallback_geruest_zaehler").is_none());
    assert!(abschnitt["extra"].get("kanal2_letzte").is_none());
}
