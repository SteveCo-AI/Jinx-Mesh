//! Op-Journal and exact rollback for every learning step (Kap. 14).
//!
//! Persistence decision: the journal lives in its own encrypted store table
//! (`op_journal`, one sealed blob per entry keyed by a plain sequence), not in
//! `state.extra`. The reason is the write-ahead requirement: an entry must be
//! durable BEFORE the state change it describes persists, while `state.extra`
//! only ever reaches the vault together with the state itself. The store
//! already pages auxiliary authenticated data into dedicated tables
//! (conversation_archive, traversal_checkpoints, graph_records); this table
//! follows exactly that layout. Vault compatibility: `Store::open` creates the
//! table with IF NOT EXISTS and `Payload::op_journal_seq` defaults to 0, so
//! existing vault files load unchanged.
//!
//! Confirmation model (mirrors the existing commit pattern in app.rs, where
//! message() persists everything at the end): a learning step appends its
//! entry first (unconfirmed), then the single learning commit carries
//! `payload.op_journal_seq = entry.seq`. If the process dies between the two
//! writes, the state change never committed, so the next unlock discards the
//! entry and reports the count honestly at the first /status.
//!
//! Exactness: every op stores complete old and new values — whole nodes,
//! single edge weights, per-sub-key state.extra values. Rolling back writes
//! the stored `alt` verbatim; nothing is approximated or "similarly
//! restored". Because undo always pops a suffix of the journal, the ops of a
//! step meet exactly the state that step produced.
//!
//! Scope: only the affected learning section is journaled — the nodes and
//! edges of the active neighborhood plus the learned state.extra runs
//! (structure_observations, context_feedback, feedback_counts,
//! feedback_target, and any other learned key). Dialog bookkeeping is
//! deliberately NOT rolled back: interaktionen, letzte_slaap, konfig and the
//! conversation archive are history, so the dialog stays in memory while only
//! learning is reversible (the same boundary as the Python reference).
//!
//! Entry arts: "learn" (one message()/train_file() learning move — a Kap.-19
//! fallback move is an ordinary learn entry; its ops record the theme counter,
//! scaffold rotation, channel-2 slot, and the removed feedback target),
//! "feedback" (a move whose explicit +/- rating, Kap.-18 anchor word, or anker-weight
//! maturation drives the change), "consolidate" (one consolidate()
//! round; it always leaves an entry so a round boundary exists even when
//! nothing changed), "heal" (reserved for the Block-4 dialog commands),
//! "integration_reject" (a Kap.-24 gate refusal: no ops, the move committed
//! nothing; the entry marks the rejected interaction).
//!
//! Rounds: a "consolidate" entry marks a round boundary. /undo round rolls
//! back the suffix starting at the most recent boundary (inclusive); without a
//! boundary the whole journal is the open round.
use crate::model::{GraphSlice, Node, State};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub(crate) const ART_LEARN: &str = "learn";
pub(crate) const ART_CONSOLIDATE: &str = "consolidate";
/// The Kap.-21/22 dialog heal commands journal under this art; every heal
/// is exactly rollbackable like a learning step.
pub(crate) const ART_HEAL: &str = "heal";
pub(crate) const ART_FEEDBACK: &str = "feedback";
/// A Kap.-24 gate rejection journals the refused move with this art before
/// its state change commits; the entry carries no ops (nothing changed) and
/// stays rollbackable like every learning step.
pub(crate) const ART_INTEGRATION_REJECT: &str = "integration_reject";
pub(crate) const KAP_STANDARD: f64 = 500.0;
pub(crate) const KAP_MIN: f64 = 1.0;
pub(crate) const KAP_MAX: f64 = 500.0;

/// One exactly reversible change of the affected learning section.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Op {
    pub kind: String,
    /// Node id, edge source, or state.extra key.
    pub ziel: String,
    /// Edge target or state.extra sub-key.
    pub name: Option<String>,
    pub alt: Option<Value>,
    pub neu: Option<Value>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct JournalEintrag {
    pub seq: u64,
    pub interaktionsnummer: u64,
    pub art: String,
    pub zeit: u64,
    pub ops: Vec<Op>,
}

