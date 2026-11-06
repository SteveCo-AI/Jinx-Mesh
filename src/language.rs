//! Structural observations and context-scoped user feedback; no sentence-type labels.
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
fn row<'a>(state: &'a State, prev: Option<&str>, word: &str) -> Option<&'a Value> {
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
    json!({"context":context(input),"words":parsed.words,"marks":parsed.marks})
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
pub(crate) fn apply_feedback(state: &mut State, value: i8) -> anyhow::Result<()> {
    let target = state
        .extra
        .remove("feedback_target")
        .ok_or_else(|| anyhow::anyhow!("Keine vorherige Modellantwort zum Bewerten"))?;
    let words = target["words"]
        .as_array()
        .filter(|w| !w.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Die vorherige Ausgabe enthält keine bewertbare Antwort"))?;
    let key = target["context"].as_str().unwrap();
    let map = state
        .extra
        .entry("context_feedback".into())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Ungültige Bewertungsdaten"))?;
    let entry = map
        .entry(key)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Ungültiger Bewertungskontext"))?;
    for i in 0..words.len() {
        let edge = pair(
            i.checked_sub(1).and_then(|j| words[j].as_str()),
            words[i].as_str().unwrap(),
        );
        let score = entry.get(&edge).and_then(Value::as_f64).unwrap_or(0.0);
        entry.insert(
            edge.clone(),
            json!((score + f64::from(value) * 0.25).clamp(-2.0, 2.0)),
        );
        let symbols = target["marks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| m["after"].as_u64() == Some((i + 1) as u64))
            .filter_map(|m| m["symbol"].as_str())
            .collect::<String>();
        let mark = format!("{edge}#mark:{symbols}");
        let score = entry.get(&mark).and_then(Value::as_f64).unwrap_or(0.0);
        entry.insert(
            mark,
            json!((score + f64::from(value) * 0.25).clamp(-2.0, 2.0)),
        );
    }
    trim(entry, 256);
    trim(map, 256);
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
