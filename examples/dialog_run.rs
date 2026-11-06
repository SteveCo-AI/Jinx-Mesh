//! Dialog loop (cyclic tutor lines with echo chains of the net's own output)
//! running only on an explicitly supplied experimental vault copy.
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
    let tutor = match config["tutor"].as_array() {
        Some(lines) if !lines.is_empty() => lines,
        _ => bail!("training.json benötigt ein nicht leeres tutor-Array"),
    };
    let messages_target = match &config["messages_target"] {
        Value::Null => 10000,
        v => match v.as_u64() {
            Some(n) if (1..=100000).contains(&n) => n as usize,
            _ => bail!("messages_target muss eine ganze Zahl zwischen 1 und 100000 sein"),
        },
    };
    let checkpoint_every = match &config["checkpoint_every"] {
        Value::Null => 1000,
        v => match v.as_u64() {
            Some(n) if n >= 100 => n as usize,
            _ => bail!("checkpoint_every muss eine ganze Zahl ab 100 sein"),
        },
    };
    let echo_repeats = match &config["echo_repeats"] {
        Value::Null => 2,
        v => match v.as_u64() {
            Some(n) if (1..=10).contains(&n) => n as usize,
            _ => bail!("echo_repeats muss eine ganze Zahl zwischen 1 und 10 sein"),
        },
    };
    let questions = config["questions"].as_array().unwrap();
    fs::write(
        dir.join("progress.json"),
        json!({"stage":"awaiting_login","step":0}).to_string(),
    )?;
    println!(
        "Jinx-Mesh: {messages_target} Dialog-Nachrichten aus {} Tutor-Zeilen mit je bis zu {echo_repeats} Echo-Wiederholungen auf separater Kopie.\nDein aktiver Vault bleibt unverändert. Ergebnisse bleiben lokal.\n",
        tutor.len()
    );
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
    let mut evaluations = vec![evaluate(&mut app, questions, 0)?];
    let mut turns = Vec::new();
    let initial = app.status()?["interactions"].as_u64().unwrap();
    let record_step = |app: &mut App,
                       evaluations: &mut Vec<Value>,
                       turns: &[Value],
                       learned_total: usize|
     -> Result<()> {
        fs::write(
            dir.join("progress.json"),
            json!({"stage":"training","step":learned_total}).to_string(),
        )?;
        if learned_total.is_multiple_of(500) {
            println!(
                "{learned_total}/{messages_target} Nachrichten; {} Knoten; {} s",
                app.status()?["neurons"],
                start.elapsed().as_secs()
            );
        }
        if learned_total.is_multiple_of(checkpoint_every) || learned_total == messages_target {
            evaluations.push(evaluate(app, questions, learned_total)?);
            assert_eq!(
                app.status()?["interactions"].as_u64().unwrap() - initial,
                learned_total as u64
            );
            fs::write(
                dir.join("results.json"),
                serde_json::to_vec_pretty(&json!({"evaluations":evaluations,"turns":turns}))?,
            )?;
        }
        Ok(())
    };
    let mut learned_total = 0;
    let mut tutor_index = 0;
    while learned_total < messages_target {
        let tutor_line = tutor[tutor_index % tutor.len()].as_str().unwrap();
        tutor_index += 1;
        let mut reply = app.message(tutor_line)?;
        learned_total += 1;
        turns.push(
            json!({"kind":"tutor","step":learned_total,"input":tutor_line,"output":reply.text}),
        );
        record_step(&mut app, &mut evaluations, &turns, learned_total)?;
        if !reply.text.is_empty() {
            for _ in 0..echo_repeats {
                if learned_total >= messages_target {
                    break;
                }
                let input = reply.text.clone();
                reply = app.message(&input)?;
                learned_total += 1;
                turns.push(
                    json!({"kind":"echo","step":learned_total,"input":input,"output":reply.text}),
                );
                record_step(&mut app, &mut evaluations, &turns, learned_total)?;
            }
        }
    }
    assert_eq!(learned_total, messages_target);
    assert_eq!(
        app.status()?["interactions"].as_u64().unwrap(),
        initial + messages_target as u64
    );
    let graph_nodes = app.verify_graph_full()?;
    let archive_episodes = app.verify_archive_full()?;
    let final_status = app.status()?;
    let report = json!({"messages_target":messages_target,"echo_repeats":echo_repeats,"tutor_count":tutor.len(),"initial_interactions":initial,"elapsed_ms":start.elapsed().as_millis(),"graph_nodes_verified":graph_nodes,"archive_episodes_verified":archive_episodes,"final_status":final_status,"evaluations":evaluations,"turns":turns});
    fs::write(
        dir.join("results.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    drop(app);
    let name = format!("trained-{}.db", messages_target);
    fs::copy(dir.join("working.db"), dir.join(name))?;
    fs::write(
        dir.join("progress.json"),
        json!({"stage":"complete","step":messages_target}).to_string(),
    )?;
    println!(
        "Abgeschlossen. Gesichertes Ergebnis und Bericht: {}",
        dir.display()
    );
    Ok(())
}
