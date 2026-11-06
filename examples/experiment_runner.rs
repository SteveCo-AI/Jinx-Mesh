//! RW-1 Experiment 1: survival of one controlled learning experience under a
//! fixed, reproducible distraction corpus. Measurement runner on a copied
//! vault. Runs only on an explicitly supplied experimental vault copy.
use anyhow::{bail, Result};
use saeugling::App;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Instant,
};
use zeroize::Zeroizing;

/// The comparison vocabulary of the E1 metrics: words of at least four
/// characters, lowercased, split on anything that is not alphanumeric.
fn long_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4)
        .map(str::to_lowercase)
        .collect()
}

/// A config text array within its permitted size range; empty entries are
/// rejected because every message must be a reproducible learning input.
fn text_list(value: &Value, field: &str, min: usize, max: usize) -> Result<Vec<String>> {
    let list = match value.as_array() {
        Some(list) if (min..=max).contains(&list.len()) => list,
        _ => bail!("{field} muss ein Array aus {min} bis {max} Textnachrichten sein"),
    };
    list.iter()
        .map(|v| match v.as_str() {
            Some(s) if !s.trim().is_empty() => Ok(s.to_string()),
            _ => bail!("{field} darf nur nicht leere Textnachrichten enthalten"),
        })
        .collect()
}

/// Cumulative measurement counts after the intro: strictly increasing.
fn cumulative_grid(config: &Value) -> Result<Vec<u64>> {
    let list = match config["grid"].as_array() {
        Some(list) if !list.is_empty() => list,
        _ => bail!("grid muss ein nicht leeres Array kumulativer Zähler sein"),
    };
    let grid: Vec<u64> = list
        .iter()
        .map(|v| match v.as_u64() {
            Some(n) => Ok(n),
            None => bail!("grid darf nur ganze Zahlen enthalten"),
        })
        .collect::<Result<_>>()?;
    if grid.windows(2).all(|pair| pair[0] < pair[1]) {
        Ok(grid)
    } else {
        bail!("grid muss streng steigend sein")
    }
}

/// The running measurement series: every snapshot is appended to results.json
/// immediately, so a crash preserves all measurements taken so far.
struct Experiment {
    dir: PathBuf,
    query: String,
    expected: String,
    summary: Value,
    snapshots: Vec<Value>,
    start: Instant,
}

impl Experiment {
    fn progress(&self, stage: &str, step: u64) -> Result<()> {
        fs::write(
            self.dir.join("progress.json"),
            json!({"stage":stage,"step":step,"elapsed_ms":self.start.elapsed().as_millis()})
                .to_string(),
        )?;
        Ok(())
    }

