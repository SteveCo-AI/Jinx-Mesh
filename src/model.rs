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
            "kanal2_letzte",
            "fallback_pro_thema",
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
            // The Kap.-18 implicit channel compares the previous input's
            // tokens; pre-Block-2 targets simply lack the field.
            if let Some(eingabe) = target.get("eingabe") {
                if eingabe
                    .as_array()
                    .is_none_or(|a| a.len() > 128 || a.iter().any(|v| !v.is_string()))
                {
                    bail!("Ungültiges Bewertungsziel");
                }
            }
        }
        // Kap.-18 channel-2 slot, filled by the Kap.-19 fallback flow: an open
        // question and/or the latest recorded result value, both keyed by
        // theme digest and interaction number.
        if let Some(kanal2) = self.extra.get("kanal2_letzte") {
            for feld in ["offen", "ergebnis"] {
                let Some(eintrag) = kanal2.get(feld) else {
                    continue;
                };
                if eintrag["thema"].as_str().is_none() || eintrag["runde"].as_u64().is_none() {
                    bail!("Ungültige Kanal-2-Daten");
                }
                if feld == "ergebnis"
                    && eintrag["worte"]
                        .as_array()
                        .is_none_or(|a| a.len() > 128 || a.iter().any(|v| !v.is_string()))
                {
                    bail!("Ungültige Kanal-2-Daten");
                }
            }
        }
        // Kap.-19 fallback bookkeeping: the per-theme question counter map and
        // the scaffold rotation counter (usage counting).
        if let Some(zaehler) = self.extra.get("fallback_geruest_zaehler") {
            let Some(zaehler) = zaehler.as_f64() else {
                bail!("Ungültiger Gerüst-Zähler");
            };
            if !zaehler.is_finite() || !(0.0..=1_000_000_000.0).contains(&zaehler) {
                bail!("Ungültiger Gerüst-Zähler");
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
        if let Some(value) = self.konfig.get("journal_kap") {
            let Some(kap) = value.as_f64() else {
                bail!("Ungültige Konfiguration: journal_kap");
            };
            if !kap.is_finite()
                || !(crate::journal::KAP_MIN..=crate::journal::KAP_MAX).contains(&kap)
            {
                bail!("Ungültige Konfiguration: journal_kap");
            }
        }
        // Kap.-19 fallback configuration. Optional like the other mechanism
        // keys so vaults predating the flow (or its switch) load unchanged;
        // Engine::new restores the fixture defaults. The question scaffolds
        // and status texts are configuration data, never code strings; each
        // array carries several bounded non-empty variants so the rotation
        // has data to choose from. The `fallback_aktivier` switch ships
        // disarmed: only the vault's konfig data arms the flow.
        if let Some(value) = self.konfig.get("fallback_aktivier") {
            if value.as_bool().is_none() {
                bail!("Ungültige Konfiguration: fallback_aktivier");
            }
        }
        for (key, bereich) in [
            ("fallback_unklar_min", 0.0..=2.0),
            ("fallback_drossel_max", 1.0..=1000.0),
        ] {
            if let Some(value) = self.konfig.get(key) {
                let Some(v) = value.as_f64() else {
                    bail!("Ungültige Konfiguration: {key}");
                };
                if !v.is_finite() || !bereich.contains(&v) {
                    bail!("Ungültige Konfiguration: {key}");
                }
            }
        }
        for key in ["fallback_frage_geruest", "fallback_status_durchsage"] {
            if let Some(value) = self.konfig.get(key) {
                let Some(varianten) = value.as_array() else {
                    bail!("Ungültige Konfiguration: {key}");
                };
                if varianten.is_empty()
                    || varianten.len() > 64
                    || varianten
                        .iter()
                        .any(|v| v.as_str().is_none_or(|s| s.is_empty() || s.len() > 256))
                {
                    bail!("Ungültige Konfiguration: {key}");
                }
            }
        }
        // Kap.-21/22 self-healing configuration. Optional like the Kap.-18/19
        // keys so vaults predating the mechanism load unchanged; Engine::new
        // restores the fixture defaults. The command formulations in
        // heal_befehl_muster are pattern data, never code strings.
        for (key, bereich) in [
            ("folding_schwelle", 0.0..=10.0),
            ("folding_min_nutzt", 0.0..=1_000_000.0),
            ("verdacht_schwelle", -1.0..=0.0),
            ("verdacht_min_nutzt", 0.0..=1_000_000.0),
            ("heil_schritt", 0.0..=1.0),
            ("heil_schwelle_min", 0.0..=1.0),
            ("heil_schwelle_max", 0.0..=1.0),
        ] {
            if let Some(value) = self.konfig.get(key) {
                let Some(v) = value.as_f64() else {
                    bail!("Ungültige Konfiguration: {key}");
                };
                if !v.is_finite() || !bereich.contains(&v) {
                    bail!("Ungültige Konfiguration: {key}");
                }
            }
        }
        if let (Some(minimum), Some(maximum)) = (
            self.konfig.get("heil_schwelle_min").and_then(Value::as_f64),
            self.konfig.get("heil_schwelle_max").and_then(Value::as_f64),
        ) {
            if minimum > maximum {
                bail!("heil_schwelle_min übersteigt heil_schwelle_max");
            }
        }
        if let Some(muster) = self.konfig.get("heal_befehl_muster") {
            let Some(muster) = muster.as_object() else {
                bail!("Ungültige Konfiguration: heal_befehl_muster");
            };
            // The falsch pattern is "{x}<trenner>{y}" with a non-empty
            // literal separator; the echtes_wort pattern is exactly "{x}".
            for (key, mit_y) in [("falsch", true), ("echtes_wort", false)] {
                let Some(text) = muster.get(key).and_then(Value::as_str) else {
                    bail!("Ungültige Konfiguration: heal_befehl_muster");
                };
                if text.len() > 256
                    || text.matches("{x}").count() != 1
                    || text.matches("{y}").count() != usize::from(mit_y)
                    || (mit_y
                        && text
                            .strip_prefix("{x}")
                            .and_then(|rest| rest.strip_suffix("{y}"))
                            .is_none_or(|trenner| {
                                trenner.trim().is_empty() || trenner.contains('{')
                            }))
                {
                    bail!("Ungültige Konfiguration: heal_befehl_muster");
                }
            }
        }
        // Kap.-24 integration-gate rule data: every declared column must be a
        // known, well-shaped rule the gate registry can interpret — a
        // declared rule that silently passes would be fake safety. The key
        // is optional so vaults predating the gate load unchanged;
        // Engine::new restores the fixture defaults.
        if let Some(regeln) = self.konfig.get(crate::gate::KONFIG_KEY) {
            if !crate::gate::konfig_pruefen(regeln) {
                bail!("Ungültige Konfiguration: gate_regeln");
            }
        }
        // Kap.-21 learned folding-threshold override (a user correction raised
        // the effective distance; the konfig value stays the start value).
        if let Some(value) = self.extra.get("folding_schwelle") {
            let Some(v) = value.as_f64() else {
                bail!("Ungültige Folding-Schwelle");
            };
            if !v.is_finite() || !(0.0..=10.0).contains(&v) {
                bail!("Ungültige Folding-Schwelle");
            }
        }
        // Kap.-22 verification-source registry: per level a source designator
        // (data, possibly empty — a level without a source simply may not
        // heal), a verification credit counter, and the level's heal threshold.
        if let Some(registry) = self.extra.get("verifikations_ebenen") {
            let Some(ebenen) = registry.as_object() else {
                bail!("Ungültige Verifikations-Ebenen");
            };
            if ebenen.len() > 16 {
                bail!("Ungültige Verifikations-Ebenen");
            }
            for eintrag in ebenen.values() {
                let Some(felder) = eintrag.as_object() else {
                    bail!("Ungültige Verifikations-Ebenen");
                };
                if let Some(quelle) = felder.get("quelle") {
                    if quelle.as_str().is_none() {
                        bail!("Ungültige Verifikations-Ebenen");
                    }
                }
                let Some(credit) = felder.get("verification_credit") else {
                    bail!("Ungültige Verifikations-Ebenen");
                };
                if credit.as_u64().is_none() {
                    bail!("Ungültige Verifikations-Ebenen");
                }
                let Some(schwelle) = felder.get("heil_schwelle") else {
                    bail!("Ungültige Verifikations-Ebenen");
                };
                let Some(schwelle) = schwelle.as_f64() else {
                    bail!("Ungültige Verifikations-Ebenen");
                };
                if !schwelle.is_finite() || !(0.0..=1.0).contains(&schwelle) {
                    bail!("Ungültige Verifikations-Ebenen");
                }
            }
        }
        // Kap.-21 folding aliases and the Kap.-21 suspicion ledger keep their
        // shape checkable so vaulted metadata stays honest.
        if let Some(alias) = self.extra.get("neuron_aliases") {
            let Some(alias) = alias.as_object() else {
                bail!("Ungültige Neuron-Aliase");
            };
            if alias.values().any(|v| v.as_str().is_none()) {
                bail!("Ungültige Neuron-Aliase");
            }
        }
        if let Some(verzeichnis) = self.extra.get("verdachtsverzeichnis") {
            if verzeichnis.as_array().is_none() {
                bail!("Ungültiges Verdachtsverzeichnis");
            }
        }
        // Phase-2.5 mutation configuration (novelty operator). Optional like
        // the Kap.-18/19 keys so vaults predating the layer load unchanged;
        // Engine::new restores the fixture defaults. Every switch and
        // threshold is data — the layer is disarmed by default (SICHERHEIT:
        // proposals only, konservativ).
        if let Some(value) = self.konfig.get("mutation_aktivier") {
            if value.as_bool().is_none() {
                bail!("Ungültige Konfiguration: mutation_aktivier");
            }
        }
        for (key, bereich) in [
            ("mutation_rate", 0.0..=1.0),
            ("mutation_temperatur", 0.0..=1.0),
            ("mutation_schwellwert", 0.0..=1.0),
            ("mutation_langeweile_schwelle", 0.0..=1.0),
            ("mutation_wildheit_schritt", 0.0..=1.0),
            ("mutation_langeweile_ab_versuchen", 1.0..=1000.0),
        ] {
            if let Some(value) = self.konfig.get(key) {
                let Some(v) = value.as_f64() else {
                    bail!("Ungültige Konfiguration: {key}");
                };
                if !v.is_finite() || !bereich.contains(&v) {
                    bail!("Ungültige Konfiguration: {key}");
                }
            }
        }
        // Phase-2.5 mutation bookkeeping: shape-checked so vaulted metadata
        // stays honest. The wegweiser learns environments (never words), the
        // memory ledger keeps every mutant's fate, the balance counts the
        // external-signal-only festigungen, and the pending candidate carries
        // the proposal awaiting its signal.
        if let Some(wegweiser) = self.extra.get("mutations_wegweiser") {
            let Some(eintraege) = wegweiser.as_object() else {
                bail!("Ungültiger Mutations-Wegweiser");
            };
            if eintraege.len() > 4096 {
                bail!("Ungültiger Mutations-Wegweiser");
            }
            for eintrag in eintraege.values() {
                let (Some(versuche), Some(erfolge)) =
                    (eintrag["versuche"].as_u64(), eintrag["erfolge"].as_u64())
                else {
                    bail!("Ungültiger Mutations-Wegweiser");
                };
                if erfolge > versuche {
                    bail!("Ungültiger Mutations-Wegweiser");
                }
            }
        }
        if let Some(wildheit) = self.extra.get("mutation_wildheit") {
            let Some(wildheit) = wildheit.as_f64() else {
                bail!("Ungültige Mutations-Wildheit");
            };
            if !wildheit.is_finite() || !(0.0..=1.0).contains(&wildheit) {
                bail!("Ungültige Mutations-Wildheit");
            }
        }
        if let Some(bilanz) = self.extra.get("mutation_bilanz") {
            let Some(felder) = bilanz.as_object() else {
                bail!("Ungültige Mutations-Bilanz");
            };
            for feld in [
                "versuche",
                "verworfen",
                "festigungen",
                "negativ",
                "verfallen",
            ] {
                if felder.get(feld).is_some_and(|v| v.as_u64().is_none()) {
                    bail!("Ungültige Mutations-Bilanz");
                }
            }
        }
        if let Some(kandidat) = self.extra.get("mutation_kandidat") {
            if kandidat["wort"]
                .as_str()
                .is_none_or(|w| w.is_empty() || w.len() > 1024)
                || kandidat["kontext"]
                    .as_str()
                    .is_none_or(|c| c.is_empty() || c.len() > 128)
                || kandidat["stelle"].as_u64().is_none_or(|s| s > 128)
                || kandidat["runde"].as_u64().is_none()
            {
                bail!("Ungültiger Mutations-Kandidat");
            }
            for feld in ["prev", "next"] {
                if let Some(wort) = kandidat[feld].as_str() {
                    if wort.is_empty() || wort.len() > 1024 {
                        bail!("Ungültiger Mutations-Kandidat");
                    }
                }
            }
        }
        if let Some(memory) = self.extra.get("mutations_memory") {
            let Some(liste) = memory.as_array() else {
                bail!("Ungültiges Mutations-Memory");
            };
            if liste.len() > 256
                || liste.iter().any(|eintrag| {
                    eintrag.as_object().is_none_or(|e| {
                        e.get("schicksal")
                            .and_then(Value::as_str)
                            .is_none_or(|s| s.len() > 32)
                            || e.get("wort")
                                .and_then(Value::as_str)
                                .is_none_or(|w| w.is_empty() || w.len() > 1024)
                    })
                })
            {
                bail!("Ungültiges Mutations-Memory");
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
    /// Kap.-19 fallback utterance appended to this move: "nachfrage" or
    /// "durchsage" (mechanical marker, never behavior text).
    #[serde(default)]
    pub fallback: Option<String>,
    /// Phase-2.5 mutation proposal in this answer: the inserted word (a
    /// learned or fresh word from data, never a code string). None when the
    /// layer is disarmed, rate-limited, or the mutant failed selection.
    #[serde(default)]
    pub mutation: Option<String>,
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
