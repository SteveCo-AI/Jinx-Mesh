//! Runtime-selectable archive retrieval strategies.
use crate::model::GraphSlice;
use anyhow::{bail, Result};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    pub episode: Value,
    pub score: usize,
}

pub(crate) trait RetrievalHandle: Send + Sync {
    fn id(&self) -> &'static str;
    fn search(
        &self,
        query: &str,
        episodes: &[Value],
        limit: usize,
        graph: &GraphSlice,
    ) -> Vec<Candidate>;
}

#[derive(Default)]
pub(crate) struct RetrievalController {
    handles: BTreeMap<&'static str, Box<dyn RetrievalHandle>>,
    order: Vec<&'static str>,
}

impl RetrievalController {
    pub fn register(&mut self, handle: Box<dyn RetrievalHandle>) -> Result<()> {
        let id = handle.id();
        if self.handles.contains_key(id) {
            bail!("duplicate retrieval handle: {id}");
        }
        self.order.push(id);
        self.handles.insert(id, handle);
        Ok(())
    }
    pub fn has(&self, id: &str) -> bool {
        self.handles.contains_key(id)
    }
    #[allow(dead_code)] // Lifecycle operation for runtime handler reconfiguration.
    pub fn unregister(&mut self, id: &str) -> bool {
        let Some(key) = self.handles.keys().copied().find(|key| *key == id) else {
            return false;
        };
        self.handles.remove(key);
        self.order.retain(|entry| *entry != key);
        true
    }
    #[allow(dead_code)] // Keeps controller reset/reinitialization deterministic.
    pub fn reset(&mut self) {
        self.handles.clear();
        self.order.clear();
    }
    pub fn get(&self, id: &str) -> Result<&dyn RetrievalHandle> {
        self.handles
            .get(id)
            .map(Box::as_ref)
            .ok_or_else(|| anyhow::anyhow!("unknown retrieval handle: {id}"))
    }
    pub fn ordered_ids(&self) -> &[&'static str] {
        &self.order
    }
    pub fn defaults() -> Result<Self> {
        let mut c = Self::default();
        c.register(Box::new(Lexical))?;
        c.register(Box::new(Temporal))?;
        c.register(Box::new(Contextual))?;
        Ok(c)
    }
}

struct Lexical;
impl RetrievalHandle for Lexical {
    fn id(&self) -> &'static str {
        "lexical"
    }
    fn search(
        &self,
        query: &str,
        episodes: &[Value],
        limit: usize,
        _: &GraphSlice,
    ) -> Vec<Candidate> {
        rank(query, episodes, limit, false)
    }
}
struct Temporal;
impl RetrievalHandle for Temporal {
    fn id(&self) -> &'static str {
        "temporal"
    }
    fn search(
        &self,
        query: &str,
        episodes: &[Value],
        limit: usize,
        _: &GraphSlice,
    ) -> Vec<Candidate> {
        // Time is a ranking signal available to an explicitly requested temporal
        // retrieval strategy; the query's words are not interpreted as time intent.
        let mut out = rank(query, episodes, usize::MAX, true);
        out.sort_by_key(|c| std::cmp::Reverse(c.episode["unix_time"].as_u64().unwrap_or(0)));
        out.truncate(limit.clamp(1, 32));
        out
    }
}
struct Contextual;
impl RetrievalHandle for Contextual {
    fn id(&self) -> &'static str {
        "contextual"
    }
    fn search(
        &self,
        query: &str,
        episodes: &[Value],
        limit: usize,
        graph: &GraphSlice,
    ) -> Vec<Candidate> {
        // Sparse vectors come only from learned lexical graph connections.
        let q = words(query);
        let query_vector = context_vector(&q, graph);
        let mut out: Vec<_> = episodes
            .iter()
            .filter_map(|e| {
                let text = e["text"].as_str()?;
                let w = words(text);
                let candidate_vector = context_vector(&w, graph);
                let similarity = cosine(&query_vector, &candidate_vector);
                let adjacency = q
                    .windows(2)
                    .filter(|pair| w.windows(2).any(|x| x == *pair))
                    .count();
                let score = (similarity * 1000.0).round() as usize + adjacency.min(20);
                (score > 0).then(|| Candidate {
                    episode: e.clone(),
                    score,
                })
            })
            .collect();
        out.sort_by_key(|c| std::cmp::Reverse(c.score));
        out.truncate(limit.clamp(1, 32));
        out
    }
}

fn context_vector(tokens: &[String], graph: &GraphSlice) -> BTreeMap<String, f64> {
    let mut vector = BTreeMap::new();
    for token in tokens {
        let id = graph.nodes.iter().find_map(|(id, node)| {
            (node.typ == "LEX" && node.label.eq_ignore_ascii_case(token)).then_some(id)
        });
        let Some(id) = id else { continue };
        if let Some(edges) = graph.edges.get(id) {
            for (target, weight) in edges {
                if *weight > 0.0 {
                    if let Some(node) = graph.nodes.get(target) {
                        *vector.entry(node.label.to_lowercase()).or_insert(0.0) += weight;
                    }
                }
            }
        }
        for (source, edges) in &graph.edges {
            if let Some(weight) = edges.get(id).filter(|weight| **weight > 0.0) {
                if let Some(node) = graph.nodes.get(source) {
                    *vector.entry(node.label.to_lowercase()).or_insert(0.0) += weight;
                }
            }
        }
    }
    vector
}

