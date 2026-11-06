//! Library test suite, split by domain. Shared fixtures (empty engine,
//! journal passphrase, konfig-armed engine) live here; every submodule owns
//! one behavioral block.
use crate::{engine::Engine, model::State};
use serde_json::{json, Value};

mod basics;
mod fallback;
mod feedback;
mod gate;
mod healing;
mod journal;
mod mutation;
mod release;
mod undo;

fn engine() -> Engine {
    Engine::new(State::empty(), json!({"aktionen":{}})).unwrap()
}

const JOURNAL_PASS: &str = "Op-Journal test passphrase 271828!";

fn engine_mit(konfig: &[(&str, Value)]) -> Engine {
    let mut s = State::empty();
    for (key, wert) in konfig {
        s.konfig.insert((*key).into(), wert.clone());
    }
    Engine::new(s, json!({})).unwrap()
}

/// Alle Rust-Quelldateien unter src/, rekursiv: die Modulteilung hat den
/// ausgelieferten Code in Fachverzeichnisse verschoben, deshalb laufen die
/// Verhaltenstext-Scanner über den Baum statt nur über die oberste Ebene.
fn quelldateien() -> Vec<std::path::PathBuf> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stapel = vec![src];
    let mut dateien = Vec::new();
    while let Some(verzeichnis) = stapel.pop() {
        for eintrag in std::fs::read_dir(&verzeichnis).unwrap() {
            let pfad = eintrag.unwrap().path();
            if pfad.is_dir() {
                stapel.push(pfad);
            } else if pfad.extension().is_some_and(|e| e == "rs") {
                dateien.push(pfad);
            }
        }
    }
    dateien
}