/// Journal and rollback bookkeeping a single commit must carry out:
/// the confirmation seq of a freshly appended entry, the suffix deletion
/// of undone entries, and the graph records undone neuron ops removed.
#[derive(Default)]
pub(crate) struct JournalVermerk {
    pub bestaetigt: Option<u64>,
    pub streichen_ab: Option<u64>,
    pub graph_entfernungen: Vec<String>,
}

/// Read-only view of one side of a learning step: the paged graph records
/// plus the metadata whose extra runs belong to the step.
pub(crate) struct Ausschnitt<'a> {
    pub cache: &'a GraphSlice,
    pub meta: &'a State,
}

/// Configured journal cap; vaults predating the key fall back to the default.
pub(crate) fn kap(state: &State) -> usize {
    state
        .konfig
        .get("journal_kap")
        .and_then(Value::as_f64)
        .unwrap_or(KAP_STANDARD)
        .clamp(KAP_MIN, KAP_MAX) as usize
}

/// Honest recovery notice for the first /status after a vault load.
pub(crate) fn verwurf_hinweis(verworfen: u64) -> String {
    if verworfen == 1 {
        "1 unbestätigter Schritt verworfen".into()
    } else {
        format!("{verworfen} unbestätigte Schritte verworfen")
    }
}

/// Build one entry as the exact diff of the affected section. Ops are ordered
/// neurons, then edges, then extra runs, so reversed application removes
/// edges before the nodes they point to.
pub(crate) fn eintrag(
    seq: u64,
    interaktionsnummer: u64,
    art: &str,
    zeit: u64,
    alt: Ausschnitt<'_>,
    neu: Ausschnitt<'_>,
) -> Result<JournalEintrag> {
    let mut ops = Vec::new();
    // Union, not chain: ids present on both sides must yield exactly one op.
    let knoten: BTreeSet<&String> = alt
        .cache
        .nodes
        .keys()
        .chain(neu.cache.nodes.keys())
        .collect();
    for id in knoten {
        let alt_node = alt.cache.nodes.get(id);
        let neu_node = neu.cache.nodes.get(id);
        if alt_node == neu_node {
            continue;
        }
        ops.push(Op {
            kind: "neuron".into(),
            ziel: id.clone(),
            name: None,
            alt: wert(alt_node)?,
            neu: wert(neu_node)?,
        });
    }
    for (quelle, ziel) in kanten_paare(alt.cache, neu.cache) {
        let alt_w = alt.cache.edges.get(&quelle).and_then(|e| e.get(&ziel));
        let neu_w = neu.cache.edges.get(&quelle).and_then(|e| e.get(&ziel));
        if alt_w == neu_w {
            continue;
        }
        ops.push(Op {
            kind: "kante".into(),
            ziel: quelle,
            name: Some(ziel),
            alt: alt_w.map(|w| serde_json::json!(w)),
            neu: neu_w.map(|w| serde_json::json!(w)),
        });
    }
    for schluessel in alt
        .meta
        .extra
        .keys()
        .chain(neu.meta.extra.keys())
        .collect::<BTreeSet<_>>()
    {
        let alt_wert = alt.meta.extra.get(schluessel);
        let neu_wert = neu.meta.extra.get(schluessel);
        if alt_wert == neu_wert {
            continue;
        }
        if let (Some(alt_map), Some(neu_map)) = (
            alt_wert.and_then(Value::as_object),
            neu_wert.and_then(Value::as_object),
        ) {
            for name in alt_map.keys().chain(neu_map.keys()) {
                if alt_map.get(name) == neu_map.get(name) {
                    continue;
                }
                ops.push(Op {
                    kind: "extra_feld".into(),
                    ziel: schluessel.clone(),
                    name: Some(name.clone()),
                    alt: alt_map.get(name).cloned(),
                    neu: neu_map.get(name).cloned(),
                });
            }
        } else {
            ops.push(Op {
                kind: "extra".into(),
                ziel: schluessel.clone(),
                name: None,
                alt: alt_wert.cloned(),
                neu: neu_wert.cloned(),
            });
        }
    }
    Ok(JournalEintrag {
        seq,
        interaktionsnummer,
        art: art.into(),
        zeit,
        ops,
    })
}

