//! Release guard (Kap. 20): behavior texts live only as konfig data. The
//! scanner reads every shipped source file under src/ (the src/tests/
//! suite is the test filter exception) and flags behavior carriers in
//! string literals.
use super::quelldateien;
use crate::model::State;
use serde_json::Value;

/// Behavior texts from the konfig fixture (Kap. 20): every configured
/// fallback scaffold and status announcement, split around the `{wort}`
/// placeholder so a code literal with the placeholder filled in is caught
/// as well. The former anchor words are ordinary vocabulary since V01 and
/// are no behavior data.
struct Verhaltenstexte {
    texte: Vec<String>,
    fragmente: Vec<(String, String)>,
}

fn verhaltenstexte() -> Verhaltenstexte {
    let konfig = State::empty().konfig;
    let mut texte = Vec::new();
    let mut fragmente = Vec::new();
    for key in ["fallback_frage_geruest", "fallback_status_durchsage"] {
        for variante in konfig[key]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
        {
            texte.push(variante.to_lowercase());
            if let Some((vor, nach)) = variante.split_once("{wort}") {
                let vor = vor.trim().to_lowercase();
                let nach = nach.trim().to_lowercase();
                if !vor.is_empty() && !nach.is_empty() {
                    fragmente.push((vor, nach));
                }
            }
        }
    }
    Verhaltenstexte { texte, fragmente }
}

/// Whether position i opens a char literal (`'x'`, `'\n'`, `'…'`) rather
/// than a lifetime (`'a`, `'static`): an escape or a closing quote two chars
/// ahead. The codebase has no raw or byte string literals and no other
/// quote form this heuristic would misread.
fn ist_zeichenliteral(q: &[char], i: usize) -> bool {
    q.get(i) == Some(&'\'') && (q.get(i + 1) == Some(&'\\') || q.get(i + 2) == Some(&'\''))
}

/// Whether the source at position i starts with the marker text.
fn ab_hier(q: &[char], i: usize, marker: &str) -> bool {
    let marker: Vec<char> = marker.chars().collect();
    i + marker.len() <= q.len() && q[i..i + marker.len()] == marker[..]
}

/// Index just past the `}` matching the `{` at `start`, string/char/comment
/// aware.
fn schliessende_klammer(q: &[char], start: usize) -> usize {
    let mut tiefe = 0usize;
    let mut i = start;
    while i < q.len() {
        match q[i] {
            '"' => {
                i += 1;
                while i < q.len() {
                    if q[i] == '\\' {
                        i += 2;
                        continue;
                    }
                    let schliesst = q[i] == '"';
                    i += 1;
                    if schliesst {
                        break;
                    }
                }
            }
            '\'' if ist_zeichenliteral(q, i) => {
                i += 1;
                while i < q.len() {
                    if q[i] == '\\' {
                        i += 2;
                        continue;
                    }
                    let schliesst = q[i] == '\'';
                    i += 1;
                    if schliesst {
                        break;
                    }
                }
            }
            '/' if q.get(i + 1) == Some(&'/') => {
                while i < q.len() && q[i] != '\n' {
                    i += 1;
                }
            }
            '/' if q.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < q.len() && !(q[i] == '*' && q[i + 1] == '/') {
                    i += 1;
                }
                i += 1;
            }
            '{' => {
                tiefe += 1;
                i += 1;
            }
            '}' => {
                tiefe = tiefe.saturating_sub(1);
                i += 1;
                if tiefe == 0 {
                    return i;
                }
            }
            _ => i += 1,
        }
    }
    q.len()
}

