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
    assert_eq!(r.text, "hallo ich bin lumen");
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
    assert!(graph::word(&fresh.state, "hallo,").is_some());
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
    for r in [e.infer("wichtig").unwrap(), e.train("wichtig").unwrap()] {
        assert_eq!(r.text, "wichtig");
        assert_eq!(r.aktion, None);
    }
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
    assert!(r.neue_neuronen <= 12);
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