/// Apply one entry's ops in reverse, writing the stored `alt` values back
/// verbatim. Returns the node ids whose graph records must be deleted.
pub(crate) fn zurueckrollen(
    cache: &mut GraphSlice,
    meta: &mut State,
    eintrag: &JournalEintrag,
) -> Result<Vec<String>> {
    let mut entfernte = Vec::new();
    for op in eintrag.ops.iter().rev() {
        match op.kind.as_str() {
            "neuron" => match &op.alt {
                Some(alt) => {
                    let node: Node = serde_json::from_value(alt.clone())?;
                    cache.nodes.insert(op.ziel.clone(), node);
                }
                None => {
                    // The created node's record must go even when the node is
                    // not part of the currently resident slice.
                    cache.nodes.remove(&op.ziel);
                    cache.edges.remove(&op.ziel);
                    entfernte.push(op.ziel.clone());
                }
            },
            "kante" => {
                let Some(ziel) = &op.name else {
                    bail!("Kanten-Op ohne Ziel");
                };
                match &op.alt {
                    Some(alt) => {
                        let Some(gewicht) = alt.as_f64() else {
                            bail!("Ungültiges Kanten-Gewicht im Journal");
                        };
                        cache
                            .edges
                            .entry(op.ziel.clone())
                            .or_default()
                            .insert(ziel.clone(), gewicht);
                    }
                    None => {
                        if let Some(ziele) = cache.edges.get_mut(&op.ziel) {
                            ziele.remove(ziel);
                        }
                    }
                }
            }
            "extra_feld" => {
                let Some(name) = &op.name else {
                    bail!("Extra-Feld-Op ohne Namen");
                };
                match &op.alt {
                    Some(alt) => {
                        let feld = meta
                            .extra
                            .entry(op.ziel.clone())
                            .or_insert_with(|| serde_json::json!({}));
                        let Some(felder) = feld.as_object_mut() else {
                            bail!("Ungültige Journal-Op");
                        };
                        felder.insert(name.clone(), alt.clone());
                    }
                    None => {
                        if let Some(felder) =
                            meta.extra.get_mut(&op.ziel).and_then(Value::as_object_mut)
                        {
                            felder.remove(name);
                        }
                    }
                }
            }
            "extra" => match &op.alt {
                Some(alt) => {
                    meta.extra.insert(op.ziel.clone(), alt.clone());
                }
                None => {
                    meta.extra.remove(&op.ziel);
                }
            },
            other => bail!("Unbekannte Journal-Op: {other}"),
        }
    }
    Ok(entfernte)
}

/// Index where the /undo round suffix starts: at the most recent consolidate
/// boundary (inclusive), or at the journal start when no boundary exists.
pub(crate) fn runden_index(journal: &[JournalEintrag]) -> usize {
    journal
        .iter()
        .rposition(|e| e.art == ART_CONSOLIDATE)
        .unwrap_or(0)
}

fn kanten_paare(alt: &GraphSlice, neu: &GraphSlice) -> Vec<(String, String)> {
    let mut paare = BTreeSet::new();
    for seite in [alt, neu] {
        for (quelle, ziele) in &seite.edges {
            for ziel in ziele.keys() {
                paare.insert((quelle.clone(), ziel.clone()));
            }
        }
    }
    paare.into_iter().collect()
}