/// Live production source: comments stripped and `#[cfg(test)]`
/// mod/impl/fn bodies removed, so the literal scan sees only shipped code.
fn produkt_code(inhalt: &str) -> String {
    let q: Vec<char> = inhalt.chars().collect();
    let mut raus = String::new();
    let mut i = 0usize;
    while i < q.len() {
        if q[i] == '"' {
            raus.push('"');
            i += 1;
            while i < q.len() {
                if q[i] == '\\' && i + 1 < q.len() {
                    raus.push(q[i]);
                    raus.push(q[i + 1]);
                    i += 2;
                    continue;
                }
                raus.push(q[i]);
                let schliesst = q[i] == '"';
                i += 1;
                if schliesst {
                    break;
                }
            }
            continue;
        }
        if ist_zeichenliteral(&q, i) {
            raus.push('\'');
            i += 1;
            while i < q.len() {
                if q[i] == '\\' && i + 1 < q.len() {
                    raus.push(q[i]);
                    raus.push(q[i + 1]);
                    i += 2;
                    continue;
                }
                raus.push(q[i]);
                let schliesst = q[i] == '\'';
                i += 1;
                if schliesst {
                    break;
                }
            }
            continue;
        }
        if q[i] == '/' && q.get(i + 1) == Some(&'/') {
            while i < q.len() && q[i] != '\n' {
                i += 1;
            }
            raus.push(' ');
            continue;
        }
        if q[i] == '/' && q.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < q.len() && !(q[i] == '*' && q[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(q.len());
            raus.push(' ');
            continue;
        }
        if q[i] == '#' && ab_hier(&q, i, "#[cfg(test)]") {
            i += "#[cfg(test)]".len();
            // Only a following mod/impl/fn body is test code; a field or a
            // bare block stays live.
            let mut j = i;
            while j < q.len() && q[j].is_whitespace() {
                j += 1;
            }
            let kopf_anfang = j;
            while j < q.len() && !matches!(q[j], '{' | ';' | ',') {
                j += 1;
            }
            let kopf: String = q[kopf_anfang..j.min(q.len())].iter().collect();
            if j < q.len()
                && q[j] == '{'
                && ["mod ", "impl ", "fn "].iter().any(|m| kopf.contains(m))
            {
                i = schliessende_klammer(&q, j);
                raus.push(' ');
                continue;
            }
            raus.push(' ');
            continue;
        }
        raus.push(q[i]);
        i += 1;
    }
    raus
}

/// The call name whose direct argument the literal at the opening quote is:
/// the identifier before the `(` right in front of it (`bail!`, `println!`,
/// `anyhow::anyhow!`, `.context`). Empty when the literal is embedded in a
/// larger expression.
fn aufruf_davor(q: &[char], anfuehrung: usize) -> String {
    let mut i = anfuehrung;
    while i > 0 && q[i - 1].is_whitespace() {
        i -= 1;
    }
    if i == 0 || q[i - 1] != '(' {
        return String::new();
    }
    i -= 1;
    if i > 0 && q[i - 1] == '!' {
        i -= 1;
    }
    let mut name: Vec<char> = Vec::new();
    while i > 0 && (q[i - 1].is_alphanumeric() || q[i - 1] == '_') {
        name.push(q[i - 1]);
        i -= 1;
    }
    name.iter().rev().collect()
}

/// All string literals of the cleaned source with the call name whose direct
/// argument each literal is (empty when embedded in a larger expression).
fn literale(code: &str) -> Vec<(String, String)> {
    let q: Vec<char> = code.chars().collect();
    let mut funde = Vec::new();
    let mut i = 0usize;
    while i < q.len() {
        if q[i] == '\'' && ist_zeichenliteral(&q, i) {
            i += 1;
            while i < q.len() {
                if q[i] == '\\' {
                    i += 2;
                    continue;
                }
                let schliesst = q[i] == '\'';
                i += 1;
                if schliesst {
                    break;
                }
            }
            continue;
        }
        if q[i] != '"' {
            i += 1;
            continue;
        }
        let anfang = i + 1;
        i += 1;
        while i < q.len() {
            if q[i] == '\\' {
                i += 2;
                continue;
            }
            if q[i] == '"' {
                break;
            }
            i += 1;
        }
        let text: String = q[anfang..i.min(q.len())].iter().collect();
        i += 1;
        funde.push((text, aufruf_davor(&q, anfang - 1)));
    }
    funde
}

/// Call contexts whose direct string arguments are the allowed Kap.-20
/// exception category: error, validation, and terminal system messages.
/// They are honest system output, never model answers — but the configured
/// behavior texts may not hide inside them either.
const MELDUNGS_KONTEXT: [&str; 11] = [
    "bail", "anyhow", "panic", "expect", "context", "println", "print", "eprintln", "eprint",
    "write", "writeln",
];

/// Kap.-20 check of one literal: a violation is (1) a complete sentence of
/// more than three words — exempt inside the message call contexts — or
/// (2) any configured behavior text (question scaffold, status
/// announcement), raw or with the `{wort}` placeholder filled in — never
/// exempt.
fn ist_verhaltenstraeger(text: &str, kontext: &str, texte: &Verhaltenstexte) -> Option<String> {
    let klein = text.to_lowercase();
    if let Some(t) = texte.texte.iter().find(|t| klein.contains(t.as_str())) {
        return Some(format!("konfig-text {t} im literal: {text}"));
    }
    if let Some((vor, nach)) = texte
        .fragmente
        .iter()
        .find(|(vor, nach)| klein.contains(vor.as_str()) && klein.contains(nach.as_str()))
    {
        return Some(format!(
            "konfig-geruest mit eingesetztem wort im literal: {text} ({vor} … {nach})"
        ));
    }
    if !MELDUNGS_KONTEXT.contains(&kontext)
        && text.split_whitespace().count() > 3
        && text.trim_end().ends_with(['.', '!', '?', '…'])
    {
        return Some(format!("verhaltenssatz im literal: {text}"));
    }
    None
}

/// All Kap.-20 violations in one Rust source file's content.
fn verletzungen_in(inhalt: &str, texte: &Verhaltenstexte) -> Vec<String> {
    let code = produkt_code(inhalt);
    literale(&code)
        .into_iter()
        .filter_map(|(text, kontext)| ist_verhaltenstraeger(&text, &kontext, texte))
        .collect()
}

#[test]
fn test_anti_hardcode() {
    // Kap. 20: formulations, triggers, and thresholds never live as code
    // strings — only as konfig data or learned state. The scan reads every
    // shipped source file under src/ recursively (the src/tests/ suite is
    // the test filter exception) and fails loudly on any behavior carrier.
    let texte = verhaltenstexte();
    assert!(texte.texte.len() >= 4);
    assert!(texte.fragmente.len() >= 4);
    let src_tests = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests");
    let mut verletzungen = Vec::new();
    for pfad in quelldateien() {
        if pfad.starts_with(&src_tests) {
            continue;
        }
        let inhalt = std::fs::read_to_string(&pfad).unwrap();
        for verletzung in verletzungen_in(&inhalt, &texte) {
            verletzungen.push(format!("{}: {verletzung}", pfad.display()));
        }
    }
    assert!(verletzungen.is_empty(), "\n{}", verletzungen.join("\n"));
}

#[test]
fn test_anti_hardcode_greift_beim_simulierten_verhaltensstring() {
    // (d) Test against test: the scanner fires on simulated violations and
    // spares the allowed exception categories — proving the guard itself
    // works, not just that today's code is clean.
    let texte = verhaltenstexte();
    // A configured question scaffold as a code literal — composed here from
    // the konfig data so this test file itself stays free of the text (the
    // Block-3 scan reads every src file, including this one)...
    let geruest = State::empty().konfig["fallback_frage_geruest"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let snippet = format!(r#"let text = "{geruest}";"#);
    assert!(!verletzungen_in(&snippet, &texte).is_empty());
    // ...a scaffold with the placeholder filled in...
    assert!(!verletzungen_in(
        r#"let text = "ich bin mir bei kaffee noch unsicher";"#,
        &texte
    )
    .is_empty());
    // ...and a complete behavior sentence...
    assert!(!verletzungen_in(
        r#"let text = "dies ist ein vollstaendiger verhaltenssatz.";"#,
        &texte
    )
    .is_empty());
    // Ordinary vocabulary is no behavior data since V01: the former
    // anchor words must not trip the guard.
    assert!(verletzungen_in(r#"let wort = "super";"#, &texte).is_empty());
    // Exceptions stay clean: an error message...
    assert!(verletzungen_in(
        r#"bail!("nachricht ueberschreitet das aktivierungsbudget von 128 woertern");"#,
        &texte
    )
    .is_empty());
    // ...a terminal system notice...
    assert!(verletzungen_in(
        r#"println!("systemhinweis: keine antwort konnte gebildet werden.");"#,
        &texte
    )
    .is_empty());
    // ...a command name...
    assert!(verletzungen_in(r#"let cmd = "/undo round";"#, &texte).is_empty());
    // ...and short mechanical identifiers.
    assert!(verletzungen_in(r#"let art = "integration_reject";"#, &texte).is_empty());
    // A full simulated source file: comments and #[cfg(test)] bodies are
    // the test filter, live code is not. The scaffold literal is composed
    // from the konfig data so this test file itself stays free of it.
    let datei = format!(
        "fn live() {{\n    // ein kommentar bleibt ein kommentar\n    let a = \"kurz\";\n    let b = \"{geruest}\";\n}}\n#[cfg(test)]\nmod tests {{\n    fn t() {{ let c = \"{geruest} aus dem test\"; }}\n}}\n"
    );
    let verletzungen = verletzungen_in(&datei, &texte);
    // Only the live scaffold literal remains: the comment was stripped and
    // the #[cfg(test)] body never ships.
    assert_eq!(verletzungen.len(), 1);
    assert!(verletzungen[0].contains("konfig-text"));
    assert!(verletzungen[0].contains("im literal"));
}
