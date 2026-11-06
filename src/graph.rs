use crate::model::{Node, State};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn tokens(text: &str) -> Vec<String> {
    crate::language::parse(text).words
}
pub(crate) fn word(state: &State, t: &str) -> Option<String> {
    let id = format!("lex:{t}");
    if state.neuronen.get(&id).is_some_and(|n| n.typ == "LEX") {
        return Some(id);
    }
    // Legacy punctuation-bearing nodes remain intact and can be reused lazily.
    state
        .neuronen
        .iter()
        .find(|(_, n)| n.typ == "LEX" && tokens(&n.label).as_slice() == [t])
        .map(|(id, _)| id.clone())
}

pub(crate) fn edge(state: &State, a: &str, b: &str) -> Option<f64> {
    state.kanten.get(a)?.get(b).copied()
}
pub(crate) fn set_edge(state: &mut State, a: &str, b: &str, w: f64) {
    state
        .kanten
        .entry(a.into())
        .or_default()
        .insert(b.into(), w.clamp(-1.0, 1.0));
}
pub(crate) fn grow(state: &mut State, tokens: &[String], budget: usize) -> usize {
    let mut n = 0;
    for t in tokens {
        if n >= budget || state.neuronen.len() >= 200_000 {
            break;
        }
        if word(state, t).is_some() {
            continue;
        }
        let id = format!("lex:{t}");
        state.neuronen.insert(
            id.clone(),
            Node {
                id: id.clone(),
                typ: "LEX".into(),
                label: t.clone(),
                staerke: state.number("start_staerke").clamp(0.0, 1.0),
                alter: "jung".into(),
                nutzt: 0,
                geboren: state.interaktionen,
                ring: None,
                extra: BTreeMap::new(),
            },
        );
        n += 1;
    }
    n
}
pub(crate) fn hear(state: &mut State, ts: &[String]) {
    let delta = state.number("staerke_zuwachs");
    for t in ts {
        if let Some(id) = word(state, t) {
            let n = state.neuronen.get_mut(&id).unwrap();
            n.staerke = (n.staerke + delta).clamp(0.0, 1.0);
        }
    }
    for pair in ts.windows(2) {
        if let (Some(a), Some(b)) = (word(state, &pair[0]), word(state, &pair[1])) {
            if a != b {
                let w = edge(state, &a, &b)
                    .map(|w| (w + state.number("paar_zuwachs")).min(state.number("paar_deckel")))
                    .unwrap_or(state.number("paar_basis"));
                set_edge(state, &a, &b, w);
            }
        }
    }
    // Store bounded second-order continuations. The lexical graph still owns
    // the words; this small side table learns which edge follows a two-word
    // context, avoiding the old single-edge greedy walk's role mixing.
    let contexts = state
        .extra
        .entry("sequence_contexts".into())
        .or_insert_with(|| serde_json::json!({}));
    if let Some(map) = contexts.as_object_mut() {
        for tri in ts.windows(3) {
            let key = format!("{}\u{1f}{}", tri[0], tri[1]);
            let entry = map.entry(key).or_insert_with(|| serde_json::json!({}));
            if let Some(nexts) = entry.as_object_mut() {
                let count = nexts.get(&tri[2]).and_then(|v| v.as_u64()).unwrap_or(0);
                nexts.insert(tri[2].clone(), serde_json::json!(count.saturating_add(1)));
            }
        }
        while map.len() > 20_000 {
            if let Some(first) = map.keys().next().cloned() {
                map.remove(&first);
            } else {
                break;
            }
        }
    }
}
#[derive(Clone, Default)]
pub(crate) struct Path {
    pub nodes: Vec<String>,
    pub edges: Vec<(String, String)>,
    pub alternative: Vec<(String, String)>,
    pub kern: Option<String>,
    pub weight: f64,
}
fn neighbors(
    state: &State,
    a: &str,
    prev: Option<&str>,
    used: &[String],
    context: Option<&str>,
) -> Vec<String> {
    let mut ns: Vec<_> = state
        .kanten
        .get(a)
        .into_iter()
        .flatten()
        .filter(|(id, _)| {
            used.iter().filter(|seen| *seen == *id).count() < 2
                && state.neuronen.get(*id).is_some_and(|n| n.typ == "LEX")
        })
        .collect();
    let continuation = prev.and_then(|p| {
        state
            .extra
            .get("sequence_contexts")
            .and_then(|v| {
                v.get(format!(
                    "{}\u{1f}{}",
                    state.neuronen[p].label, state.neuronen[a].label
                ))
            })
            .and_then(serde_json::Value::as_object)
    });
    ns.sort_by(|(a_id, x), (b_id, y)| {
        let a_order = continuation
            .and_then(|m| m.get(&state.neuronen[*a_id].label))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let b_order = continuation
            .and_then(|m| m.get(&state.neuronen[*b_id].label))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let feedback = |id: &str| {
            context
                .map(|q| {
                    crate::language::bias(
                        state,
                        q,
                        tokens(&state.neuronen[a].label).first().map(String::as_str),
                        tokens(&state.neuronen[id].label)
                            .first()
                            .map(String::as_str)
                            .unwrap_or(id),
                    )
                })
                .unwrap_or(0.0)
        };
        feedback(b_id)
            .total_cmp(&feedback(a_id))
            .then_with(|| b_order.cmp(&a_order))
            .then_with(|| y.total_cmp(x))
            .then(a_id.cmp(b_id))
    });
    ns.into_iter().map(|(id, _)| id.clone()).collect()
}
fn sentence_end(state: &State, ids: &[String]) -> bool {
    let words: Vec<_> = ids
        .iter()
        .filter_map(|id| tokens(&state.neuronen[id].label).into_iter().next())
        .collect();
    words.last().is_some_and(|w| {
        crate::language::end(state, words.iter().rev().nth(1).map(String::as_str), w)
    })
}
fn walk(state: &State, start: &str, context: Option<&str>) -> Path {
    let mut p = Path {
        nodes: vec![start.into()],
        ..Path::default()
    };
    while p.nodes.len() < (state.number("max_pfad_len") as usize) {
        let a = p.nodes.last().unwrap().clone();
        if sentence_end(state, &p.nodes) {
            break;
        }
        let prev = p.nodes.iter().rev().nth(1).map(String::as_str);
        let ns = neighbors(state, &a, prev, &p.nodes, context);
        if ns.is_empty() {
            break;
        }
        if p.alternative.is_empty() && ns.len() > 1 {
            let mut alt = p.nodes.clone();
            alt.push(ns[1].clone());
            while alt.len() < (state.number("max_pfad_len") as usize)
                && state.neuronen[alt.last().unwrap()].typ != "KERN"
                && !sentence_end(state, &alt)
            {
                let prev = alt.iter().rev().nth(1).map(String::as_str);
                let next = neighbors(state, alt.last().unwrap(), prev, &alt, context);
                if next.is_empty() {
                    break;
                }
                alt.push(next[0].clone());
            }
            p.alternative = alt
                .windows(2)
                .map(|a| (a[0].clone(), a[1].clone()))
                .collect();
        }
        let b = ns[0].clone();
        p.weight += edge(state, &a, &b).unwrap();
        p.edges.push((a, b.clone()));
        p.nodes.push(b.clone());
        if state.neuronen[&b].typ == "KERN" {
            break;
        }
    }
    if let Some(last) = p.nodes.last() {
        if state.neuronen[last].typ == "KERN" {
            p.kern = Some(last.clone());
        }
    }
    p
}
#[cfg(test)]
pub(crate) fn path(state: &State, starts: &[String]) -> Option<Path> {
    path_with_context(state, starts, None)
}
pub(crate) fn path_with_context(
    state: &State,
    starts: &[String],
    context: Option<&str>,
) -> Option<Path> {
    let mut best: Option<Path> = None;
    for s in starts {
        let mut p = walk(state, s, context);
        if let Some(context) = context {
            let mut prev: Option<String> = None;
            for id in &p.nodes {
                let word = tokens(&state.neuronen[id].label)
                    .into_iter()
                    .next()
                    .unwrap_or_default();
                p.weight += 10.0 * crate::language::bias(state, context, prev.as_deref(), &word);
                prev = Some(word);
            }
        }
        if best
            .as_ref()
            .is_none_or(|b| (p.kern.is_some(), p.weight) > (b.kern.is_some(), b.weight))
        {
            best = Some(p);
        }
    }
    best
}
pub(crate) fn consolidate(state: &mut State) {
    let ids: Vec<_> = state.neuronen.keys().cloned().collect();
    for id in ids {
        let n = &state.neuronen[&id];
        if n.typ != "LEX" || n.alter != "jung" {
            continue;
        }
        let delta = if n.nutzt >= state.number("praezise_ab_nutzt") as u64 {
            0.05
        } else if n.nutzt == 0 {
            -0.05
        } else {
            continue;
        };
        let mature = n.nutzt >= state.number("reif_ab_nutzt") as u64;
        let factor = state.number("daempf_faktor");
        let n = state.neuronen.get_mut(&id).unwrap();
        n.staerke = if delta > 0.0 {
            (n.staerke + 0.1).min(1.0)
        } else {
            (n.staerke * factor).clamp(0.01, 1.0)
        };
        if mature {
            n.alter = "reif".into();
        }
        for (a, es) in &mut state.kanten {
            for (b, w) in es {
                if a == &id || b == &id {
                    *w = (*w + delta).clamp(-1.0, 0.95);
                }
            }
        }
    }
    let _report = crate::healing::heal(state);
    state.letzte_slaap = state.interaktionen;
}
pub(crate) fn starts(state: &State, ts: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    ts.iter()
        .filter_map(|t| word(state, t))
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

// Language-neutral lexical corpus tokens. No parser, interpreter or code execution.
pub(crate) fn code_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            if !word.is_empty() {
                out.push(format!("code:word:{}", std::mem::take(&mut word)));
            }
            out.push(format!("code:char:{c}"));
        }
    }
    if !word.is_empty() {
        out.push(format!("code:word:{word}"));
    }
    out
}
