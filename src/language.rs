//! Structural observations and context-scoped user feedback; no sentence-type labels.
//! Kap. 18's implicit channel is structure recognition only — comparing two
//! token sequences: repeating the previous question word for word is one
//! negative rating step. There is no ritual word and no continuation
//! confirmation; without a +/- prefix an ordinary message never rates
//! anything.
use crate::{graph, model::State};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Mark {
    pub after: usize,
    pub symbol: String,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct Parsed {
    pub words: Vec<String>,
    pub marks: Vec<Mark>,
}
pub(crate) fn parse(text: &str) -> Parsed {
    let mut words = Vec::new();
    let mut marks = Vec::new();
    let mut word = String::new();
    for c in text.nfc().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() || (c == '\'' && !word.is_empty()) {
            word.push(c);
        } else {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            if matches!(c, '.' | '!' | '?' | ':' | ';' | ',' | '…') && !words.is_empty() {
                marks.push(Mark {
                    after: words.len(),
                    symbol: c.to_string(),
                });
            }
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    if !words.is_empty()
        && !marks
            .iter()
            .any(|m| m.after == words.len() && m.symbol != ",")
    {
        marks.push(Mark {
            after: words.len(),
            symbol: ".".into(),
        });
    }
    Parsed { words, marks }
}
pub(crate) fn split_feedback(text: &str) -> (Option<i8>, &str) {
    let text = text.trim_start();
    if let Some(rest) = text.strip_prefix('+') {
        (Some(1), rest.trim_start())
    } else if let Some(rest) = text.strip_prefix('-') {
        (Some(-1), rest.trim_start())
    } else {
        (None, text)
    }
}
fn digest(value: &impl Serialize) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("serializable language data"))
    )
}
pub(crate) fn context(text: &str) -> String {
    digest(&parse(text))
}
fn pair(prev: Option<&str>, word: &str) -> String {
    serde_json::to_string(&(prev, word)).unwrap()
}
fn trim(map: &mut serde_json::Map<String, Value>, max: usize) {
    while map.len() > max {
        if let Some(key) = map.keys().next().cloned() {
            map.remove(&key);
        }
    }
}
pub(crate) fn observe(state: &mut State, p: &Parsed) {
    let map = state
        .extra
        .entry("structure_observations".into())
        .or_insert_with(|| json!({}))
        .as_object_mut();
    let Some(map) = map else {
        return;
    };
    for (i, word) in p.words.iter().enumerate() {
        let key = pair(i.checked_sub(1).map(|j| p.words[j].as_str()), word);
        let row = map
            .entry(key)
            .or_insert_with(|| json!({"marks":{},"end":0,"continue":0}));
        if !row.is_object() || !row["marks"].is_object() {
            *row = json!({"marks":{},"end":0,"continue":0});
        }
        let symbols: String = p
            .marks
            .iter()
            .filter(|m| m.after == i + 1)
            .map(|m| m.symbol.as_str())
            .collect();
        let count = row["marks"][&symbols].as_u64().unwrap_or(0);
        row["marks"][&symbols] = json!(count.saturating_add(1));
        let field = if i + 1 == p.words.len() {
            "end"
        } else {
            "continue"
        };
        row[field] = json!(row[field].as_u64().unwrap_or(0).saturating_add(1));
    }
    trim(map, 4096);
}
pub(crate) fn row<'a>(state: &'a State, prev: Option<&str>, word: &str) -> Option<&'a Value> {
    state
        .extra
        .get("structure_observations")?
        .get(pair(prev, word))
}
pub(crate) fn end(state: &State, prev: Option<&str>, word: &str) -> bool {
    row(state, prev, word).is_some_and(|r| {
        r["end"].as_u64().unwrap_or(0) > 0
            && r["end"].as_u64().unwrap_or(0) >= r["continue"].as_u64().unwrap_or(0)
    })
}
pub(crate) fn render(words: &[String], marks: &[Mark]) -> String {
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let suffix: String = marks
                .iter()
                .filter(|m| m.after == i + 1)
                .map(|m| m.symbol.as_str())
                .collect();
            format!("{w}{suffix}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}
pub(crate) fn render_path(state: &State, ids: &[String], context: &str) -> String {
    let mut previous: Option<String> = None;
    ids.iter()
        .map(|id| {
            let raw = &state.neuronen[id].label;
            let word = graph::tokens(raw)
                .into_iter()
                .next()
                .unwrap_or_else(|| raw.clone());
            let suffix = row(state, previous.as_deref(), &word)
                .and_then(|r| r["marks"].as_object())
                .and_then(|m| {
                    m.iter().max_by(|(a, x), (b, y)| {
                        marker_bias(state, context, previous.as_deref(), &word, a)
                            .total_cmp(&marker_bias(state, context, previous.as_deref(), &word, b))
                            .then_with(|| x.as_u64().unwrap_or(0).cmp(&y.as_u64().unwrap_or(0)))
                    })
                })
                .map(|(k, _)| k.as_str())
                .unwrap_or_else(|| raw.strip_prefix(&word).unwrap_or(""));
            let result = format!("{word}{suffix}");
            previous = Some(word);
            result
        })
        .collect::<Vec<_>>()
        .join(" ")
}
pub(crate) fn target(input: &str, output: &str) -> Value {
    let parsed = parse(output);
    // The Kap.-18 implicit channel contrasts the next input with the input
    // this response answered, so the target stores the previous input's
    // tokens (bounded like the response words). Pre-Block-2 targets lack
    // the field and honestly produce no implicit signal.
    let mut eingabe = parse(input).words;
    eingabe.truncate(128);
    json!({"context":context(input),"words":parsed.words,"marks":parsed.marks,"eingabe":eingabe})
}
pub(crate) fn remember(state: &mut State, input: &str, output: &str) {
    state
        .extra
        .insert("feedback_target".into(), target(input, output));
}
fn marker_bias(state: &State, query: &str, prev: Option<&str>, word: &str, symbol: &str) -> f64 {
    state
        .extra
        .get("context_feedback")
        .and_then(|m| m.get(query))
        .and_then(|m| m.get(format!("{}#mark:{symbol}", pair(prev, word))))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}
/// One explicit +/- rating step. Shared mechanics of all rating channels.
const BEWERTUNG_SCHRITT: f64 = 0.25;

/// Rate the stored target's learned edges and marks by `delta` inside its
/// context. Shared mechanics of the explicit +/- and implicit repetition
/// channels; it never consumes the stored target.
fn ziel_bewerten(state: &mut State, ziel: &Value, delta: f64) -> anyhow::Result<()> {
    let words = ziel["words"]
        .as_array()
        .filter(|w| !w.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Die vorherige Ausgabe enthält keine bewertbare Antwort"))?;
    let key = ziel["context"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Ungültige Bewertungsdaten"))?;
    let map = state
        .extra
        .entry("context_feedback".into())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Ungültige Bewertungsdaten"))?;
    let entry = map
        .entry(key.to_owned())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Ungültiger Bewertungskontext"))?;
    for i in 0..words.len() {
        let edge = pair(
            i.checked_sub(1).and_then(|j| words[j].as_str()),
            words[i].as_str().unwrap(),
        );
        let score = entry.get(&edge).and_then(Value::as_f64).unwrap_or(0.0);
        entry.insert(edge.clone(), json!((score + delta).clamp(-2.0, 2.0)));
        let symbols = ziel["marks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| m["after"].as_u64() == Some((i + 1) as u64))
            .filter_map(|m| m["symbol"].as_str())
            .collect::<String>();
        let mark = format!("{edge}#mark:{symbols}");
        let score = entry.get(&mark).and_then(Value::as_f64).unwrap_or(0.0);
        entry.insert(mark, json!((score + delta).clamp(-2.0, 2.0)));
    }
    trim(entry, 256);
    trim(map, 256);
    Ok(())
}

pub(crate) fn apply_feedback(state: &mut State, value: i8) -> anyhow::Result<()> {
    let target = state
        .extra
        .remove("feedback_target")
        .ok_or_else(|| anyhow::anyhow!("Keine vorherige Modellantwort zum Bewerten"))?;
    ziel_bewerten(state, &target, f64::from(value) * BEWERTUNG_SCHRITT)?;
    let counts = state
        .extra
        .entry("feedback_counts".into())
        .or_insert_with(|| json!({"positive":0,"negative":0}));
    let field = if value > 0 { "positive" } else { "negative" };
    counts[field] = json!(counts[field].as_u64().unwrap_or(0).saturating_add(1));
    Ok(())
}
pub(crate) fn bias(state: &State, query: &str, prev: Option<&str>, word: &str) -> f64 {
    state
        .extra
        .get("context_feedback")
        .and_then(|m| m.get(query))
        .and_then(|m| m.get(pair(prev, word)))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

// ---- Kap. 18: Impliziter Wiederholungs-Kanal ----
// There is no ritual word: rating happens only through the explicit +/-
// prefix (a DEV tool; a sign at the message start hardly ever occurs by
// chance) or through structure recognition — comparing two token sequences,
// never a word list. The former konfig-data anchor words are gone together
// with their weight and maturation: anchor words can occur by chance in any
// sentence and falsify the weights (owner decision V01).

/// One training move's Kap.-18 implicit signal for the previous response:
/// whether the input repeats the previous question word for word — the
/// answer did not carry.
pub(crate) struct Signale {
    pub wiederholung: bool,
}

/// Read one move's Kap.-18 signal before learning mutates anything.
/// `ziel` is the stored feedback target; the caller clones it because the
/// explicit +/- rating consumes the stored copy.
pub(crate) fn signale_lesen(
    tokens: &[String],
    explizit: Option<i8>,
    ziel: Option<&Value>,
) -> Signale {
    // Kanal 1 (Ausführungskompetenz): repeating the previous question word
    // for word means the answer did not carry — one negative rating step.
    // Moving on to something new is NOT a signal (the implicit continuation
    // confirmation is gone); an explicit +/- rating is the definitive signal
    // of its move; without the stored previous input there is no honest
    // comparison.
    let wiederholung = explizit.is_none()
        && !tokens.is_empty()
        && ziel
            .and_then(|ziel| ziel.get("eingabe"))
            .and_then(Value::as_array)
            .is_some_and(|eingabe| {
                eingabe.len() == tokens.len()
                    && eingabe
                        .iter()
                        .zip(tokens)
                        .all(|(alt, token)| alt.as_str() == Some(token.as_str()))
            });
    Signale { wiederholung }
}

/// Apply one move's Kap.-18 signal: a repeated question rates the previous
/// response one negative rating step. `ziel` is the move's feedback target,
/// passed separately because an explicit +/- rating already consumed the
/// stored copy. Without a stored previous response nothing is rated — the
/// repeated word still learns as an ordinary token through grow/hear.
pub(crate) fn signale_anwenden(
    state: &mut State,
    signale: &Signale,
    ziel: Option<&Value>,
) -> anyhow::Result<()> {
    // Kanal 2 (Ergebnis-Wert) is filled by the Kap.-19 fallback flow below;
    // the slot stays an object even for moves that never see a fallback.
    state
        .extra
        .entry("kanal2_letzte".into())
        .or_insert_with(|| json!({}));
    if signale.wiederholung {
        if let Some(ziel) = ziel {
            ziel_bewerten(state, ziel, -BEWERTUNG_SCHRITT)?;
        }
    }
    Ok(())
}

// ---- Kap. 19: Unsicherheits-Fallback ----
// The unclear signal is purely mechanical: a training move without an
// explicit +/- rating while its conversation carries contradictory
// context-feedback values in the move's context — the strongest positive
// AND the strongest negative both reach `fallback_unklar_min` (the rating
// channels clamp scores into [-2, 2], which is the band's upper end).
// Instead of leaving another bare guess-answer, the move appends ONE
// utterance whose building blocks come from konfig data — rotating scaffold
// variants plus one learned conversation word — and a theme asked too often
// gets the status announcement instead of a further question. The flow
// ships disarmed: the konfig switch `fallback_aktivier` (the
// mutation_aktivier pattern) gates the whole path; without the vault's GO
// in the data no prefabricated text is ever appended. The utterance never
// becomes the rateable feedback target: no rating or repetition comparison
// may consume it (Kap. 18/19); the user's answer to it is a normal learning
// move, recorded into the channel-2 slot.

/// Context-feedback values one training move carries between its input tokens
/// and its answer words in the move's context: every input→answer pair plus
/// the answer path's own consecutive pairs (the first word with no
/// predecessor). Each value comes with the word the rating channels score.
fn signal_werte(
    state: &State,
    kontext: &str,
    eingabe: &[String],
    antwort: &[String],
) -> Vec<(f64, String)> {
    let mut werte = Vec::new();
    for (i, wort) in antwort.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| antwort[j].as_str());
        werte.push((bias(state, kontext, prev, wort), wort.clone()));
    }
    for token in eingabe {
        for wort in antwort {
            werte.push((
                bias(state, kontext, Some(token.as_str()), wort),
                wort.clone(),
            ));
        }
    }
    werte
}

/// Kap.-19 fallback step after a training move produced `antwort` for
/// `eingabe` in context `kontext` (the theme digest). The konfig switch
/// `fallback_aktivier` gates the whole flow — disarmed (the fixture
/// default) no utterance is ever composed or appended. When armed and the
/// move's signal is unclear, compose ONE utterance from the configured
/// scaffold variants plus the learned conversation word carrying the
/// strongest contradiction; a theme at the `fallback_drossel_max` question
/// cap gets the configured status announcement instead. Returns the
/// utterance and whether it is the status announcement; without
/// contradictory values or without configured scaffolds there is honestly
/// nothing to ask (None).
pub(crate) fn fallback_schritt(
    state: &mut State,
    kontext: &str,
    eingabe: &[String],
    antwort: &[String],
) -> Option<(String, bool)> {
    if !state
        .konfig
        .get("fallback_aktivier")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    if eingabe.is_empty() || antwort.is_empty() {
        return None;
    }
    let werte = signal_werte(state, kontext, eingabe, antwort);
    let schwelle = state.number("fallback_unklar_min");
    let max = werte
        .iter()
        .map(|(v, _)| *v)
        .max_by(|a, b| a.total_cmp(b))?;
    let min = werte
        .iter()
        .map(|(v, _)| *v)
        .min_by(|a, b| a.total_cmp(b))?;
    if max < schwelle || min > -schwelle {
        return None;
    }
    // The learned conversation word carrying the strongest contradiction;
    // ties resolve deterministically to the later value in collection order.
    let (_, wort) = werte
        .iter()
        .max_by(|(a, _), (b, _)| a.abs().total_cmp(&b.abs()))?;
    let wort = wort.clone();
    let thema_zaehler = state
        .extra
        .get("fallback_pro_thema")
        .and_then(|m| m.get(kontext))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let durchsage = thema_zaehler >= state.number("fallback_drossel_max") as u64;
    let gerueste: Vec<&str> = state
        .konfig
        .get(if durchsage {
            "fallback_status_durchsage"
        } else {
            "fallback_frage_geruest"
        })
        .and_then(Value::as_array)
        .map(|varianten| {
            varianten
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if gerueste.is_empty() {
        // Without configured data no honest utterance can be composed.
        return None;
    }
    // Rotation by usage counting: every fallback utterance steps the counter,
    // so consecutive questions cycle through the variants instead of
    // repeating one static text.
    let zaehler = state
        .extra
        .get("fallback_geruest_zaehler")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let geruest = gerueste[(zaehler % gerueste.len() as u64) as usize];
    state.extra.insert(
        "fallback_geruest_zaehler".into(),
        json!(zaehler.saturating_add(1)),
    );
    if !durchsage {
        // The question counts toward the theme's throttle...
        let themen = state
            .extra
            .entry("fallback_pro_thema".into())
            .or_insert_with(|| json!({}));
        if let Some(map) = themen.as_object_mut() {
            map.insert(kontext.into(), json!(thema_zaehler.saturating_add(1)));
            trim(map, 256);
        }
        // ...and stays open in the channel-2 slot until the next move
        // answers it.
        let kanal2 = state
            .extra
            .entry("kanal2_letzte".into())
            .or_insert_with(|| json!({}));
        if let Some(felder) = kanal2.as_object_mut() {
            felder.insert(
                "offen".into(),
                json!({"thema": kontext, "runde": state.interaktionen}),
            );
        }
    }
    Some((geruest.replace("{wort}", &wort), durchsage))
}

/// Kap.-18 channel 2 (Ergebnis-Wert), filled by the Kap.-19 fallback flow:
/// when the previous move left an open question, the user's next input is the
/// observed result value — record it and close the open slot. Pure usage
/// data; nothing here rates anything.
pub(crate) fn kanal2_notieren(state: &mut State, eingabe: &[String]) {
    let Some(thema) = state
        .extra
        .get("kanal2_letzte")
        .and_then(|k| k.get("offen"))
        .and_then(|o| o.get("thema"))
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return;
    };
    let mut worte = eingabe.to_vec();
    worte.truncate(128);
    let kanal2 = state
        .extra
        .entry("kanal2_letzte".into())
        .or_insert_with(|| json!({}));
    if let Some(felder) = kanal2.as_object_mut() {
        felder.remove("offen");
        felder.insert(
            "ergebnis".into(),
            json!({"thema": thema, "worte": worte, "runde": state.interaktionen}),
        );
    }
}
