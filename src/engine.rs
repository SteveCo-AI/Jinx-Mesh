use crate::{
    graph, language,
    model::{GraphSlice, Reply, State},
};
use anyhow::{bail, Result};
use serde_json::Value;

/// Linguistic output comes only from observed tokens and learned lexical edges.
/// Imported demo metadata is preserved by storage, never executed or verbalized.
pub(crate) struct Engine {
    pub state: State,
}
impl Engine {
    pub fn new(mut state: State, _legacy_spec: Value) -> Result<Self> {
        state.validate()?;
        // Upgrade the original twelve-word defaults in existing vaults as well.
        // Learned nodes and connections are preserved.
        for key in ["max_neu_pro_nachricht", "max_pfad_len"] {
            if state.number(key) == 12.0 {
                state.konfig.insert(key.into(), serde_json::json!(128));
            }
        }
        Ok(Self { state })
    }
    pub fn infer(&self, text: &str) -> Result<Reply> {
        validate_text(text)?;
        let tokens = graph::tokens(text);
        let path = graph::path_with_context(
            &self.state,
            &graph::starts(&self.state, &tokens),
            Some(&language::context(text)),
        );
        Ok(Reply {
            text: path
                .as_ref()
                .map(|p| language::render_path(&self.state, &p.nodes, &language::context(text)))
                .unwrap_or_default(),
            art: if path.is_some() {
                "inferenz"
            } else {
                "unbekannt"
            }
            .into(),
            aktion: None,
            gelernt: false,
            neue_neuronen: 0,
            activation: None,
            feedback: None,
        })
    }
    #[cfg(test)]
    pub fn train(&mut self, text: &str) -> Result<Reply> {
        let source = format!("episode:{}", self.state.interaktionen.saturating_add(1));
        self.train_with_source(text, &source)
    }

    pub(crate) fn train_with_source(&mut self, text: &str, source_id: &str) -> Result<Reply> {
        validate_text(text)?;
        let (feedback, text) = language::split_feedback(text);
        if let Some(value) = feedback {
            language::apply_feedback(&mut self.state, value)?;
        }
        let parsed = language::parse(text);
        let tokens = parsed.words.clone();
        let had_known = tokens.iter().any(|t| graph::word(&self.state, t).is_some());
        self.state.interaktionen = self
            .state
            .interaktionen
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Interaktionslimit"))?;
        let budget = (self.state.number("max_neu_pro_nachricht") as usize)
            .min(128usize.saturating_sub(self.state.neuronen.len()));
        let added = graph::grow(&mut self.state, &tokens, budget);
        graph::hear(&mut self.state, &tokens);
        language::observe(&mut self.state, &parsed);
        for token in &tokens {
            if let Some(id) = graph::word(&self.state, token) {
                let node = self
                    .state
                    .neuronen
                    .get_mut(&id)
                    .expect("resolved lexical node");
                let refs = node
                    .extra
                    .entry("source_episode_ids".into())
                    .or_insert_with(|| serde_json::json!([]));
                if let Some(items) = refs.as_array_mut() {
                    if !items.iter().any(|v| v.as_str() == Some(source_id)) {
                        items.push(serde_json::Value::String(source_id.to_owned()));
                        if items.len() > 64 {
                            items.remove(0);
                        }
                    }
                }
            }
        }
        let path = graph::path_with_context(
            &self.state,
            &graph::starts(&self.state, &tokens),
            Some(&language::context(text)),
        );
        let output = if !had_known {
            language::render(&tokens, &parsed.marks)
        } else {
            path.as_ref()
                .map(|p| language::render_path(&self.state, &p.nodes, &language::context(text)))
                .unwrap_or_default()
        };
        if !output.is_empty() {
            language::remember(&mut self.state, text, &output);
        }
        if let Some(path) = path {
            for id in path.nodes {
                let n = self.state.neuronen.get_mut(&id).unwrap();
                n.nutzt = n.nutzt.saturating_add(1);
            }
        }
        if self
            .state
            .interaktionen
            .saturating_sub(self.state.letzte_slaap)
            >= self.state.number("auto_slaap_ab") as u64
        {
            graph::consolidate(&mut self.state);
        }
        self.state.validate()?;
        Ok(Reply {
            text: output,
            art: if had_known { "imitation" } else { "spiegel" }.into(),
            aktion: None,
            gelernt: true,
            neue_neuronen: added,
            activation: None,
            feedback,
        })
    }

