//! Explicit feedback and repetition channel (Kap. 18): ordinary anchor
//! vocabulary, the implicit negative of repeated questions, and feedback
//! journaling as its own art.
use super::{engine, JOURNAL_PASS};
use crate::{engine::Engine, graph, model::State};
use serde_json::{json, Value};

// ---- Phase-2 Block 2: Explizites Feedback + Wiederholungs-Kanal (Kap. 18) ----

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

#[test]
fn ankerwoerter_sind_gewoehnliche_woerter_und_loesen_keine_bewertung_aus() {
    let ctx = crate::language::context("hallo welt");
    // V01 (owner decision, binding): the former anchor words are ordinary
    // words. Appearing in a normal message rates nothing through any
    // channel — neither the plus root...
    let mut e = engine();
    e.train("hallo welt").unwrap();
    let reply = e.train("super danke").unwrap();
    assert_eq!(reply.feedback, None);
    assert!((bias_fuer(&e, &ctx, None, "hallo") - 0.0).abs() < 1e-9);
    // ...nor the minus root.
    let reply = e.train("nein danke").unwrap();
    assert_eq!(reply.feedback, None);
    assert!((bias_fuer(&e, &ctx, None, "hallo") - 0.0).abs() < 1e-9);
    // A plain continuation is not a signal either (the implicit positive
    // channel is gone): only a repeated question rates.
    let mut fortsetzung = engine();
    fortsetzung.train("hallo welt").unwrap();
    fortsetzung.train("danke sehr").unwrap();
    assert!((bias_fuer(&fortsetzung, &ctx, None, "hallo") - 0.0).abs() < 1e-9);
    // The word itself learns as ordinary vocabulary; no anchor bookkeeping
    // key ever appears.
    assert!(graph::word(&e.state, "super").is_some());
    assert!(!e.state.extra.contains_key("anker_maturing"));
    assert!(!e.state.extra.contains_key("anker_gewicht"));
    assert!(!e.state.extra.contains_key("feedback_counts"));
}

#[test]
fn wiederholte_frage_wirkt_implizit_negativ() {
    let mut e = engine();
    e.train("was ist los").unwrap();
    let ctx = crate::language::context("was ist los");
    let vor = bias_fuer(&e, &ctx, None, "was");
    let reply = e.train("was ist los").unwrap();
    assert_eq!(reply.feedback, None);
    // One negative rating step is visible in the bias values...
    let nach = bias_fuer(&e, &ctx, None, "was");
    assert!((nach - vor + 0.25).abs() < 1e-9);
    // ...but never in the explicit counters.
    assert!(!e.state.extra.contains_key("feedback_counts"));
    // A new question is no signal at all: the bias stays as it is and no
    // implicit bookkeeping key appears.
    e.train("ganz andere sache").unwrap();
    assert!((bias_fuer(&e, &ctx, None, "was") - nach).abs() < 1e-9);
    assert!(!e.state.extra.contains_key("anker_maturing"));
    assert!(!e.state.extra.contains_key("feedback_counts"));
}

#[test]
fn explizite_bewertung_journiert_als_feedback_und_rollt_exakt_zurueck() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("feedback-art.db");
    let mut app = crate::App::create_empty(&db, "jinx", JOURNAL_PASS, None, None).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    app.message("eins zwei").unwrap();
    // 50 continuation moves stay plain learn entries: the implicit positive
    // channel is gone, nothing matures, nothing reweights.
    for i in 0..50 {
        app.message(&format!("novum{i} wechsel{i}")).unwrap();
    }
    let abschnitt = app.test_abschnitt();
    assert!(abschnitt["extra"].get("anker_gewicht").is_none());
    assert!(abschnitt["extra"].get("anker_maturing").is_none());
    assert_eq!(app.status().unwrap()["journal_letzter"]["art"], "learn");
    // An explicit + rating journals as art "feedback"...
    app.message("+ weiter so").unwrap();
    assert_eq!(app.status().unwrap()["journal_letzter"]["art"], "feedback");
    // ...and rolls back exactly; the previous learn step is the last one
    // again.
    let report = app.undo(1).unwrap();
    assert_eq!(report["zurueckgerollt"], 1);
    assert_eq!(app.status().unwrap()["journal_letzter"]["art"], "learn");
}

#[test]
fn ankerwoerter_sind_regulaere_lex_knoten_und_lernen_wie_alle_anderen() {
    let mut e = engine();
    e.train("super sache").unwrap();
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
fn alte_vaults_mit_ankerdaten_laden_weiter_und_bleiben_inert() {
    // V01: the anchor mechanism is gone. Vaults written before the decision
    // carry its konfig keys and learned extra data — both stay loadable
    // (serde's flattened extra tolerates them) and nothing reads or writes
    // them anymore.
    let mut s = State::empty();
    for (key, wert) in [
        ("anker_plus", json!("super")),
        ("anker_minus", json!("nein")),
        ("anker_gewicht", json!(0.5)),
        ("anker_gewicht_min", json!(0.0)),
        ("anker_reif_fenster", json!(50)),
        ("anker_reif_schritt", json!(0.05)),
        ("anker_reif_ueber_einstimmig", json!(0.85)),
        ("implizit_positiv", json!(0.1)),
    ] {
        s.konfig.insert(key.into(), wert);
    }
    s.extra.insert(
        "anker_maturing".into(),
        json!({"super": {"positiv": 3, "negativ": 0, "nutzungen": 2, "fenster": ["positiv"]}}),
    );
    s.extra.insert("anker_gewicht".into(), json!(0.45));
    assert!(s.validate().is_ok());
    let mut e = Engine::new(s, json!({"aktionen":{}})).unwrap();
    let ctx = crate::language::context("hallo welt");
    e.train("hallo welt").unwrap();
    // The legacy anchor word behaves like every ordinary word: no rating.
    let reply = e.train("super danke").unwrap();
    assert_eq!(reply.feedback, None);
    assert!((bias_fuer(&e, &ctx, None, "hallo") - 0.0).abs() < 1e-9);
    // The legacy data survives untouched: nothing writes it anymore.
    assert_eq!(
        e.state.extra["anker_maturing"]["super"]["nutzungen"],
        json!(2)
    );
    assert_eq!(e.state.extra["anker_gewicht"], json!(0.45));
    assert!(e.state.validate().is_ok());
}
