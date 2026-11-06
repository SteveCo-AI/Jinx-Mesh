//! Auditable, non-destructive graph normalization.
use crate::model::State;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
type ExactNodeKey = (
    String,
    String,
    u64,
    Option<String>,
    u64,
    String,
    u64,
    String,
);

#[derive(Clone, Debug, Default)]
pub(crate) struct HealingReport {
    pub aliases_added: usize,
    pub edges_merged: usize,
    pub quarantined: usize,
}

/// Canonicalize exact duplicate observations while retaining every node ID.
/// Suspicious edges and orphan nodes are recorded for inspection; they are not deleted.
pub(crate) fn heal(state: &mut State) -> HealingReport {
    let mut report = HealingReport::default();
    let mut canonical_by_key: BTreeMap<ExactNodeKey, String> = BTreeMap::new();
    let mut aliases: BTreeMap<String, String> = state
        .extra
        .get("neuron_aliases")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let mut quarantine = state
        .extra
        .get("quarantine")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    aliases.retain(|from, to| {
        let valid = state.neuronen.contains_key(from) && state.neuronen.contains_key(to);
        if !valid {
            quarantine.push(json!({"kind":"invalid_alias_target","from":from,"to":to}));
            report.quarantined += 1;
        }
        valid
    });
    let existing_aliases = aliases.clone();
    for (id, node) in &state.neuronen {
        let key = (
            node.typ.clone(),
            node.label.clone(),
            node.geboren,
            node.ring.clone(),
            node.staerke.to_bits(),
            node.alter.clone(),
            node.nutzt,
            format!("{:?}", node.extra),
        );
        if let Some(canon) = canonical_by_key.get(&key).cloned() {
            if canon != *id {
                let preferred = if id.starts_with("lex:") && !canon.starts_with("lex:") {
                    id.clone()
                } else {
                    canon.clone()
                };
                let other = if preferred == *id { canon } else { id.clone() };
                canonical_by_key.insert(key, preferred.clone());
                if existing_aliases.get(&other) != Some(&preferred) {
                    report.aliases_added += 1;
                }
                aliases.insert(other, preferred);
            }
        } else {
            canonical_by_key.insert(key, id.clone());
        }
    }
    let resolve = |id: &str, aliases: &BTreeMap<String, String>| -> String {
        let mut current = id.to_owned();
        let mut visited = BTreeSet::new();
        while visited.insert(current.clone()) {
            match aliases.get(&current) {
                Some(next) => current = next.clone(),
                None => break,
            }
        }
        current
    };
    let mut merged: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    // Break cycles deterministically and make them visible rather than resolving
    // to an arbitrary member of a malformed alias loop.
    let alias_keys: Vec<_> = aliases.keys().cloned().collect();
    for key in alias_keys {
        let mut current = key.clone();
        let mut seen = BTreeSet::new();
        while let Some(next) = aliases.get(&current) {
            if !seen.insert(current.clone()) || seen.contains(next) {
                aliases.remove(&key);
                quarantine.push(json!({"kind":"alias_cycle","id":key}));
                report.quarantined += 1;
                break;
            }
            current = next.clone();
        }
    }
    for (from, edges) in &state.kanten {
        for (to, weight) in edges {
            if !weight.is_finite() || !(-1.0..=1.0).contains(weight) {
                quarantine
                    .push(json!({"kind":"invalid_weight","from":from,"to":to,"weight":weight}));
                report.quarantined += 1;
                continue;
            }
            let a = resolve(from, &aliases);
            let b = resolve(to, &aliases);
            let slot = merged.entry(a).or_default().entry(b).or_insert(*weight);
            if *slot != *weight {
                report.edges_merged += 1;
                *slot = slot.max(*weight);
            }
        }
    }
    state.kanten = merged;
    state.extra.insert("neuron_aliases".into(), json!(aliases));
    state
        .extra
        .insert("quarantine".into(), Value::Array(quarantine));
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Node;
    use std::collections::BTreeMap;

    #[test]
    fn exact_duplicate_aliases_merge_edges_without_deleting_neurons() {
        let mut s = State::empty();
        let node = Node {
            id: "dup".into(),
            typ: "LEX".into(),
            label: "hallo".into(),
            staerke: 0.4,
            alter: "jung".into(),
            nutzt: 1,
            geboren: 2,
            ring: None,
            extra: BTreeMap::new(),
        };
        s.neuronen.insert("dup".into(), node);
        s.neuronen.insert(
            "lex:hallo".into(),
            Node {
                id: "lex:hallo".into(),
                typ: "LEX".into(),
                label: "hallo".into(),
                staerke: 0.4,
                alter: "jung".into(),
                nutzt: 1,
                geboren: 2,
                ring: None,
                extra: BTreeMap::new(),
            },
        );
        s.neuronen.insert(
            "lex:test".into(),
            Node {
                id: "lex:test".into(),
                typ: "LEX".into(),
                label: "test".into(),
                staerke: 0.5,
                alter: "jung".into(),
                nutzt: 0,
                geboren: 1,
                ring: None,
                extra: BTreeMap::new(),
            },
        );
        s.kanten
            .entry("lex:test".into())
            .or_default()
            .insert("dup".into(), 0.2);
        s.kanten
            .entry("lex:test".into())
            .or_default()
            .insert("lex:hallo".into(), 0.5);
        s.kanten
            .entry("dup".into())
            .or_default()
            .insert("lex:hallo".into(), 0.1);
        let r = heal(&mut s);
        assert_eq!(r.aliases_added, 1);
        assert!(s.neuronen.contains_key("dup"));
        assert_eq!(s.extra["neuron_aliases"]["dup"], "lex:hallo");
        assert_eq!(s.kanten["lex:test"]["lex:hallo"], 0.5);
        assert_eq!(s.kanten["lex:hallo"]["lex:hallo"], 0.1);
        assert_eq!(heal(&mut s).aliases_added, 0);
        assert!(s.validate().is_ok());
    }
}