    /// Preserve the loaded graph records while exposing only a valid, bounded
    /// neighborhood to the existing lexical walker. References to cold nodes
    /// remain in `records` and are not treated as missing data.
    pub(crate) fn active_state(metadata: &State, records: &GraphSlice) -> State {
        let mut state = metadata.clone();
        state.neuronen = records.nodes.clone();
        state.kanten = records
            .edges
            .iter()
            .filter(|(source, _)| records.nodes.contains_key(*source))
            .map(|(source, edges)| {
                (
                    source.clone(),
                    edges
                        .iter()
                        .filter(|(target, _)| records.nodes.contains_key(*target))
                        .map(|(target, weight)| (target.clone(), *weight))
                        .collect(),
                )
            })
            .collect();
        for node in state.neuronen.values_mut() {
            if node
                .ring
                .as_ref()
                .is_some_and(|ring| !records.nodes.contains_key(ring))
            {
                node.ring = None;
            }
        }
        state
    }

    /// Apply mutable node/edge values learned in the active neighborhood back
    /// onto the paged records. Cold outgoing edges are retained verbatim.
    pub(crate) fn merge_active(records: &mut GraphSlice, active: &State) {
        for (id, node) in &active.neuronen {
            let mut updated = node.clone();
            // `active_state` clears ring links whose target was not loaded so
            // strict State validation and graph walking remain safe. That is
            // a projection detail, never a learned deletion.
            if updated.ring.is_none() {
                updated.ring = records.nodes.get(id).and_then(|old| old.ring.clone());
            }
            records.nodes.insert(id.clone(), updated);
        }
        for (source, edges) in &active.kanten {
            let stored = records.edges.entry(source.clone()).or_default();
            for (target, weight) in edges {
                stored.insert(target.clone(), *weight);
            }
        }
    }
}
fn validate_text(text: &str) -> Result<()> {
    if text.len() > 16384 {
        bail!("Nachricht zu lang (16384 UTF-8-Bytes)");
    }
    if text.trim_start().starts_with('/') {
        bail!("Befehle werden nicht als Lerntext verarbeitet");
    }
    Ok(())
}

#[cfg(test)]
mod paging_tests {
    use super::*;
    use crate::model::Node;
    use std::collections::BTreeMap;

    fn node(id: &str, ring: Option<&str>) -> Node {
        Node {
            id: id.into(),
            typ: if id.starts_with("ring:") {
                "RING"
            } else {
                "LEX"
            }
            .into(),
            label: id.into(),
            staerke: 0.5,
            alter: "jung".into(),
            nutzt: 0,
            geboren: 1,
            ring: ring.map(str::to_owned),
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn active_slice_roundtrip_preserves_cold_ring_and_outgoing_edge() {
        let metadata = State::empty();
        let mut records = GraphSlice::default();
        records
            .nodes
            .insert("lex:hot".into(), node("lex:hot", Some("ring:cold")));
        records
            .edges
            .entry("lex:hot".into())
            .or_default()
            .insert("lex:cold".into(), 0.8);

        let active = Engine::active_state(&metadata, &records);
        assert!(active.neuronen["lex:hot"].ring.is_none());
        assert!(active.kanten["lex:hot"].is_empty());
        active.validate().unwrap();

        Engine::merge_active(&mut records, &active);
        assert_eq!(records.nodes["lex:hot"].ring.as_deref(), Some("ring:cold"));
        assert_eq!(records.edges["lex:hot"]["lex:cold"], 0.8);
    }
}