fn cosine(left: &BTreeMap<String, f64>, right: &BTreeMap<String, f64>) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let dot: f64 = left
        .iter()
        .filter_map(|(key, value)| right.get(key).map(|v| value * v))
        .sum();
    let norm = |v: &BTreeMap<String, f64>| v.values().map(|x| x * x).sum::<f64>().sqrt();
    (dot / (norm(left) * norm(right))).clamp(0.0, 1.0)
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
        .collect()
}
fn rank(query: &str, episodes: &[Value], limit: usize, recency_tie: bool) -> Vec<Candidate> {
    let terms = words(query);
    let mut out: Vec<_> = episodes
        .iter()
        .filter_map(|e| {
            let text = e["text"].as_str()?;
            let w = words(text);
            let score = terms.iter().filter(|q| w.iter().any(|x| x == *q)).count();
            (score > 0).then(|| Candidate {
                episode: e.clone(),
                score,
            })
        })
        .collect();
    out.sort_by_key(|c| {
        (
            std::cmp::Reverse(c.score),
            if recency_tie {
                std::cmp::Reverse(c.episode["unix_time"].as_u64().unwrap_or(0))
            } else {
                std::cmp::Reverse(0)
            },
        )
    });
    out.truncate(limit.clamp(1, 32));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Node;
    use std::collections::BTreeMap;
    #[test]
    fn registry_is_ordered_and_rejects_duplicates_and_unknown_ids() {
        let mut c = RetrievalController::defaults().unwrap();
        assert_eq!(c.ordered_ids(), &["lexical", "temporal", "contextual"]);
        assert!(c.has("lexical"));
        assert!(c.get("missing").is_err());
        assert!(c.register(Box::new(Lexical)).is_err());
    }
    #[test]
    fn unregister_reset_and_reinitialization_are_deterministic() {
        let mut c = RetrievalController::defaults().unwrap();
        assert!(c.unregister("temporal"));
        assert!(!c.has("temporal"));
        c.reset();
        assert!(c.ordered_ids().is_empty());
        let once = RetrievalController::defaults().unwrap();
        let twice = RetrievalController::defaults().unwrap();
        assert_eq!(once.ordered_ids(), twice.ordered_ids());
    }
    #[test]
    fn strategies_rank_text_context_and_time() {
        let data = vec![
            serde_json::json!({"text":"Ich heiße Lumen","unix_time":1}),
            serde_json::json!({"text":"Lumen heißt Jinx","unix_time":2}),
        ];
        let c = RetrievalController::defaults().unwrap();
        let graph = GraphSlice::default();
        assert_eq!(
            c.get("contextual")
                .unwrap()
                .search("ich heiße", &data, 3, &graph)[0]
                .episode["unix_time"],
            1
        );
        assert_eq!(
            c.get("temporal").unwrap().search("lumen", &data, 3, &graph)[0].episode["unix_time"],
            2
        );
    }

    #[test]
    fn learned_neighbor_vectors_match_without_literal_word_overlap() {
        let mut graph = GraphSlice::default();
        for (id, label) in [
            ("lex:tea", "tea"),
            ("lex:coffee", "coffee"),
            ("lex:drink", "drink"),
            ("lex:car", "car"),
        ] {
            graph.nodes.insert(
                id.into(),
                Node {
                    id: id.into(),
                    typ: "LEX".into(),
                    label: label.into(),
                    staerke: 0.5,
                    alter: "learned".into(),
                    nutzt: 1,
                    geboren: 1,
                    ring: None,
                    extra: BTreeMap::new(),
                },
            );
        }
        graph.edges.insert(
            "lex:tea".into(),
            BTreeMap::from([("lex:drink".into(), 0.8)]),
        );
        graph.edges.insert(
            "lex:coffee".into(),
            BTreeMap::from([("lex:drink".into(), 0.8)]),
        );
        graph
            .edges
            .insert("lex:car".into(), BTreeMap::from([("lex:road".into(), 0.8)]));
        graph.nodes.insert(
            "lex:road".into(),
            Node {
                id: "lex:road".into(),
                typ: "LEX".into(),
                label: "road".into(),
                staerke: 0.5,
                alter: "learned".into(),
                nutzt: 1,
                geboren: 1,
                ring: None,
                extra: BTreeMap::new(),
            },
        );
        let data = vec![
            serde_json::json!({"text":"coffee","unix_time":1}),
            serde_json::json!({"text":"car","unix_time":2}),
        ];
        let c = RetrievalController::defaults().unwrap();
        let hits = c.get("contextual").unwrap().search("tea", &data, 3, &graph);
        assert_eq!(hits[0].episode["text"], "coffee");
        assert!(!hits.iter().any(|hit| hit.episode["text"] == "tea"));
    }
}
