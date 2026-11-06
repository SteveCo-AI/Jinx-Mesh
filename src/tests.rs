use crate::{
    crypto,
    engine::Engine,
    graph,
    model::{Node, State},
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
fn engine() -> Engine {
    Engine::new(State::empty(), json!({"aktionen":{}})).unwrap()
}
#[test]
fn new_model_is_linguistically_empty() {
    let e = engine();
    assert!(e.state.neuronen.is_empty());
    assert!(e.state.kanten.is_empty());
    for key in ["anker", "formulierungen", "lab"] {
        assert_eq!(e.state.extra[key], json!({}));
    }
    for text in [
        "Hallo",
        "wichtig",
        "gut",
        "nein",
        "status",
        "What is a house?",
    ] {
        let r = e.infer(text).unwrap();
        assert_eq!(r.text, "");
        assert_eq!(r.aktion, None);
        assert!(!r.gelernt);
    }
}
#[test]
fn first_message_is_imitation_and_only_heard_words_become_nodes() {
    let mut e = engine();
    let r = e.train("Hallo ich bin Lumen").unwrap();
    assert_eq!(r.text, "hallo ich bin lumen.");
    assert_eq!(e.state.neuronen.len(), 4);
    assert_eq!(r.aktion, None);
    for n in e.state.neuronen.values() {
        assert_eq!(n.typ, "LEX");
    }
    for word in [
        "wichtig",
        "kundenblick",
        "attention",
        "gut",
        "projekt alpha",
    ] {
        assert!(graph::word(&e.state, word).is_none());
    }
    let once = graph::edge(&e.state, "lex:ich", "lex:bin").unwrap();
    e.train("ich bin lumen").unwrap();
    assert!(graph::edge(&e.state, "lex:ich", "lex:bin").unwrap() > once);
    assert!(e.infer("lumen").unwrap().text.contains("lumen"));
}

#[test]
fn repeated_two_word_context_learns_its_own_continuation() {
    let mut e = engine();
    e.train("ich heiße lumen heute").unwrap();
    e.train("ich heiße jinx morgen").unwrap();
    e.train("ich heiße lumen jetzt").unwrap();
    let contexts = &e.state.extra["sequence_contexts"];
    assert_eq!(contexts["ich\u{1f}heiße"]["lumen"], json!(2));
    assert_eq!(contexts["ich\u{1f}heiße"]["jinx"], json!(1));
    // The context table is metadata used for ordering; it is not emitted as text.
    assert!(!e
        .train("ich heiße lumen")
        .unwrap()
        .text
        .contains("sequence_contexts"));
}

#[test]
fn context_counts_change_the_generated_path_and_punctuation_is_retained() {
    let mut e = engine();
    e.train("alpha beta xray").unwrap();
    e.train("alpha beta yellow").unwrap();
    e.train("alpha beta xray").unwrap();
    // Equalize the lexical edge weights: this choice must come from context.
    graph::set_edge(&mut e.state, "lex:beta", "lex:xray", 0.5);
    graph::set_edge(&mut e.state, "lex:beta", "lex:yellow", 0.5);
    let p = graph::path(&e.state, &["lex:alpha".into()]).unwrap();
    assert_eq!(p.nodes.get(2).map(String::as_str), Some("lex:xray"));

    let mut fresh = engine();
    let first = fresh.train("Hallo, Welt!").unwrap();
    assert_eq!(first.text, "hallo, welt!");
    assert!(graph::word(&fresh.state, "hallo").is_some());
    assert!(!fresh.state.neuronen.contains_key("lex:hallo,"));
}

#[test]
fn learned_paths_can_repeat_a_word_when_a_cycle_was_observed() {
    let mut e = engine();
    e.train("a b a").unwrap();
    let p = graph::path(&e.state, &["lex:a".into()]).unwrap();
    assert_eq!(&p.nodes[..3], &["lex:a", "lex:b", "lex:a"]);
    assert!(
        p.nodes.len() <= 4,
        "each lexical node is bounded to two occurrences"
    );
}
#[test]
fn users_actual_introduction_has_no_demo_response() {
    let mut e = engine();
    let r=e.train("Ja, ich weiß aktuell nicht, was ich dir erzählen könnte. Ich stelle mich einfach mal richtig vor! Du bist Jinx, ich bin Lumen und ich mag es, mit künstlicher Intelligenz zu arbeiten. Dabei ist mir wichtig, dass beide Seiten auf Augenhöhe arbeiten.").unwrap();
    assert_eq!(r.aktion, None);
    assert!(!r.text.contains("kundenblick"));
    assert!(!r.text.contains("projekt alpha"));
    assert!(!r.text.contains("attention"));
}
#[test]
fn even_imported_demo_edges_cannot_trigger_actions_or_expose_internal_labels() {
    let mut s = State::empty();
    for (id, typ, label) in [
        ("lex:wichtig", "LEX", "wichtig"),
        ("ring:a", "RING", "kundenblick"),
        ("kern:a", "KERN", "attention"),
    ] {
        s.neuronen.insert(
            id.into(),
            Node {
                id: id.into(),
                typ: typ.into(),
                label: label.into(),
                staerke: 0.5,
                alter: "reif".into(),
                nutzt: 0,
                geboren: 0,
                ring: None,
                extra: BTreeMap::new(),
            },
        );
    }
    graph::set_edge(&mut s, "lex:wichtig", "ring:a", 0.5);
    graph::set_edge(&mut s, "ring:a", "kern:a", 0.6);
    s.extra.insert(
        "formulierungen".into(),
        json!({"nachfrage":[{"text":"vorgefertigte antwort"}]}),
    );
    let mut e = Engine::new(
        s,
        json!({"aktionen":{"attention":{"ergebnis":[{"text":"projekt alpha"}]}}}),
    )
    .unwrap();
    assert_eq!(e.infer("wichtig").unwrap().text, "wichtig");
    let r = e.train("wichtig").unwrap();
    assert_eq!(r.text, "wichtig.");
    assert_eq!(r.aktion, None);
    assert_eq!(e.state.neuronen.len(), 3); // Preserve imported data, never delete identities.
}
#[test]
fn empty_input_does_not_invent_a_response() {
    let mut e = engine();
    let r = e.train("").unwrap();
    assert!(r.text.is_empty());
    assert!(e.state.neuronen.is_empty());
}
#[test]
fn graph_budget_consolidation_and_unicode() {
    let mut e = engine();
    assert_eq!(
        graph::tokens("École Привет 世界 cafe\u{301}"),
        vec!["école", "привет", "世界", "café"]
    );
    let r = e
        .train(
            &(0..100)
                .map(|i| format!("wort{i}"))
                .collect::<Vec<_>>()
                .join(" "),
        )
        .unwrap();
    assert_eq!(r.neue_neuronen, 100);
    let count = e.state.neuronen.len();
    for _ in 0..100 {
        graph::consolidate(&mut e.state);
    }
    assert_eq!(e.state.neuronen.len(), count);
    e.state.validate().unwrap();
}
#[test]
fn encryption_remains_lossless_authenticated_and_randomized() {
    let key = zeroize::Zeroizing::new([7; 32]);
    let text = "data".repeat(2000);
    let a = crypto::seal(&key, text.as_bytes(), b"state").unwrap();
    let b = crypto::seal(&key, text.as_bytes(), b"state").unwrap();
    assert_ne!(a, b);
    assert_eq!(
        &*crypto::unseal(&key, &a, b"state").unwrap(),
        text.as_bytes()
    );
    assert!(crypto::unseal(&key, &a, b"audit").is_err());
}
#[test]
fn malformed_state_rejected() {
    let mut v: Value = serde_json::to_value(State::empty()).unwrap();
    v["kanten"] = json!({"missing":{"x":0.5}});
    assert!(serde_json::from_value::<State>(v)
        .unwrap()
        .validate()
        .is_err());
}

#[test]
fn long_learned_sentence_is_not_cut_after_twelve_words() {
    let mut old = State::empty();
    old.konfig.insert("max_pfad_len".into(), json!(12));
    old.konfig.insert("max_neu_pro_nachricht".into(), json!(12));
    let mut e = Engine::new(old, json!({})).unwrap();
    let mut words: Vec<_> = (0..40).map(|i| format!("langwort{i}")).collect();
    words.last_mut().unwrap().push('.');
    let text = words.join(" ");
    assert_eq!(e.train(&text).unwrap().neue_neuronen, 40);
    assert_eq!(e.infer("langwort0").unwrap().text, text);
}

#[test]
fn observed_internal_markers_do_not_impose_a_hardcoded_stop() {
    let mut e = engine();
    e.train("eins zwei. drei vier.").unwrap();
    assert_eq!(e.infer("eins").unwrap().text, "eins zwei. drei vier.");
}

#[test]
fn punctuation_is_separate_and_missing_end_is_a_default_dot() {
    let mut e = engine();
    e.train("Jinx").unwrap();
    e.train("Jinx!").unwrap();
    e.train("Jinx?").unwrap();
    assert_eq!(e.state.neuronen.len(), 1);
    assert_eq!(
        crate::language::render(
            &crate::language::parse("Hallo").words,
            &crate::language::parse("Hallo").marks
        ),
        "hallo."
    );
    for symbol in [".", "!", "?", ":", ";"] {
        let p = crate::language::parse(&format!("Jinx{symbol}"));
        assert_eq!(p.words, ["jinx"]);
        assert_eq!(p.marks.last().unwrap().symbol, symbol);
    }
}

#[test]
fn feedback_changes_only_the_rated_context_and_never_becomes_vocabulary() {
    let mut e = engine();
    e.train("start red end.").unwrap();
    e.train("start blue end.").unwrap();
    let original = e.train("start?").unwrap().text;
    assert!(original.contains("blue"));
    let reply = e.train("- andere wörter.").unwrap();
    assert_eq!(reply.feedback, Some(-1));
    assert!(e.infer("start?").unwrap().text.contains("red"));
    assert!(e.infer("start!").unwrap().text.contains("blue"));
    let rated = e.infer("start?").unwrap().text;
    crate::language::remember(&mut e.state, "start?", &rated);
    e.train("+ weitere wörter.").unwrap();
    assert!(e.infer("start?").unwrap().text.contains("red"));
    let counts = e.state.extra["feedback_counts"].clone();
    e.train("neutraler neuer satz.").unwrap();
    assert_eq!(e.state.extra["feedback_counts"], counts);
    assert!(!e.state.neuronen.contains_key("lex:+"));
    assert!(!e.state.neuronen.contains_key("lex:-"));
}

#[test]
fn feedback_also_weights_observed_markers_without_question_rules() {
    let mut e = engine();
    e.train("hallo!").unwrap();
    e.train("hallo?").unwrap();
    crate::language::remember(&mut e.state, "gruß", "hallo?");
    e.train("- neuer satz.").unwrap();
    let ids = vec!["lex:hallo".into()];
    assert_eq!(
        crate::language::render_path(&e.state, &ids, &crate::language::context("gruß")),
        "hallo!"
    );
    crate::language::remember(&mut e.state, "gruß", "hallo!");
    e.train("+ anderer satz.").unwrap();
    assert_eq!(
        crate::language::render_path(&e.state, &ids, &crate::language::context("gruß")),
        "hallo!"
    );
}

#[test]
fn old_punctuation_node_is_reused_after_lazy_loading_without_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = State::empty();
    state.neuronen.insert(
        "lex:jinx!".into(),
        Node {
            id: "lex:jinx!".into(),
            typ: "LEX".into(),
            label: "jinx!".into(),
            staerke: 0.2,
            alter: "jung".into(),
            nutzt: 0,
            geboren: 0,
            ring: None,
            extra: BTreeMap::new(),
        },
    );
    let state_path = dir.path().join("state.json");
    let actions_path = dir.path().join("actions.json");
    std::fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    std::fs::write(&actions_path, b"{\"aktionen\":{}}").unwrap();
    let db = dir.path().join("model.db");
    let pass = "Legacy compatibility passphrase!";
    let mut app = crate::App::create(
        &db,
        "review",
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
    app.login("review", pass, None).unwrap();
    let reply = app.message("Jinx?").unwrap();
    assert_eq!(reply.neue_neuronen, 0);
    assert_eq!(reply.text, "jinx?");
    assert_eq!(app.verify_graph_full().unwrap(), 1);
    assert_eq!(app.graph_view()["nodes"][0]["id"], "lex:jinx!");
    drop(app);
    let mut app = crate::App::open(&db).unwrap();
    app.login("review", pass, None).unwrap();
    app.freeze().unwrap();
    assert_eq!(app.message("Jinx").unwrap().text, "jinx?");
}

// ---- Phase-2 Block 1: Op-Journal + exakter Rollback (Kap. 14) ----

const JOURNAL_PASS: &str = "Op-Journal test passphrase 271828!";

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

// ---- Phase-2 Block 2: Anker-Wörter + implizite Feedback-Kanäle (Kap. 18) ----

/// Bias the rating channels leave for one target edge in one context.
fn bias_fuer(e: &Engine, ctx: &str, prev: Option<&str>, wort: &str) -> f64 {
    let kante = serde_json::to_string(&(prev, wort)).unwrap();
    e.state
        .extra
        .get("context_feedback")
        .and_then(|m| m.get(ctx))
        .and_then(|m| m.get(kante))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

fn engine_mit(konfig: &[(&str, Value)]) -> Engine {
    let mut s = State::empty();
    for (key, wert) in konfig {
        s.konfig.insert((*key).into(), wert.clone());
    }
    Engine::new(s, json!({})).unwrap()
}

#[test]
fn ankerwort_bewertet_die_vorherige_antwort_staerker_als_ohne() {
    let ctx = crate::language::context("hallo welt");
    // Identical setup; only the follow-up move differs.
    let mut mit_anker = engine();
    mit_anker.train("hallo welt").unwrap();
    let reply = mit_anker.train("super danke").unwrap();
    assert_eq!(reply.anker.as_deref(), Some("super"));
    let mut ohne_anker = engine();
    ohne_anker.train("hallo welt").unwrap();
    ohne_anker.train("danke sehr").unwrap();
    // The anchor adds its full configured weight on top of the implicit
    // continuation signal (Kap. 18: zusätzlich zum impliziten Anteil).
    let mit = bias_fuer(&mit_anker, &ctx, None, "hallo");
    let ohne = bias_fuer(&ohne_anker, &ctx, None, "hallo");
    assert!((mit - 0.6).abs() < 1e-9);
    assert!((ohne - 0.1).abs() < 1e-9);
    assert!((mit - ohne - 0.5).abs() < 1e-9);

    // The minus root fires symmetrically.
    let mut min_anker = engine();
    min_anker.train("hallo welt").unwrap();
    let reply = min_anker.train("nein danke").unwrap();
    assert_eq!(reply.anker.as_deref(), Some("nein"));
    let bias = bias_fuer(&min_anker, &ctx, None, "hallo");
    assert!(bias < 0.0);
    assert!((bias + 0.4).abs() < 1e-9);

    // Only a standalone token fires, never a substring.
    let mut teilwort = engine();
    teilwort.train("hallo welt").unwrap();
    let reply = teilwort.train("supersache danke").unwrap();
    assert_eq!(reply.anker, None);
    assert!((bias_fuer(&teilwort, &ctx, None, "hallo") - 0.1).abs() < 1e-9);
}

#[test]
fn wiederholte_frage_ohne_anker_wirkt_implizit_negativ() {
    let mut e = engine();
    e.train("was ist los").unwrap();
    let ctx = crate::language::context("was ist los");
    let vor = bias_fuer(&e, &ctx, None, "was");
    let reply = e.train("was ist los").unwrap();
    assert_eq!(reply.anker, None);
    assert_eq!(reply.feedback, None);
    // One negative rating step is visible in the bias values...
    let nach = bias_fuer(&e, &ctx, None, "was");
    assert!((nach - vor + 0.25).abs() < 1e-9);
    // ...and in the maturing window, but never in the explicit counters.
    assert!(!e.state.extra.contains_key("feedback_counts"));
    assert_eq!(
        e.state.extra["anker_maturing"]["super"]["fenster"],
        json!(["negativ"])
    );
    assert_eq!(
        e.state.extra["anker_maturing"]["super"]["negativ"],
        json!(1)
    );
    assert_eq!(
        e.state.extra["anker_maturing"]["nein"]["fenster"],
        json!(["negativ"])
    );
    // A new question continues positively instead.
    e.train("ganz andere sache").unwrap();
    assert_eq!(
        e.state.extra["anker_maturing"]["super"]["positiv"],
        json!(1)
    );
    assert_eq!(
        e.state.extra["anker_maturing"]["super"]["fenster"],
        json!(["negativ", "positiv"])
    );
}

#[test]
fn anker_reifung_nach_50_konformen_faellen_senkt_gewicht_und_protokolliert() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("anker-reifung.db");
    let mut app = crate::App::create_empty(&db, "jinx", JOURNAL_PASS, None, None).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    app.message("eins zwei").unwrap();
    // 50 konforme Fälle: every continuation without an anchor word and
    // without an explicit rating is an implicit positive signal for the
    // plus root.
    for i in 0..50 {
        app.message(&format!("novum{i} wechsel{i}")).unwrap();
    }
    let abschnitt = app.test_abschnitt();
    assert_eq!(abschnitt["extra"]["anker_gewicht"], json!(0.45));
    assert_eq!(
        abschnitt["extra"]["anker_maturing"]["super"]["positiv"],
        json!(50)
    );
    assert_eq!(
        abschnitt["extra"]["anker_maturing"]["super"]["fenster"]
            .as_array()
            .unwrap()
            .len(),
        50
    );
    // The maturation is protolled as a feedback journal entry (Kap. 14/18).
    let status = app.status().unwrap();
    assert_eq!(status["journal_letzter"]["art"], "feedback");
    // And it is exactly rollbackable: the learned override disappears and
    // the weight falls back to the configured start value.
    let report = app.undo(1).unwrap();
    assert_eq!(report["zurueckgerollt"], 1);
    assert_eq!(app.test_abschnitt()["extra"]["anker_gewicht"], Value::Null);
}

#[test]
fn ankerwoerter_sind_regulaere_lex_knoten_und_lernen_wie_alle_anderen() {
    let mut e = engine();
    let reply = e.train("super sache").unwrap();
    assert_eq!(reply.anker.as_deref(), Some("super"));
    {
        let node = &e.state.neuronen["lex:super"];
        assert_eq!(node.typ, "LEX");
        assert_eq!(node.label, "super");
        assert!(node.ring.is_none());
        // No special anchor status anywhere in the node: only the ordinary
        // source bookkeeping every learned token carries.
        assert_eq!(
            node.extra.keys().map(String::as_str).collect::<Vec<_>>(),
            ["source_episode_ids"]
        );
    }
    assert_eq!(graph::word(&e.state, "super").as_deref(), Some("lex:super"));
    // And it learns like every other token: hear raises the strength, word
    // pairs form edges.
    let staerke = e.state.neuronen["lex:super"].staerke;
    e.train("super sache").unwrap();
    assert!(e.state.neuronen["lex:super"].staerke > staerke);
    assert!(graph::edge(&e.state, "lex:super", "lex:sache").is_some());
}

#[test]
fn anker_erkennung_folgt_der_konfig_nicht_dem_code() {
    let ctx = crate::language::context("hallo welt");
    // Default konfig data: "super" is the plus root.
    let mut a = engine();
    a.train("hallo welt").unwrap();
    let reply = a.train("super danke").unwrap();
    assert_eq!(reply.anker.as_deref(), Some("super"));
    assert!((bias_fuer(&a, &ctx, None, "hallo") - 0.6).abs() < 1e-9);

    // Editing the konfig data redirects the mechanism: "super" becomes
    // ordinary vocabulary, the configured word fires instead.
    let mut b = engine_mit(&[("anker_plus", json!("toll"))]);
    b.train("hallo welt").unwrap();
    let reply = b.train("super danke").unwrap();
    assert_eq!(reply.anker, None);
    assert!((bias_fuer(&b, &ctx, None, "hallo") - 0.1).abs() < 1e-9);

    let mut c = engine_mit(&[("anker_plus", json!("toll"))]);
    c.train("hallo welt").unwrap();
    let reply = c.train("toll danke").unwrap();
    assert_eq!(reply.anker.as_deref(), Some("toll"));
    assert!((bias_fuer(&c, &ctx, None, "hallo") - 0.6).abs() < 1e-9);
}

#[test]
fn kanal2_bleibt_in_diesem_block_ein_unbefuellter_datenslot() {
    let mut e = engine();
    e.train("hallo welt").unwrap();
    e.train("super danke").unwrap();
    // The result-value channel keeps its data slot (kanal2_letzte); Block 3
    // fills it from usage. No values appear yet.
    assert_eq!(e.state.extra["kanal2_letzte"], json!({}));
    assert!(e.state.extra["kanal2_letzte"]
        .as_object()
        .unwrap()
        .is_empty());
}

#[test]
fn anker_konfig_wird_validiert_und_fehlende_keys_fallen_auf_die_startwerte_zurueck() {
    // New konfig keys are range-checked (single-token anchor words, weight
    // bands, maturation thresholds, floor below the start weight).
    let mut s = State::empty();
    s.konfig.insert("anker_gewicht".into(), json!(5.0));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert("anker_gewicht".into(), json!(0.1));
    s.konfig.insert("anker_gewicht_min".into(), json!(0.5));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert("anker_plus".into(), json!("zwei woerter"));
    assert!(s.validate().is_err());
    let mut s = State::empty();
    s.konfig.insert("anker_minus".into(), json!("super"));
    assert!(s.validate().is_err());

    // Vault compatibility: a state without the Kap.-18 keys (every vault
    // predating the mechanism) still validates and loads...
    let mut s = State::empty();
    for key in [
        "anker_gewicht",
        "anker_gewicht_min",
        "anker_minus",
        "anker_plus",
        "anker_reif_fenster",
        "anker_reif_schritt",
        "anker_reif_ueber_einstimmig",
        "implizit_positiv",
    ] {
        s.konfig.remove(key);
    }
    assert!(s.validate().is_ok());
    // ...and Engine::new restores the configured defaults from the fixture
    // state, so the mechanism is complete without code-side words.
    let mut e = Engine::new(s, json!({"aktionen":{}})).unwrap();
    assert_eq!(e.state.konfig["anker_plus"], json!("super"));
    assert_eq!(e.state.konfig["anker_minus"], json!("nein"));
    assert_eq!(e.state.konfig["anker_gewicht"], json!(0.5));
    e.train("hallo welt").unwrap();
    let reply = e.train("super danke").unwrap();
    assert_eq!(reply.anker.as_deref(), Some("super"));
}

#[test]
fn undo_stellt_kanten_kalter_quellen_auch_im_vault_exakt_wieder_her() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("anker-kalt.db");
    let mut app = crate::App::create_empty(&db, "jinx", JOURNAL_PASS, None, None).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    // The repeated question makes the implicit negative signal mark the
    // start word, so consolidation later reweights its edge WITHOUT a
    // neuron op; a following move then evicts both words from the resident
    // working set.
    for _ in 0..3 {
        app.message("wort eins").unwrap();
    }
    app.consolidate().unwrap();
    app.message("neue sache").unwrap();
    app.undo_runde().unwrap();
    // After reopening, the rolled-back edge of the cold source must be the
    // stored old value in the vault — not the consolidation value.
    drop(app);
    let mut app = crate::App::open(&db).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    app.freeze().unwrap();
    app.message("wort eins").unwrap();
    let abschnitt = app.test_abschnitt();
    assert_eq!(abschnitt["kanten"]["lex:wort"]["lex:eins"], json!(0.4));
    assert_eq!(abschnitt["neuronen"]["lex:wort"]["nutzt"], json!(1));
}

