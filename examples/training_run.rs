//! Runs only on an explicitly supplied experimental vault copy.
use anyhow::{bail, Result};
use saeugling::App;
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    time::Instant,
};
use zeroize::Zeroizing;

fn evaluate(app: &mut App, questions: &[Value], step: usize) -> Result<Value> {
    let before = app.status()?["interactions"].clone();
    let mut answers = Vec::new();
    for question in questions {
        let query = question["query"].as_str().unwrap();
        let reply = app.infer_read_only(query)?;
        answers.push(json!({"query":query,"category":question["category"],"answer":reply.text,"activation":reply.activation}));
    }
    assert_eq!(before, app.status()?["interactions"]);
    Ok(json!({"step":step,"status":app.status()?,"answers":answers}))
}
fn main() -> Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).expect("experiment directory"));
    let config: Value = serde_json::from_slice(&fs::read(dir.join("training.json"))?)?;
    let messages = config["messages"].as_array().unwrap();
    if messages.len() != 1000 {
        bail!("Exactly 1000 training messages required");
    }
    fs::write(
        dir.join("progress.json"),
        json!({"stage":"awaiting_login","step":0}).to_string(),
    )?;
    println!("Jinx-Mesh: 1000 Trainingsnachrichten auf separater Kopie.\nDein aktiver Vault bleibt unverändert. Ergebnisse bleiben lokal.\n");
    print!("Nutzername [Lumen]: ");
    io::stdout().flush()?;
    let mut user = String::new();
    io::stdin().read_line(&mut user)?;
    let user = if user.trim().is_empty() {
        "Lumen"
    } else {
        user.trim()
    };
    let password = Zeroizing::new(rpassword::prompt_password("Passwort (verdeckt): ")?);
    let code = Zeroizing::new(rpassword::prompt_password(
        "2FA-Code (leer falls deaktiviert): ",
    )?);
    let mut app = App::open(&dir.join("working.db"))?;
    if let Err(e) = app.login(user, &password, (!code.is_empty()).then_some(code.as_str())) {
        fs::write(
            dir.join("progress.json"),
            json!({"stage":"login_failed","error":e.to_string()}).to_string(),
        )?;
        return Err(e);
    }
    drop(password);
    drop(code);
    let start = Instant::now();
    let questions = config["questions"].as_array().unwrap();
    let mut evaluations = vec![evaluate(&mut app, questions, 0)?];
    let mut turns = Vec::new();
    let initial = app.status()?["interactions"].as_u64().unwrap();
    for (index, text) in messages.iter().enumerate() {
        let step = index + 1;
        let turn_start = Instant::now();
        let reply = app.message(text.as_str().unwrap())?;
        let mut idle_steps = 0;
        for _ in 0..4 {
            if app.prefetch_idle_step()?.is_some() {
                idle_steps += 1;
            } else {
                break;
            }
        }
        turns.push(json!({"step":step,"input":text,"output":reply.text,"new_nodes":reply.neue_neuronen,"elapsed_us":turn_start.elapsed().as_micros(),"idle_steps":idle_steps}));
        fs::write(
            dir.join("progress.json"),
            json!({"stage":"training","step":step,"elapsed_ms":start.elapsed().as_millis()})
                .to_string(),
        )?;
        if [100, 250, 500, 750, 1000].contains(&step) {
            evaluations.push(evaluate(&mut app, questions, step)?);
            fs::write(
                dir.join("results.json"),
                serde_json::to_vec_pretty(&json!({"evaluations":evaluations,"turns":turns}))?,
            )?;
            println!(
                "{step}/1000 Nachrichten; {} Knoten; {} s",
                app.status()?["neurons"],
                start.elapsed().as_secs()
            );
        }
    }
    assert_eq!(
        app.status()?["interactions"].as_u64().unwrap(),
        initial + 1000
    );
    let graph_nodes = app.verify_graph_full()?;
    let archive_episodes = app.verify_archive_full()?;
    let final_status = app.status()?;
    let report = json!({"training_messages":1000,"initial_interactions":initial,"elapsed_ms":start.elapsed().as_millis(),"graph_nodes_verified":graph_nodes,"archive_episodes_verified":archive_episodes,"final_status":final_status,"evaluations":evaluations,"turns":turns});
    fs::write(
        dir.join("results.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    drop(app);
    fs::copy(dir.join("working.db"), dir.join("trained-1000.db"))?;
    fs::write(
        dir.join("progress.json"),
        json!({"stage":"complete","step":1000,"elapsed_ms":start.elapsed().as_millis()})
            .to_string(),
    )?;
    println!(
        "Abgeschlossen. Gesichertes Ergebnis und Bericht: {}",
        dir.display()
    );
    Ok(())
}
