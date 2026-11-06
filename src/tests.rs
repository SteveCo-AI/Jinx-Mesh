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
