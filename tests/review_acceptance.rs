//! Independent regression checks for cold graph activation.
use saeugling::App;
use tempfile::TempDir;

const PASSWORD: &str = "Review-only password 2026!";

#[test]
fn long_message_does_not_recreate_already_stored_cold_words() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("review.db");
    let mut app = App::create_empty(&path, "review", PASSWORD, None, None).unwrap();
    app.login("review", PASSWORD, None).unwrap();
    let words: Vec<String> = (0..45).map(|i| format!("reviewword{i}")).collect();
    for word in &words {
        app.message(word).unwrap();
    }
    assert_eq!(app.status().unwrap()["neurons"], 45);
    drop(app);
    let mut app = App::open(&path).unwrap();
    app.login("review", PASSWORD, None).unwrap();
    let reply = app.message(&words.join(" ")).unwrap();
    assert_eq!(
        reply.neue_neuronen, 0,
        "cold words are existing observations"
    );
    assert_eq!(app.status().unwrap()["neurons"], 45);
    drop(app);
    let mut app = App::open(&path).unwrap();
    app.login("review", PASSWORD, None).unwrap();
    assert_eq!(app.status().unwrap()["neurons"], 45);
}

#[test]
fn every_archived_turn_survives_repeated_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("review.db");
    drop(App::create_empty(&path, "review", PASSWORD, None, None).unwrap());
    for turn in 0..4 {
        let mut app = App::open(&path).unwrap();
        app.login("review", PASSWORD, None).unwrap();
        let text = format!("UniqueSource{turn} — Großschreibung!\nzweite Zeile");
        app.message(&text).unwrap();
        let hits = app.recall(&format!("UniqueSource{turn}"), 20).unwrap();
        assert!(hits
            .iter()
            .any(|hit| hit["speaker"] == "user" && hit["text"] == text));
        assert_eq!(app.status().unwrap()["interactions"], turn + 1);
    }
    let mut app = App::open(&path).unwrap();
    app.login("review", PASSWORD, None).unwrap();
    for turn in 0..4 {
        let hits = app.recall(&format!("UniqueSource{turn}"), 20).unwrap();
        assert!(hits.iter().any(|hit| hit["speaker"] == "user"));
    }
}

#[test]
fn graph_larger_than_working_set_can_finish_without_retraining() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("large.db");
    let mut app = App::create_empty(&path, "review", PASSWORD, None, None).unwrap();
    app.login("review", PASSWORD, None).unwrap();
    for index in 0..170 {
        app.message(&format!("chain{index} chain{}", index + 1))
            .unwrap();
    }
    drop(app);
    let mut app = App::open(&path).unwrap();
    app.login("review", PASSWORD, None).unwrap();
    let mut reply = app.message("chain0").unwrap();
    let interactions = app.status().unwrap()["interactions"].clone();
    let mut completed = false;
    for _ in 0..180 {
        let activation = serde_json::to_value(&reply).unwrap()["activation"].clone();
        assert!(activation["nodes_loaded"].as_u64().unwrap() <= 128);
        if activation["phase"] == "complete" {
            completed = true;
            break;
        }
        assert_eq!(activation["can_continue"], true);
        reply = app.continue_message().unwrap();
        assert!(!reply.gelernt);
        assert_eq!(app.status().unwrap()["interactions"], interactions);
    }
    assert!(
        completed,
        "continuation must advance beyond its original working set"
    );
}

#[test]
fn graph_window_only_exposes_resident_graph_during_authenticated_session() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("view.db");
    let mut app = App::create_empty(&path, "review", PASSWORD, None, None).unwrap();
    assert_eq!(app.graph_view()["locked"], true);
    app.login("review", PASSWORD, None).unwrap();
    app.message("hallo jinx").unwrap();
    let view = app.graph_view();
    assert_eq!(view["locked"], false);
    assert!(!view["nodes"].as_array().unwrap().is_empty());
    assert!(view["nodes"].as_array().unwrap().len() <= 128);
    assert!(view["edges"].as_array().unwrap().len() <= 512);
    app.logout();
    assert_eq!(app.graph_view()["locked"], true);
    assert!(app.graph_view()["nodes"].as_array().unwrap().is_empty());
}
