use crate::model::{Node, State};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn tokens(text: &str) -> Vec<String> {
    crate::language::parse(text).words
}
/// Name-based lexical lookup WITHOUT alias resolution: the node whose id or
/// legacy label carries the token. The Kap.-21 heal commands address a word
/// by its name even when it is already folded into another word.
pub(crate) fn wort_namentlich(state: &State, t: &str) -> Option<String> {
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
/// Follow the `neuron_aliases` chain while every hop is a resident node; the
/// last resident member is the token's canonical word (Kap. 21). A cold hop
/// is a paging artifact and stops the walk instead of inventing a missing id.
pub(crate) fn alias_aufloesung(state: &State, start: &str) -> String {
    let alias = state.extra.get("neuron_aliases").and_then(Value::as_object);
    let mut aktuell = start.to_owned();
    let mut besucht = BTreeSet::new();
    while besucht.insert(aktuell.clone()) {
        let Some(next) = alias.and_then(|a| a.get(&aktuell)).and_then(Value::as_str) else {
            break;
        };
        if !state.neuronen.contains_key(next) {
            break;
        }
        aktuell = next.to_owned();
    }
    aktuell
}
pub(crate) fn word(state: &State, t: &str) -> Option<String> {
    let id = wort_namentlich(state, t)?;
    // The resolution result is always a resident node: the chain only hops
    // onto resident ids, so callers can index state.neuronen with it.
    Some(alias_aufloesung(state, &id))
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
    // Kap. 21/22 fold gate, read once: None while the word level has no
    // registered verification source — then no fold may even be proposed.
    let gate = crate::healing::fold_gate(state);
    for t in tokens {
        if n >= budget || state.neuronen.len() >= 200_000 {
            break;
        }
        if word(state, t).is_some() {
            continue;
        }
        // Kap. 21: the code corpus namespace (code:word:/code:char: prefixed
        // tokens, see code_tokens) is no spelling — it never folds.
        if !t.contains(':') {
            if let Some(gate) = &gate {
                if let Some((kanon, label, distanz)) = crate::healing::fold_kandidat(state, t, gate)
                {
                    if crate::healing::fold_beweis(t, &label, distanz) >= gate.schwelle {
                        let id = format!("lex:{t}");
                        let mut extra = BTreeMap::new();
                        // The origin stays visible: the node keeps its label,
                        // ring, alias entry, and this marker (non-destructive).
                        extra.insert("folding_origin".into(), serde_json::json!(true));
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
                                ring: Some(kanon.clone()),
                                extra,
                            },
                        );
                        let alias = state
                            .extra
                            .entry("neuron_aliases".into())
                            .or_insert_with(|| serde_json::json!({}));
                        if let Some(map) = alias.as_object_mut() {
                            map.insert(id, serde_json::json!(kanon));
                        }
                        // The token's edges route through the alias from now
                        // on (word() resolves it to the canonical node), and
                        // the confirmed fold grows the word level's credit.
                        crate::healing::erfolg(state, crate::healing::EBENE_WORT);
                        n += 1;
                        continue;
                    }
                    // The level proposed a fold and the verification source
                    // forbade it: a failed heal (Kap. 22), the word stays its
                    // own independent node.
                    crate::healing::misserfolg(state, crate::healing::EBENE_WORT);
                }
            }
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
    // Kap. 21: the consistency check of every learned sequence edge runs in
    // the consolidation round — heavily used edges the confirmed path
    // weights contradict get damped into the suspicion ledger, not deleted.
    let _verdaechte = crate::healing::verdachts_pruefung(state);
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