// ---- Phase-2 Block 3: Unsicherheits-Fallback (Kap. 19) ----

/// Pair key of the rating channels for one context-feedback entry.
fn paar(prev: Option<&str>, wort: &str) -> String {
    serde_json::to_string(&(prev, wort)).unwrap()
}

/// Kap.-19 mechanism fixture: a small graph where "a b" walks the answer
/// path "a b c", with contradictory context-feedback ratings in the "a b"
/// theme (positive on the c step, negative on the input→answer pair a→c)
/// and a second, independent "b a" theme with its own contradiction.
fn fallback_state() -> State {
    let mut s = State::empty();
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
    let mut e = engine();
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
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for eintrag in std::fs::read_dir(&src).unwrap() {
        let pfad = eintrag.unwrap().path();
        if pfad.extension().is_none_or(|e| e != "rs") {
            continue;
        }
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
fn die_nachfrage_wird_nie_zum_bewertungs_oder_reifungsziel() {
    let mut e = Engine::new(fallback_state(), json!({})).unwrap();
    e.train("a b").unwrap(); // question appended; no rateable target remains
    assert!(!e.state.extra.contains_key("feedback_target"));
    let feedback_vorher = e.state.extra["context_feedback"].clone();
    let maturing_vorher = e
        .state
        .extra
        .get("anker_maturing")
        .cloned()
        .unwrap_or(json!({}));
    // Repeating the question as the answer — normally an implicit negative
    // on the previous response — rates nothing here and matures nothing.
    let reply = e.train("a b").unwrap();
    assert_eq!(reply.fallback.as_deref(), Some("nachfrage")); // still unclear, asked again
    assert_eq!(e.state.extra["context_feedback"], feedback_vorher);
    assert_eq!(
        e.state
            .extra
            .get("anker_maturing")
            .cloned()
            .unwrap_or(json!({})),
        maturing_vorher
    );
    assert!(!e.state.extra.contains_key("feedback_target"));
    // An explicit +/- on the question fails honestly: nothing rateable exists.
    assert!(e.train("+ doch").is_err());
    // An anchor word in the answer still learns as an ordinary node and
    // counts its use, but with no target it rates nothing and matures nothing.
    e.train("super vielleicht").unwrap();
    assert_eq!(
        e.state.extra["anker_maturing"]["super"]["nutzungen"],
        json!(1)
    );
    assert_eq!(
        e.state.extra["anker_maturing"]["super"]["fenster"],
        json!([])
    );
    assert!(graph::word(&e.state, "super").is_some());
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

    // Same with a configured anchor word (its token legitimately becomes part
    // of the mirrored answer — the point is that no utterance is appended).
    let mut s = fallback_state();
    s.extra.insert(
        "feedback_target".into(),
        crate::language::target("x y", "x y."),
    );
    let mut e = Engine::new(s, json!({})).unwrap();
    let reply = e.train("super a b").unwrap();
    assert_eq!(reply.anker.as_deref(), Some("super"));
    assert_eq!(reply.fallback, None);
    for key in ["fallback_frage_geruest", "fallback_status_durchsage"] {
        let varianten = State::empty().konfig[key].as_array().unwrap().len();
        for variante in 0..varianten {
            assert!(!reply.text.contains(&geruest_voll(key, variante, "super")));
        }
    }
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
    // predating the mechanism) still validates...
    let mut s = State::empty();
    for key in [
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

/// Behavior texts from the konfig fixture (Kap. 20): the anchor words plus
/// every configured fallback scaffold and status announcement, split around
/// the `{wort}` placeholder so a code literal with the placeholder filled
/// in is caught as well.
struct Verhaltenstexte {
    worte: Vec<String>,
    texte: Vec<String>,
    fragmente: Vec<(String, String)>,
}

fn verhaltenstexte() -> Verhaltenstexte {
    let konfig = State::empty().konfig;
    let mut worte = Vec::new();
    for key in ["anker_plus", "anker_minus"] {
        if let Some(wort) = konfig[key].as_str() {
            worte.push(wort.to_lowercase());
        }
    }
    let mut texte = Vec::new();
    let mut fragmente = Vec::new();
    for key in ["fallback_frage_geruest", "fallback_status_durchsage"] {
        for variante in konfig[key]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
        {
            texte.push(variante.to_lowercase());
            if let Some((vor, nach)) = variante.split_once("{wort}") {
                let vor = vor.trim().to_lowercase();
                let nach = nach.trim().to_lowercase();
                if !vor.is_empty() && !nach.is_empty() {
                    fragmente.push((vor, nach));
                }
            }
        }
    }
    Verhaltenstexte {
        worte,
        texte,
        fragmente,
    }
}

/// Whether position i opens a char literal (`'x'`, `'\n'`, `'…'`) rather
/// than a lifetime (`'a`, `'static`): an escape or a closing quote two chars
/// ahead. The codebase has no raw or byte string literals and no other
/// quote form this heuristic would misread.
fn ist_zeichenliteral(q: &[char], i: usize) -> bool {
    q.get(i) == Some(&'\'') && (q.get(i + 1) == Some(&'\\') || q.get(i + 2) == Some(&'\''))
}

/// Whether the source at position i starts with the marker text.
fn ab_hier(q: &[char], i: usize, marker: &str) -> bool {
    let marker: Vec<char> = marker.chars().collect();
    i + marker.len() <= q.len() && q[i..i + marker.len()] == marker[..]
}

/// Index just past the `}` matching the `{` at `start`, string/char/comment
/// aware.
fn schliessende_klammer(q: &[char], start: usize) -> usize {
    let mut tiefe = 0usize;
    let mut i = start;
    while i < q.len() {
        match q[i] {
            '"' => {
                i += 1;
                while i < q.len() {
                    if q[i] == '\\' {
                        i += 2;
                        continue;
                    }
                    let schliesst = q[i] == '"';
                    i += 1;
                    if schliesst {
                        break;
                    }
                }
            }
            '\'' if ist_zeichenliteral(q, i) => {
                i += 1;
                while i < q.len() {
                    if q[i] == '\\' {
                        i += 2;
                        continue;
                    }
                    let schliesst = q[i] == '\'';
                    i += 1;
                    if schliesst {
                        break;
                    }
                }
            }
            '/' if q.get(i + 1) == Some(&'/') => {
                while i < q.len() && q[i] != '\n' {
                    i += 1;
                }
            }
            '/' if q.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < q.len() && !(q[i] == '*' && q[i + 1] == '/') {
                    i += 1;
                }
                i += 1;
            }
            '{' => {
                tiefe += 1;
                i += 1;
            }
            '}' => {
                tiefe = tiefe.saturating_sub(1);
                i += 1;
                if tiefe == 0 {
                    return i;
                }
            }
            _ => i += 1,
        }
    }
    q.len()
}

/// Live production source: comments stripped and `#[cfg(test)]`
/// mod/impl/fn bodies removed, so the literal scan sees only shipped code.
fn produkt_code(inhalt: &str) -> String {
    let q: Vec<char> = inhalt.chars().collect();
    let mut raus = String::new();
    let mut i = 0usize;
    while i < q.len() {
        if q[i] == '"' {
            raus.push('"');
            i += 1;
            while i < q.len() {
                if q[i] == '\\' && i + 1 < q.len() {
                    raus.push(q[i]);
                    raus.push(q[i + 1]);
                    i += 2;
                    continue;
                }
                raus.push(q[i]);
                let schliesst = q[i] == '"';
                i += 1;
                if schliesst {
                    break;
                }
            }
            continue;
        }
        if ist_zeichenliteral(&q, i) {
            raus.push('\'');
            i += 1;
            while i < q.len() {
                if q[i] == '\\' && i + 1 < q.len() {
                    raus.push(q[i]);
                    raus.push(q[i + 1]);
                    i += 2;
                    continue;
                }
                raus.push(q[i]);
                let schliesst = q[i] == '\'';
                i += 1;
                if schliesst {
                    break;
                }
            }
            continue;
        }
        if q[i] == '/' && q.get(i + 1) == Some(&'/') {
            while i < q.len() && q[i] != '\n' {
                i += 1;
            }
            raus.push(' ');
            continue;
        }
        if q[i] == '/' && q.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < q.len() && !(q[i] == '*' && q[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(q.len());
            raus.push(' ');
            continue;
        }
        if q[i] == '#' && ab_hier(&q, i, "#[cfg(test)]") {
            i += "#[cfg(test)]".len();
            // Only a following mod/impl/fn body is test code; a field or a
            // bare block stays live.
            let mut j = i;
            while j < q.len() && q[j].is_whitespace() {
                j += 1;
            }
            let kopf_anfang = j;
            while j < q.len() && !matches!(q[j], '{' | ';' | ',') {
                j += 1;
            }
            let kopf: String = q[kopf_anfang..j.min(q.len())].iter().collect();
            if j < q.len()
                && q[j] == '{'
                && ["mod ", "impl ", "fn "].iter().any(|m| kopf.contains(m))
            {
                i = schliessende_klammer(&q, j);
                raus.push(' ');
                continue;
            }
            raus.push(' ');
            continue;
        }
        raus.push(q[i]);
        i += 1;
    }
    raus
}

/// The call name whose direct argument the literal at the opening quote is:
/// the identifier before the `(` right in front of it (`bail!`, `println!`,
/// `anyhow::anyhow!`, `.context`). Empty when the literal is embedded in a
/// larger expression.
fn aufruf_davor(q: &[char], anfuehrung: usize) -> String {
    let mut i = anfuehrung;
    while i > 0 && q[i - 1].is_whitespace() {
        i -= 1;
    }
    if i == 0 || q[i - 1] != '(' {
        return String::new();
    }
    i -= 1;
    if i > 0 && q[i - 1] == '!' {
        i -= 1;
    }
    let mut name: Vec<char> = Vec::new();
    while i > 0 && (q[i - 1].is_alphanumeric() || q[i - 1] == '_') {
        name.push(q[i - 1]);
        i -= 1;
    }
    name.iter().rev().collect()
}

/// All string literals of the cleaned source with the call name whose direct
/// argument each literal is (empty when embedded in a larger expression).
fn literale(code: &str) -> Vec<(String, String)> {
    let q: Vec<char> = code.chars().collect();
    let mut funde = Vec::new();
    let mut i = 0usize;
    while i < q.len() {
        if q[i] == '\'' && ist_zeichenliteral(&q, i) {
            i += 1;
            while i < q.len() {
                if q[i] == '\\' {
                    i += 2;
                    continue;
                }
                let schliesst = q[i] == '\'';
                i += 1;
                if schliesst {
                    break;
                }
            }
            continue;
        }
        if q[i] != '"' {
            i += 1;
            continue;
        }
        let anfang = i + 1;
        i += 1;
        while i < q.len() {
            if q[i] == '\\' {
                i += 2;
                continue;
            }
            if q[i] == '"' {
                break;
            }
            i += 1;
        }
        let text: String = q[anfang..i.min(q.len())].iter().collect();
        i += 1;
        funde.push((text, aufruf_davor(&q, anfang - 1)));
    }
    funde
}

/// Call contexts whose direct string arguments are the allowed Kap.-20
/// exception category: error, validation, and terminal system messages.
/// They are honest system output, never model answers — but the configured
/// behavior texts may not hide inside them either.
const MELDUNGS_KONTEXT: [&str; 11] = [
    "bail", "anyhow", "panic", "expect", "context", "println", "print", "eprintln", "eprint",
    "write", "writeln",
];

/// Kap.-20 check of one literal: a violation is (1) a complete sentence of
/// more than three words — exempt inside the message call contexts — or
/// (2) any configured behavior text (anchor word, question scaffold, status
/// announcement), raw or with the `{wort}` placeholder filled in — never
/// exempt.
fn ist_verhaltenstraeger(text: &str, kontext: &str, texte: &Verhaltenstexte) -> Option<String> {
    let klein = text.to_lowercase();
    if let Some(wort) = texte.worte.iter().find(|w| klein.contains(w.as_str())) {
        return Some(format!("ankerwort {wort} im literal: {text}"));
    }
    if let Some(t) = texte.texte.iter().find(|t| klein.contains(t.as_str())) {
        return Some(format!("konfig-text {t} im literal: {text}"));
    }
    if let Some((vor, nach)) = texte
        .fragmente
        .iter()
        .find(|(vor, nach)| klein.contains(vor.as_str()) && klein.contains(nach.as_str()))
    {
        return Some(format!(
            "konfig-geruest mit eingesetztem wort im literal: {text} ({vor} … {nach})"
        ));
    }
    if !MELDUNGS_KONTEXT.contains(&kontext)
        && text.split_whitespace().count() > 3
        && text.trim_end().ends_with(['.', '!', '?', '…'])
    {
        return Some(format!("verhaltenssatz im literal: {text}"));
    }
    None
}

/// All Kap.-20 violations in one Rust source file's content.
fn verletzungen_in(inhalt: &str, texte: &Verhaltenstexte) -> Vec<String> {
    let code = produkt_code(inhalt);
    literale(&code)
        .into_iter()
        .filter_map(|(text, kontext)| ist_verhaltenstraeger(&text, &kontext, texte))
        .collect()
}

#[test]
fn test_anti_hardcode() {
    // Kap. 20: formulations, triggers, anchors, and thresholds never live as
    // code strings — only as konfig data or learned state. The scan reads
    // every shipped src/*.rs file (the test file itself is the test filter
    // exception) and fails loudly on any behavior carrier.
    let texte = verhaltenstexte();
    assert!(texte.worte.iter().all(|w| !w.is_empty()));
    assert!(texte.texte.len() >= 4);
    assert!(texte.fragmente.len() >= 4);
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut verletzungen = Vec::new();
    for eintrag in std::fs::read_dir(&src).unwrap() {
        let pfad = eintrag.unwrap().path();
        if pfad.extension().is_none_or(|e| e != "rs")
            || pfad.file_name().is_some_and(|name| name == "tests.rs")
        {
            continue;
        }
        let inhalt = std::fs::read_to_string(&pfad).unwrap();
        for verletzung in verletzungen_in(&inhalt, &texte) {
            verletzungen.push(format!("{}: {verletzung}", pfad.display()));
        }
    }
    assert!(verletzungen.is_empty(), "\n{}", verletzungen.join("\n"));
}

#[test]
fn test_anti_hardcode_greift_beim_simulierten_verhaltensstring() {
    // (d) Test against test: the scanner fires on simulated violations and
    // spares the allowed exception categories — proving the guard itself
    // works, not just that today's code is clean.
    let texte = verhaltenstexte();
    // A configured question scaffold as a code literal — composed here from
    // the konfig data so this test file itself stays free of the text (the
    // Block-3 scan reads every src file, including this one)...
    let geruest = State::empty().konfig["fallback_frage_geruest"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let snippet = format!(r#"let text = "{geruest}";"#);
    assert!(!verletzungen_in(&snippet, &texte).is_empty());
    // ...an anchor word...
    assert!(!verletzungen_in(r#"let wort = "super";"#, &texte).is_empty());
    // ...a scaffold with the placeholder filled in...
    assert!(!verletzungen_in(
        r#"let text = "ich bin mir bei kaffee noch unsicher";"#,
        &texte
    )
    .is_empty());
    // ...and a complete behavior sentence...
    assert!(!verletzungen_in(
        r#"let text = "dies ist ein vollstaendiger verhaltenssatz.";"#,
        &texte
    )
    .is_empty());
    // Exceptions stay clean: an error message...
    assert!(verletzungen_in(
        r#"bail!("nachricht ueberschreitet das aktivierungsbudget von 128 woertern");"#,
        &texte
    )
    .is_empty());
    // ...a terminal system notice...
    assert!(verletzungen_in(
        r#"println!("systemhinweis: keine antwort konnte gebildet werden.");"#,
        &texte
    )
    .is_empty());
    // ...a command name...
    assert!(verletzungen_in(r#"let cmd = "/undo round";"#, &texte).is_empty());
    // ...and short mechanical identifiers.
    assert!(verletzungen_in(r#"let art = "integration_reject";"#, &texte).is_empty());
    // A full simulated source file: comments and #[cfg(test)] bodies are
    // the test filter, live code is not.
    let datei = concat!(
        "fn live() {\n",
        "    // ein kommentar bleibt ein kommentar\n",
        "    let a = \"kurz\";\n",
        "    let b = \"super\";\n",
        "}\n",
        "#[cfg(test)]\n",
        "mod tests {\n",
        "    fn t() { let c = \"super aus dem test\"; }\n",
        "}\n",
    );
    let verletzungen = verletzungen_in(datei, &texte);
    // Only the live "super" remains: the comment was stripped and the
    // #[cfg(test)] body never ships.
    assert_eq!(verletzungen.len(), 1);
    assert!(verletzungen[0].contains("super"));
    assert!(verletzungen[0].contains("im literal"));
}