    /// One measurement: state and graph first, then the probe answer via
    /// read-only inference with the evaluate() assert that nothing persisted.
    fn record(&mut self, app: &mut App, label: Value) -> Result<()> {
        let status = app.status()?;
        let graph = app.graph_view();
        let before = status["interactions"].clone();
        let reply = app.infer_read_only(&self.query)?;
        assert_eq!(before, app.status()?["interactions"]);
        let answer = reply.text.to_lowercase();
        let retrieval = long_words(&self.expected)
            .iter()
            .any(|word| answer.contains(word.as_str()));
        let labels: Vec<&str> = graph["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|node| node["label"].as_str())
            .collect();
        let words: BTreeSet<String> = long_words(&self.query)
            .into_iter()
            .chain(long_words(&self.expected))
            .collect();
        let probe_words_in_graph: BTreeMap<String, bool> = words
            .into_iter()
            .map(|word| {
                let contained = labels.contains(&word.as_str());
                (word, contained)
            })
            .collect();
        let probe_answer = serde_json::to_value(&reply)?;
        let probe_words = serde_json::to_value(&probe_words_in_graph)?;
        let label_text = match label.as_str() {
            Some(text) => text.to_string(),
            None => label.to_string(),
        };
        println!(
            "Messung {label_text}: Abruf {retrieval}, {} aktive Knoten, {} s",
            status["active_nodes"],
            self.start.elapsed().as_secs()
        );
        let snapshot = json!({
            "label": label,
            "status": status,
            "graph": graph,
            "probe_answer": probe_answer,
            "retrieval": retrieval,
            "probe_words_in_graph": probe_words,
        });
        self.snapshots.push(snapshot);
        fs::write(
            self.dir.join("results.json"),
            serde_json::to_vec_pretty(&json!({
                "config_summary": self.summary,
                "snapshots": self.snapshots
            }))?,
        )?;
        Ok(())
    }
}

fn main() -> Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).expect("experiment directory"));
    let config: Value = serde_json::from_slice(&fs::read(dir.join("experiment.json"))?)?;
    let initial_vault = match config["initial_vault"].as_str() {
        Some(path) if !path.trim().is_empty() => dir.join(path.trim()),
        _ => bail!("experiment.json benötigt initial_vault als Pfad zur Template-Vault"),
    };
    if !initial_vault.is_file() {
        bail!("initial_vault fehlt: {}", initial_vault.display());
    }
    let active_vault = Path::new(env!("CARGO_MANIFEST_DIR")).join("private/jinx.db");
    if let (Ok(template), Ok(active)) = (
        fs::canonicalize(initial_vault.as_path()),
        fs::canonicalize(active_vault.as_path()),
    ) {
        if template == active {
            bail!(
                "initial_vault darf nicht der aktive Vault sein: {}",
                active_vault.display()
            );
        }
    }
    let working = dir.join("working.db");
    if working.exists() {
        bail!(
            "working.db existiert bereits in {}: halb gelaufene Experimente werden nicht erneut gestartet",
            dir.display()
        );
    }
    let user_default = match &config["user"] {
        Value::Null => "Lumen".to_string(),
        v => match v.as_str() {
            Some(u) if !u.trim().is_empty() => u.to_string(),
            _ => bail!("user muss ein nicht leerer Text sein"),
        },
    };
    let query = match config["probe"]["query"].as_str() {
        Some(q) if !q.trim().is_empty() => q.to_string(),
        _ => bail!("experiment.json benötigt probe.query als nicht leeren Text"),
    };
    let expected = match config["probe"]["expected"].as_str() {
        Some(e) if !e.trim().is_empty() => e.to_string(),
        _ => bail!("experiment.json benötigt probe.expected als nicht leeren Text"),
    };
    let intro = text_list(&config["intro"], "intro", 1, 20)?;
    // Reinforcement lines go to the app verbatim: a leading +/- rating prefix
    // is meaningful there (the app evaluates the previous answer), so it is
    // neither stripped nor validated here.
    let reinforce = match &config["reinforce"] {
        Value::Null => Vec::new(),
        v => text_list(v, "reinforce", 0, 50)?,
    };
    let distractors = text_list(&config["distractors"], "distractors", 1, 5000)?;
    let grid = cumulative_grid(&config)?;
    let total = grid[grid.len() - 1];
    fs::copy(&initial_vault, &working)?;
    fs::write(
        dir.join("progress.json"),
        json!({"stage":"awaiting_login","step":0}).to_string(),
    )?;
    println!(
        "Jinx-Mesh RW-1 E1: eine kontrollierte Lernerfahrung aus {} Nachrichten, dann {} Ablenkungs-Nachrichten zyklisch aus {} Zeilen, Messraster {:?} auf separater Kopie.\nDein aktiver Vault bleibt unverändert. Ergebnisse bleiben lokal.\n",
        intro.len(),
        total,
        distractors.len(),
        grid
    );
    print!("Nutzername [{user_default}]: ");
    io::stdout().flush()?;
    let mut user = String::new();
    io::stdin().read_line(&mut user)?;
    let user = if user.trim().is_empty() {
        user_default
    } else {
        user.trim().to_owned()
    };
    let password = Zeroizing::new(rpassword::prompt_password("Passwort (verdeckt): ")?);
    let code = Zeroizing::new(rpassword::prompt_password(
        "2FA-Code (leer falls deaktiviert): ",
    )?);
    let mut app = App::open(&working)?;
    if let Err(e) = app.login(
        &user,
        &password,
        (!code.is_empty()).then_some(code.as_str()),
    ) {
        fs::write(
            dir.join("progress.json"),
            json!({"stage":"login_failed","error":e.to_string()}).to_string(),
        )?;
        return Err(e);
    }
    drop(password);
    drop(code);
    let mut experiment = Experiment {
        dir,
        query,
        expected,
        summary: json!({"grid": grid, "intro_count": intro.len(), "distractor_count": distractors.len(), "reinforce_count": reinforce.len()}),
        snapshots: Vec::new(),
        start: Instant::now(),
    };
    experiment.progress("training", 0)?;
    experiment.record(&mut app, json!(0))?;
    for text in &intro {
        app.message(text)?;
        experiment.progress("training", 0)?;
    }
    experiment.record(&mut app, json!(1))?;
    for text in &reinforce {
        app.message(text)?;
        experiment.progress("training", 0)?;
    }
    if !reinforce.is_empty() {
        experiment.record(&mut app, json!("post_reinforce"))?;
    }
    let mut fed: u64 = 0;
    let mut index = 0usize;
    for &n in &grid {
        while fed < n {
            app.message(&distractors[index])?;
            index = (index + 1) % distractors.len();
            fed += 1;
            experiment.progress("training", fed)?;
        }
        experiment.record(&mut app, json!(n))?;
    }
    experiment.progress("complete", total)?;
    println!(
        "Abgeschlossen. Messreihe und Vault-Kopie: {}",
        experiment.dir.display()
    );
    Ok(())
}
