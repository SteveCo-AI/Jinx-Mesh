use anyhow::{bail, Result};
use saeugling::{App, Import};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc, Mutex,
    },
    time::Duration,
};
use zeroize::Zeroizing;

fn input(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        bail!("Eingabe beendet");
    }
    Ok(line.trim_end().into())
}
fn input_with_progress(prompt: &str, progress: &Receiver<String>) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let (line_tx, line_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = io::stdin().read_line(&mut line).map(|count| (count, line));
        let _ = line_tx.send(result);
    });
    loop {
        match line_rx.recv_timeout(Duration::from_millis(150)) {
            Ok(Ok((0, _))) => bail!("Eingabe beendet"),
            Ok(Ok((_, line))) => return Ok(line.trim_end().into()),
            Ok(Err(error)) => return Err(error.into()),
            Err(mpsc::RecvTimeoutError::Disconnected) => bail!("Eingabe beendet"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                while let Ok(message) = progress.try_recv() {
                    print_async_status(&message);
                }
            }
        }
    }
}
fn print_async_status(message: &str) {
    use std::io::IsTerminal;
    let stderr = io::stderr();
    let mut out = stderr.lock();
    if stderr.is_terminal() {
        let _ = write!(out, "\x1b7\r\n{message}\r\n\x1b8");
    } else {
        let _ = writeln!(out, "{message}");
    }
    let _ = out.flush();
}
fn password(prompt: &str) -> Result<Zeroizing<String>> {
    Ok(Zeroizing::new(rpassword::prompt_password(prompt)?))
}
struct CredentialWait(Arc<AtomicBool>);
impl CredentialWait {
    fn begin(flag: Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::Relaxed);
        Self(flag)
    }
}
impl Drop for CredentialWait {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}
fn show_reply(reply: saeugling::Reply) {
    println!("{}", reply.text);
    if let Some(progress) = reply.activation {
        let phase = match progress.phase.as_str() {
            "complete" => "vollständig",
            "budget_limited" if progress.can_continue => {
                "Budget erreicht; Fortsetzung mit /continue möglich"
            }
            "budget_limited" => "Budget erreicht; keine Fortsetzung verfügbar",
            _ => "vorläufig",
        };
        println!(
            "[Aktivierung: {phase}; {} Knoten von {}, {} Seiten, {} Archivepisoden geladen, {} Quellen-Treffer]",
            progress.nodes_loaded,
            progress.node_budget,
            progress.pages_loaded,
            progress.sources_loaded,
            progress.source_matches.len()
        );
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("Fehler: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" {
        println!("Jinx 2.5.0\n  saeugling init VAULT [STATE_JSON ACTIONS_JSON [LEGACY_AUDIT]]\n  saeugling chat VAULT\nAnmeldedaten werden ausschließlich verdeckt im Terminal eingegeben.");
        return Ok(());
    }
    if args[0] == "init" {
        if args.len() != 2 && !(4..=5).contains(&args.len()) {
            bail!("init VAULT [STATE_JSON ACTIONS_JSON [LEGACY_AUDIT]]");
        }
        let user = input("Developer-Nutzername: ")?;
        let pass = password("Neues Passwort (mind. 12 Zeichen): ")?;
        if *pass != *password("Passwort wiederholen: ")? {
            bail!("Passwörter stimmen nicht überein");
        }
        let (secret, code) = if input("Optionale Authenticator-2FA einrichten? [j/N]: ")?
            .to_lowercase()
            == "j"
        {
            let secret = App::new_totp_secret();
            println!("Authenticator manuell: TOTP, SHA1, 6 Stellen, 30 Sekunden.\nJinx-Einrichtungsschlüssel (nur jetzt sichtbar): {secret}");
            let code = password("Authenticator-Code bestätigen: ")?;
            (Some(secret), Some(code))
        } else {
            (None, None)
        };
        if args.len() == 2 {
            App::create_empty(
                Path::new(&args[1]),
                &user,
                &pass,
                secret,
                code.as_deref().map(|s| s.as_str()),
            )?;
        } else {
            App::create(
                Path::new(&args[1]),
                &user,
                &pass,
                secret,
                code.as_deref().map(|s| s.as_str()),
                Import {
                    state: PathBuf::from(&args[2]),
                    actions: PathBuf::from(&args[3]),
                    legacy_audit: args.get(4).map(PathBuf::from),
                },
            )?;
        }
        println!("Verschlüsselter Vault angelegt. Originaldateien bleiben unverändert. Start: saeugling chat {}",args[1]);
        return Ok(());
    }
    if args[0] != "chat" || args.len() != 2 {
        bail!("chat VAULT");
    }
    let app = Arc::new(Mutex::new(App::open(Path::new(&args[1]))?));
    let weak = Arc::downgrade(&app);
    let credential_wait = Arc::new(AtomicBool::new(false));
    let idle_gate = Arc::clone(&credential_wait);
    let (progress_tx, progress_rx) = mpsc::channel::<String>();
    // Expire sessions and prefetch one bounded graph step while the user types.
    // Credential prompts suppress prefetch; progress uses cursor save/restore.
    std::thread::spawn(move || {
        let mut last_status: Option<(usize, usize, usize, String)> = None;
        let mut last_error: Option<String> = None;
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let Some(app) = weak.upgrade() else {
                break;
            };
            if let Ok(mut app) = app.lock() {
                app.tick();
                if !idle_gate.load(Ordering::Relaxed) {
                    match app.prefetch_idle_step() {
                        Ok(Some(status)) => {
                            let snapshot = (
                                status.nodes_loaded,
                                status.frontier_remaining,
                                status.sources_loaded,
                                status.phase.clone(),
                            );
                            if last_status.as_ref() != Some(&snapshot) {
                                let message = format!(
                                "[Jinx arbeitet weiter: {} Knoten, {} Seiten, {} Archivepisoden geladen, {} offene Graphverweise]",
                                status.nodes_loaded,
                                status.pages_loaded,
                                status.sources_loaded,
                                status.frontier_remaining
                            );
                                let _ = progress_tx.send(message);
                                last_status = Some(snapshot);
                            }
                            last_error = None;
                        }
                        Ok(None) => {
                            last_status = None;
                            last_error = None;
                        }
                        Err(error) => {
                            let error = error.to_string();
                            if last_error.as_deref() != Some(&error) {
                                let _ = progress_tx
                                    .send(format!("[Vorladung fehlgeschlagen: {error}]"));
                                last_error = Some(error);
                            }
                            last_status = None;
                        }
                    }
                }
            };
        }
    });
    println!("Gesperrt. /login | /freeze | /logout | /status | /recall [--strategy NAME] TEXT | /verify-archive | /verify-graph | /continue | /slaap | /train TEXTDATEI | /code CODEDATEI | /exit\nNach /login lernt jede normale Nachricht automatisch. /recall sucht in gespeicherten Gesprächsstellen; NAME ist lexical, contextual oder temporal. /verify-archive und /verify-graph prüfen alle gespeicherten Blöcke. /continue setzt budgetbegrenzte Graphaktivierung fort. /freeze stoppt Lernen; zum Fortsetzen erneut /login.");
    loop {
        let line = match input_with_progress("> ", &progress_rx) {
            Ok(l) => l,
            Err(_) => break,
        };
        let command = line.trim();
        if command == "/exit" {
            break;
        }
        let result: Result<()> = (|| {
            if command == "/login" {
                let _credential_wait = CredentialWait::begin(Arc::clone(&credential_wait));
                while progress_rx.try_recv().is_ok() {}
                let user = input("Nutzername: ")?;
                let pass = password("Passwort: ")?;
                let code = password("2FA-Code (leer, falls nicht eingerichtet): ")?;
                app.lock().unwrap().login(
                    &user,
                    &pass,
                    if code.is_empty() { None } else { Some(&code) },
                )?;
                println!("Developer angemeldet. Kontinuierliches Lernen aktiv.");
            } else if command == "/freeze" {
                app.lock().unwrap().freeze()?;
                println!("Eingefroren: nur Lesen, keine Aktionen oder Lernänderungen.");
            } else if command == "/logout" {
                app.lock().unwrap().logout();
                println!("Gesperrt.");
            } else if command == "/status" {
                println!("{}", app.lock().unwrap().status()?);
            } else if command == "/continue" {
                let reply = app
                    .lock()
                    .unwrap()
                    .continue_message_with_progress(|progress| {
                        println!(
                            "[Graphaktivierung: {} Knoten, {} Seiten; {} Quellen geladen]",
                            progress.nodes_loaded, progress.pages_loaded, progress.sources_loaded
                        );
                    })?;
                show_reply(reply);
            } else if command == "/verify-archive" {
                println!("Prüfe alle verschlüsselten Archivblöcke …");
                let count = app.lock().unwrap().verify_archive_full()?;
                println!("Archivprüfung erfolgreich: {count} Episoden.");
            } else if command == "/verify-graph" {
                println!("Prüfe alle verschlüsselten Graphdatensätze …");
                let count = app.lock().unwrap().verify_graph_full()?;
                println!("Graphprüfung erfolgreich: {count} Neuronen.");
            } else if let Some(query) = command.strip_prefix("/recall ") {
                println!("Prüfe verschlüsselte Gesprächspassagen …");
                let mut parts = query.split_whitespace();
                let mut strategy: Option<String> = None;
                let mut before = 0u64;
                let mut query_words = Vec::new();
                while let Some(part) = parts.next() {
                    match part {
                        "--strategy" => {
                            strategy = Some(
                                parts
                                    .next()
                                    .ok_or_else(|| {
                                        anyhow::anyhow!("Nach --strategy fehlt der Handlername")
                                    })?
                                    .to_owned(),
                            );
                        }
                        "--before" => {
                            before = parts
                                .next()
                                .ok_or_else(|| anyhow::anyhow!("Nach --before fehlt der Offset"))?
                                .parse()
                                .map_err(|_| anyhow::anyhow!("Ungültiger Recall-Offset"))?;
                        }
                        _ => {
                            query_words.push(part);
                            query_words.extend(parts);
                            break;
                        }
                    }
                }
                let query = query_words.join(" ");
                let result = app.lock().unwrap().recall_with_strategy_before_progress(
                    &query,
                    5,
                    strategy.as_deref(),
                    before,
                    |scanned, total| {
                        println!("[Archivsuche: {scanned} von {total} Seiten geprüft]");
                    },
                )?;
                if !result.complete {
                    println!(
                        "Teiltreffer: Seitenbudget erreicht; geprüfte Episoden {} von {}. Fortsetzen mit `/recall --before {} {}{}`.",
                        result.episodes_scanned,
                        result.total_episodes,
                        result.next_offset.unwrap_or(before),
                        strategy.as_ref().map(|s| format!("--strategy {s} ")).unwrap_or_default(),
                        query
                    );
                }
                if result.hits.is_empty() {
                    println!("Keine passende archivierte Passage gefunden.");
                }
                for hit in result.hits {
                    println!("{}", hit);
                }
            } else if command == "/slaap" {
                let report = app.lock().unwrap().consolidate()?;
                println!("Konsolidierung abgeschlossen: {report}");
            } else if let Some(path) = command.strip_prefix("/train ") {
                println!(
                    "{}",
                    app.lock().unwrap().train_file(Path::new(path), false)?
                );
            } else if let Some(path) = command.strip_prefix("/code ") {
                println!("{}", app.lock().unwrap().train_file(Path::new(path), true)?);
            } else if command.starts_with('/') {
                bail!(
                    "Unbekannter Befehl. /login ohne Passwort in der Nachrichtenzeile verwenden."
                );
            } else {
                let reply = app
                    .lock()
                    .unwrap()
                    .message_with_progress(&line, |progress| {
                        println!(
                            "[Graphaktivierung: {} Knoten, {} Seiten; {} Verweise, {} Archivepisoden geladen]",
                            progress.nodes_loaded, progress.pages_loaded, progress.source_refs_seen, progress.sources_loaded
                        );
                    })?;
                show_reply(reply);
            }
            Ok(())
        })();
        if let Err(e) = result {
            println!("{e}");
        }
    }
    app.lock().unwrap().logout();
    Ok(())
}