fn wert(node: Option<&Node>) -> Result<Option<Value>> {
    node.map(|n| serde_json::to_value(n).map_err(|e| anyhow!("Journal-Op: {e}")))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn node(id: &str, staerke: f64, nutzt: u64) -> Node {
        Node {
            id: id.into(),
            typ: "LEX".into(),
            label: id.trim_start_matches("lex:").into(),
            staerke,
            alter: "jung".into(),
            nutzt,
            geboren: 1,
            ring: None,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn eintrag_und_zurueckrollen_sind_exakt_inklusive_entfernter_extra_felder() {
        let mut alt_cache = GraphSlice::default();
        alt_cache
            .nodes
            .insert("lex:a".into(), node("lex:a", 0.2, 0));
        alt_cache
            .nodes
            .insert("lex:c".into(), node("lex:c", 0.2, 0));
        alt_cache
            .edges
            .entry("lex:a".into())
            .or_default()
            .insert("lex:b".into(), 0.5);
        alt_cache.edges.entry("lex:c".into()).or_default();
        let mut alt_meta = State::empty();
        alt_meta.extra.insert(
            "structure_observations".into(),
            serde_json::json!({
                "[\"a\",\"b\"]": {"marks":{},"end":1,"continue":0},
                "[\"c\",\"d\"]": {"marks":{},"end":2,"continue":0}
            }),
        );
        // Learned successor: a and c modified, d added, a->b reweighted,
        // c->a and d->a added, one observation row trimmed, another added.
        let mut neu_cache = alt_cache.clone();
        neu_cache
            .nodes
            .insert("lex:a".into(), node("lex:a", 0.25, 1));
        neu_cache
            .nodes
            .insert("lex:c".into(), node("lex:c", 0.25, 1));
        neu_cache
            .nodes
            .insert("lex:d".into(), node("lex:d", 0.2, 0));
        neu_cache
            .edges
            .entry("lex:a".into())
            .or_default()
            .insert("lex:b".into(), 0.6);
        neu_cache
            .edges
            .entry("lex:c".into())
            .or_default()
            .insert("lex:a".into(), 0.2);
        neu_cache
            .edges
            .entry("lex:d".into())
            .or_default()
            .insert("lex:a".into(), 0.2);
        let mut neu_meta = State::empty();
        neu_meta.extra.insert(
            "structure_observations".into(),
            serde_json::json!({
                "[\"a\",\"b\"]": {"marks":{},"end":1,"continue":0},
                "[\"e\",\"f\"]": {"marks":{},"end":1,"continue":0}
            }),
        );

        let eintrag = eintrag(
            7,
            3,
            ART_LEARN,
            0,
            Ausschnitt {
                cache: &alt_cache,
                meta: &alt_meta,
            },
            Ausschnitt {
                cache: &neu_cache,
                meta: &neu_meta,
            },
        )
        .unwrap();
        assert_eq!(eintrag.seq, 7);
        assert_eq!(eintrag.interaktionsnummer, 3);
        assert_eq!(
            eintrag.ops.iter().filter(|op| op.kind == "neuron").count(),
            3
        );
        assert_eq!(
            eintrag.ops.iter().filter(|op| op.kind == "kante").count(),
            3
        );
        assert_eq!(
            eintrag
                .ops
                .iter()
                .filter(|op| op.kind == "extra_feld")
                .count(),
            2
        );

        let mut cache = neu_cache;
        let mut meta = neu_meta;
        let entfernte = zurueckrollen(&mut cache, &mut meta, &eintrag).unwrap();
        assert_eq!(entfernte, vec!["lex:d".to_string()]);
        assert_eq!(
            serde_json::to_value(&cache).unwrap(),
            serde_json::to_value(&alt_cache).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&meta.extra).unwrap(),
            serde_json::to_value(&alt_meta.extra).unwrap()
        );
    }

    #[test]
    fn alle_vier_eintrag_arten_serialisieren_und_rueckholen() {
        // The journal structure must carry every spec art; "heal" is reserved
        // for the Block-4 dialog commands and stays fully rollbackable now.
        for art in [ART_LEARN, ART_CONSOLIDATE, ART_HEAL, ART_FEEDBACK] {
            let eintrag = JournalEintrag {
                seq: 1,
                interaktionsnummer: 1,
                art: art.into(),
                zeit: 0,
                ops: vec![Op {
                    kind: "extra".into(),
                    ziel: "formulierungen".into(),
                    name: None,
                    alt: None,
                    neu: Some(serde_json::json!({"nachfrage":[]})),
                }],
            };
            let bytes = serde_json::to_vec(&eintrag).unwrap();
            let geladen: JournalEintrag = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(geladen.art, art);
            assert_eq!(geladen.seq, 1);
            let mut cache = GraphSlice::default();
            let mut meta = State::empty();
            zurueckrollen(&mut cache, &mut meta, &geladen).unwrap();
            assert!(!meta.extra.contains_key("formulierungen"));
        }
    }
}
