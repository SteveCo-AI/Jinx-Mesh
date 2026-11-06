//! Undo of cold sources and large spans: the bounded-workspace rollback
//! restores vault records exactly across paging and round boundaries.
use super::JOURNAL_PASS;
use serde_json::{json, Value};

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

#[test]
fn undo_runde_grosser_spanne_ueber_rundengrenze_haelt_budget_und_kanten_exakt() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("undo-spanne.db");
    let mut app = crate::App::create_empty(&db, "jinx", JOURNAL_PASS, None, None).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    // 26 blocks of 5 unique words: 130 nodes — more cold sources than one
    // 128-record rollback page, so the undo must batch its store reads.
    let wort = |i: usize, j: usize| {
        let bytes = Sha256::digest(format!("spanne-{i}-{j}"));
        format!(
            "w{}",
            bytes[..8]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        )
    };
    let bloecke: Vec<String> = (0..26)
        .map(|i| (0..5).map(|j| wort(i, j)).collect::<Vec<_>>().join(" "))
        .collect();
    for block in &bloecke {
        app.message(block).unwrap();
    }
    app.message("alpha bravo").unwrap();
    app.message("alpha charlie").unwrap();
    // Exact pre-round weights, read through the frozen inference path —
    // /undo round restores the state from BEFORE the boundary (inclusive).
    drop(app);
    let mut app = crate::App::open(&db).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    app.freeze().unwrap();
    app.message("alpha bravo charlie").unwrap();
    let kante = |blick: &Value, von: &str, nach: &str| {
        blick["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["from"] == von && e["to"] == nach)
            .and_then(|e| e["weight"].as_f64())
            .unwrap()
    };
    let blick = app.graph_view();
    let alt_ab = kante(&blick, "lex:alpha", "lex:bravo");
    let alt_ac = kante(&blick, "lex:alpha", "lex:charlie");
    drop(app);
    let mut app = crate::App::open(&db).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    app.consolidate().unwrap();
    // The round after the boundary retrains every block (weight changes on
    // all 130 words), retrains alpha->bravo, and ends with a fresh block
    // whose new words evict every touched record from the resident slice.
    for block in &bloecke {
        app.message(block).unwrap();
    }
    app.message("alpha bravo").unwrap();
    let frisch: String = (0..5).map(|j| wort(99, j)).collect::<Vec<_>>().join(" ");
    app.message(&frisch).unwrap();
    // /undo round across the boundary: the consolidate entry plus 28 learns,
    // every touched source cold, more than one 128-record page to reload.
    let report = app.undo_runde().unwrap();
    assert_eq!(report["zurueckgerollt"], 29);
    assert_eq!(report["journal_länge"], 28);
    let status = app.status().unwrap();
    assert!(
        status["active_nodes"].as_u64().unwrap() <= 128,
        "Undo must not hydrate the rolled-back span into the resident set"
    );
    // The fresh block's words were created inside the round and must be
    // gone; the 130 retrained words and alpha/bravo/charlie survive.
    assert_eq!(status["neurons"], 133);
    assert_eq!(app.verify_graph_full().unwrap(), 133);
    drop(app);
    // Reopen: the restored alpha->bravo weight and the untouched
    // alpha->charlie edge must be exact in the vault, not cache remnants.
    let mut app = crate::App::open(&db).unwrap();
    app.login("jinx", JOURNAL_PASS, None).unwrap();
    app.freeze().unwrap();
    app.message("alpha bravo charlie").unwrap();
    let blick = app.graph_view();
    assert_eq!(kante(&blick, "lex:alpha", "lex:bravo"), alt_ab);
    assert_eq!(kante(&blick, "lex:alpha", "lex:charlie"), alt_ac);
}
