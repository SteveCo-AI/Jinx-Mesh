use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Node {
    pub id: String,
    pub typ: String,
    pub label: String,
    pub staerke: f64,
    pub alter: String,
    pub nutzt: u64,
    pub geboren: u64,
    pub ring: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct State {
    pub neuronen: BTreeMap<String, Node>,
    pub kanten: BTreeMap<String, BTreeMap<String, f64>>,
    pub konfig: BTreeMap<String, Value>,
    pub interaktionen: u64,
    pub letzte_slaap: u64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Independently loadable encrypted graph page. Nodes are placed by stable
/// SHA-256 prefix; every source node's outgoing edges live in the same page.
#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct GraphShard {
    pub id: u8,
    pub nodes: BTreeMap<String, Node>,
    pub edges: BTreeMap<String, BTreeMap<String, f64>>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct GraphSlice {
    pub nodes: BTreeMap<String, Node>,
    pub edges: BTreeMap<String, BTreeMap<String, f64>>,
}

impl State {
    pub fn graph_shards(&self) -> Vec<GraphShard> {
        let mut shards: Vec<GraphShard> = (0..=u8::MAX)
            .map(|id| GraphShard {
                id,
                ..GraphShard::default()
            })
            .collect();
        for (id, node) in &self.neuronen {
            let shard = stable_shard(id);
            shards[shard as usize]
                .nodes
                .insert(id.clone(), node.clone());
            if let Some(edges) = self.kanten.get(id) {
                shards[shard as usize]
                    .edges
                    .insert(id.clone(), edges.clone());
            }
        }
        shards
            .into_iter()
            .filter(|page| !page.nodes.is_empty())
            .collect()
    }
}

pub(crate) fn stable_shard(id: &str) -> u8 {
    use sha2::{Digest, Sha256};
    Sha256::digest(id.as_bytes())[0]
}
impl State {
    pub fn number(&self, key: &str) -> f64 {
        self.konfig[key].as_f64().expect("validated numeric config")
    }
    pub fn empty() -> Self {
        serde_json::from_str(include_str!("../fixtures/start.json"))
            .expect("mechanical empty state")
    }
    pub fn validate(&self) -> Result<()> {
        if self.neuronen.len() > 200_000
            || self.kanten.values().map(|v| v.len()).sum::<usize>() > 2_000_000
        {
            bail!("Graph-Limit");
        }
        for (id, n) in &self.neuronen {
            if id != &n.id
                || !["LEX", "RING", "KERN"].contains(&n.typ.as_str())
                || !n.staerke.is_finite()
                || !(0.0..=1.0).contains(&n.staerke)
            {
                bail!("Ungültiges Neuron");
            }
            if let Some(r) = &n.ring {
                if !self.neuronen.contains_key(r) {
                    bail!("Ungültiger Ring");
                }
            }
        }
        for (a, edges) in &self.kanten {
            if !self.neuronen.contains_key(a) {
                bail!("Fehlende Kantenquelle");
            }
            for (b, w) in edges {
                if !self.neuronen.contains_key(b) || !w.is_finite() || !(-1.0..=1.0).contains(w) {
                    bail!("Ungültige Kante");
                }
            }
        }
        for name in [
            "structure_observations",
            "context_feedback",
            "feedback_counts",
        ] {
            if let Some(value) = self.extra.get(name) {
                if !value.is_object() {
                    bail!("Ungültige Sprachmetadaten: {name}");
                }
            }
        }
        if let Some(target) = self.extra.get("feedback_target") {
            if target["context"].as_str().is_none()
                || target["words"]
                    .as_array()
                    .is_none_or(|a| a.len() > 128 || a.iter().any(|v| !v.is_string()))
            {
                bail!("Ungültiges Bewertungsziel");
            }
        }
        for key in [
            "auto_slaap_ab",
            "daempf_faktor",
            "max_neu_pro_nachricht",
            "max_pfad_len",
            "paar_basis",
            "paar_deckel",
            "paar_zuwachs",
            "praezise_ab_nutzt",
            "staerke_zuwachs",
            "start_staerke",
            "reif_ab_nutzt",
        ] {
            let Some(v) = self.konfig.get(key).and_then(Value::as_f64) else {
                bail!("Fehlende Konfiguration: {key}");
            };
            if !v.is_finite() || !(0.0..=1_000_000.0).contains(&v) {
                bail!("Ungültige Konfiguration");
            }
        }
        if !(1.0..=128.0).contains(&self.number("max_pfad_len"))
            || self.number("max_neu_pro_nachricht") > 128.0
            || self.number("auto_slaap_ab") < 1.0
        {
            bail!("Ungültiges Budget");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reply {
    #[serde(default)]
    pub feedback: Option<i8>,
    pub text: String,
    pub art: String,
    pub aktion: Option<String>,
    pub gelernt: bool,
    pub neue_neuronen: usize,
    #[serde(default)]
    pub activation: Option<ActivationStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActivationStatus {
    /// `complete` when the bounded frontier was exhausted; `budget_limited`
    /// when more graph records remained after this turn's activation budget.
    pub phase: String,
    pub nodes_loaded: usize,
    pub node_budget: usize,
    pub pages_loaded: usize,
    pub source_refs_seen: usize,
    pub sources_loaded: usize,
    pub frontier_remaining: usize,
    pub can_continue: bool,
    pub source_matches: Vec<SourceMatch>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceMatch {
    pub id: String,
    pub speaker: String,
    pub unix_time: u64,
    pub token_overlap: usize,
}
