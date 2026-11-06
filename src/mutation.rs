//! Phase 2.5 — novelty operator / mutation layer.
//!
//! The third building block next to the main lexical graph and the learned
//! structure markers: a PROPOSAL mechanism. With a configured probability
//! it inserts at most one learned or fresh word into an answer at a spot the
//! learned patterns do not predict. The main net itself judges the mutant —
//! consecutive-pair edge weights plus structure-marker consistency against
//! the original, no extra network — and a configured threshold decides
//! candidate vs. discard. A discard stays as damped memory (the Kap.-5
//! principle: failed attempts remain visible, nothing is deleted).
//!
//! Model-collapse protection (binding, cf. arXiv:2310.01798): the layer may
//! only PROPOSE. Festigung of a mutant word happens solely through external
//! signals — an explicit +/- rating or the Kap.-18 repetition signal of the
//! following user move; selection acceptance alone changes nothing
//! structural. In /freeze and locked mode the layer never runs: it lives
//! exclusively in the training move, and frozen inference stays read-only.
//!
//! Mutations-Wegweiser: a small sparse table in state.extra (bounded like
//! every learned run, oldest entries rotate out — the ring discipline) that
//! learns the SUCCESS LANDSCAPE of the attack points: which (prev, next)
//! environments brought externally rewarded mutants, which almost always
//! failed. The numeric position of every attempt stays visible in the
//! memory ledger; the wegweiser aggregates by environment. It never learns
//! the WORD to insert: the concrete word choice stays a dice under the
//! configured temperature, otherwise the learned answer would become a
//! second main net and novelty would die.
//!
//! Boredom guard: once an attack point becomes predictably successful
//! (wegweiser score over `mutation_langeweile_schwelle`, seasoned by at
//! least `mutation_langeweile_ab_versuchen` attempts), the layer raises its
//! own wildness by `mutation_wildheit_schritt` — the jump radius widens and
//! the predictable point is avoided — and relaxes it again when no point is
//! predictable. The system stays at the edge of its ability, where novelty
//! exists; the concrete outcome stays dice-rolled.

use crate::{
    graph,
    language::{self, Mark},
    model::State,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Konfig keys of the layer, restored from the fixture state by
/// `Engine::new` — every switch and threshold is data, never code.
pub(crate) const KONFIG: [&str; 7] = [
    "mutation_aktivier",
    "mutation_langeweile_ab_versuchen",
    "mutation_langeweile_schwelle",
    "mutation_rate",
    "mutation_schwellwert",
    "mutation_temperatur",
    "mutation_wildheit_schritt",
];

/// One accepted mutation proposal for the caller: the inserted word and the
/// mutated answer text.
pub(crate) struct Vorschlag {
    pub wort: String,
    pub text: String,
}

/// One insertion slot of the answer with its wegweiser landscape values.
struct Slot {
    stelle: usize,
    prev: Option<String>,
    next: Option<String>,
    umgebung: String,
    score: f64,
    reif: bool,
}

/// Deterministic dice of the layer: xorshift64* over a SHA-256 seed derived
/// from the state, so the same state proposes the same mutation. The rng is
/// mechanics only; every word it selects comes from learned data.
struct Wuerfel(u64);

impl Wuerfel {
    fn neu(samen: &[u8; 32]) -> Self {
        let mut anfang = [0u8; 8];
        anfang.copy_from_slice(&samen[..8]);
        Self(u64::from_le_bytes(anfang) | 1)
    }
    fn zieh(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn f64(&mut self) -> f64 {
        (self.zieh() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn seed(state: &State, kontext: &str, output: &str) -> [u8; 32] {
    let samen = format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        state.interaktionen,
        kontext,
        output,
        state.neuronen.len()
    );
    let mut verdauf = [0u8; 32];
    verdauf.copy_from_slice(&Sha256::digest(samen.as_bytes()));
    verdauf
}

/// Environment key of an attack point: the (prev, next) word pair around
/// the insertion slot, None at the answer's edges.
fn umgebung_key(prev: Option<&str>, next: Option<&str>) -> String {
    serde_json::to_string(&(prev, next)).expect("serialisierbare Umgebung")
}

fn umgebung_aus(kandidat: &Value) -> String {
    umgebung_key(kandidat["prev"].as_str(), kandidat["next"].as_str())
}

fn wildheit_lesen(state: &State) -> f64 {
    state
        .extra
        .get("mutation_wildheit")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

/// The attack point's success score: externally rewarded attempts over all
/// attempts. An unexplored point counts neutral — that is where novelty
/// lives, between the extremes.
fn wegweiser_score(state: &State, umgebung: &str) -> f64 {
    state
        .extra
        .get("mutations_wegweiser")
        .and_then(|m| m.get(umgebung))
        .map(|e| {
            let versuche = e["versuche"].as_u64().unwrap_or(0);
            if versuche == 0 {
                0.5
            } else {
                e["erfolge"].as_u64().unwrap_or(0) as f64 / versuche as f64
            }
        })
        .unwrap_or(0.5)
}

/// Whether the point is seasoned enough that its score counts as evidence
/// (a single lucky reward is not a predictable pattern yet).
fn wegweiser_reif(state: &State, umgebung: &str) -> bool {
    state
        .extra
        .get("mutations_wegweiser")
        .and_then(|m| m.get(umgebung))
        .and_then(|e| e["versuche"].as_u64())
        .is_some_and(|versuche| versuche as f64 >= state.number("mutation_langeweile_ab_versuchen"))
}

/// Count one attempt (or reward) at an attack point in the wegweiser; the
/// bounded table rotates its oldest entries out.
fn wegweiser_zaehlen(state: &mut State, umgebung: &str, feld: &str) {
    let tabelle = state
        .extra
        .entry("mutations_wegweiser".into())
        .or_insert_with(|| json!({}));
    let Some(tabelle) = tabelle.as_object_mut() else {
        return;
    };
    let eintrag = tabelle
        .entry(umgebung.to_owned())
        .or_insert_with(|| json!({"versuche": 0, "erfolge": 0}));
    if let Some(felder) = eintrag.as_object_mut() {
        let zahl = felder.get(feld).and_then(Value::as_u64).unwrap_or(0);
        felder.insert(feld.into(), json!(zahl.saturating_add(1)));
    }
    while tabelle.len() > 256 {
        if let Some(alt) = tabelle.keys().next().cloned() {
            tabelle.remove(&alt);
        } else {
            break;
        }
    }
}

/// Count one event in the layer's honest balance.
fn bilanz_feld(state: &mut State, feld: &str) {
    let bilanz = state.extra.entry("mutation_bilanz".into()).or_insert_with(
        || json!({"versuche":0,"verworfen":0,"festigungen":0,"negativ":0,"verfallen":0}),
    );
    if let Some(felder) = bilanz.as_object_mut() {
        let zahl = felder.get(feld).and_then(Value::as_u64).unwrap_or(0);
        felder.insert(feld.into(), json!(zahl.saturating_add(1)));
    }
}

/// Keep one mutant's fate in the damped memory ledger (Kap.-5 principle:
/// failed attempts remain, nothing is deleted).
fn memory_push(state: &mut State, eintrag: Value) {
    let memory = state
        .extra
        .entry("mutations_memory".into())
        .or_insert_with(|| json!([]));
    if let Some(liste) = memory.as_array_mut() {
        liste.push(eintrag);
        while liste.len() > 256 {
            liste.remove(0);
        }
    }
}

/// Whether the learned patterns place `wort` at this attack point: a
/// recorded continuation behind `prev`, or an edge seen more than once on
/// either side. Only such words "belong" there — the mutation proposes
/// exactly the words the data does NOT expect at this position.
fn gehoert_dort(
    state: &State,
    p2: Option<&str>,
    prev: Option<&str>,
    wort: &str,
    next: Option<&str>,
) -> bool {
    if let Some(prev) = prev {
        if let Some(p2) = p2 {
            let fortsetzung = state
                .extra
                .get("sequence_contexts")
                .and_then(|m| m.get(format!("{p2}\u{1f}{prev}")))
                .and_then(|m| m.get(wort))
                .and_then(Value::as_u64)
                .is_some_and(|zahl| zahl > 0);
            if fortsetzung {
                return true;
            }
        }
        if let (Some(a), Some(b)) = (graph::word(state, prev), graph::word(state, wort)) {
            if graph::edge(state, &a, &b).is_some_and(|w| w > state.number("paar_basis")) {
                return true;
            }
        }
    }
    if let Some(next) = next {
        if let (Some(a), Some(b)) = (graph::word(state, wort), graph::word(state, next)) {
            if graph::edge(state, &a, &b).is_some_and(|w| w > state.number("paar_basis")) {
                return true;
            }
        }
    }
    false
}

/// Candidate words for one attack point: learned resident words and fresh
/// input tokens (words the import budget never grew into neurons) that the
/// learned patterns do NOT place there, ranked by node strength — the
/// temperature decides how far down the ranking the dice may reach.
fn kandidaten(
    state: &State,
    slot: &Slot,
    antwort: &[String],
    eingabe: &[String],
) -> Vec<(String, f64)> {
    let p2 = slot
        .stelle
        .checked_sub(2)
        .and_then(|i| antwort.get(i))
        .map(String::as_str);
    let mut pool: Vec<(String, f64)> = Vec::new();
    for (id, n) in &state.neuronen {
        // The code-corpus namespace is no dialog word; it never mutates.
        if n.typ != "LEX" || id.starts_with("lex:code:") {
            continue;
        }
        let Some(token) = graph::tokens(&n.label).into_iter().next() else {
            continue;
        };
        if antwort.contains(&token) {
            continue;
        }
        if gehoert_dort(
            state,
            p2,
            slot.prev.as_deref(),
            &token,
            slot.next.as_deref(),
        ) {
            continue;
        }
        pool.push((token, n.staerke));
    }
    for token in eingabe {
        if token.contains(':')
            || antwort.contains(token)
            || pool.iter().any(|(t, _)| t == token)
            || graph::word(state, token).is_some()
        {
            continue;
        }
        if gehoert_dort(state, p2, slot.prev.as_deref(), token, slot.next.as_deref()) {
            continue;
        }
        // Fresh words carry no strength yet; only a high temperature lets
        // the dice reach them.
        pool.push((token.clone(), 0.0));
    }
    pool.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    pool
}

/// The main net's verdict over one token sequence (selection without an
/// extra network): the average confirmed edge weight of consecutive pairs —
/// the same accumulation a walked path carries per step — normalized to
/// [0, 1], combined with the structure-marker consistency of the sequence's
/// marks against the learned observation rows.
fn satz_urteil(state: &State, worte: &[String], marks: &[Mark]) -> f64 {
    let paar = if worte.len() < 2 {
        0.5
    } else {
        let summe: f64 = worte
            .windows(2)
            .map(
                |paar| match (graph::word(state, &paar[0]), graph::word(state, &paar[1])) {
                    (Some(a), Some(b)) if a != b => graph::edge(state, &a, &b).unwrap_or(0.0),
                    _ => 0.0,
                },
            )
            .sum();
        (summe / (worte.len() - 1) as f64 + 1.0) / 2.0
    };
    let marker = if marks.is_empty() {
        0.5
    } else {
        let treffer = marks
            .iter()
            .filter(|m| {
                m.after
                    .checked_sub(1)
                    .and_then(|i| worte.get(i))
                    .is_some_and(|wort| {
                        let prev = m
                            .after
                            .checked_sub(2)
                            .and_then(|i| worte.get(i))
                            .map(String::as_str);
                        language::row(state, prev, wort)
                            .and_then(|r| r["marks"][&m.symbol].as_u64())
                            .is_some_and(|zahl| zahl > 0)
                    })
            })
            .count();
        treffer as f64 / marks.len() as f64
    };
    (paar + marker) / 2.0
}

/// Whether any seasoned attack point is predictably successful (Kap.-2.5
/// boredom guard): score over the configured threshold with enough attempts
/// behind it.
fn langeweile_vorhersehbar(state: &State, slots: &[Slot]) -> bool {
    let schwelle = state.number("mutation_langeweile_schwelle");
    slots.iter().any(|s| s.reif && s.score >= schwelle)
}

/// One move's mutation proposal (at most one per answer): rate gate, attack
/// point under the wegweiser landscape and the wildness jump radius, word
/// under the temperature, then the main net's selection. Rejected mutants
/// stay as damped memory; an accepted one becomes the pending candidate
/// awaiting an external signal — nothing structural changes here.
pub(crate) fn schritt(
    state: &mut State,
    kontext: &str,
    eingabe: &[String],
    output: &str,
) -> Option<Vorschlag> {
    if !state
        .konfig
        .get("mutation_aktivier")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let parsed = language::parse(output);
    if parsed.words.is_empty() {
        return None;
    }
    let mut wuerfel = Wuerfel::neu(&seed(state, kontext, output));
    if wuerfel.f64() >= state.number("mutation_rate") {
        return None;
    }
    let mut slots: Vec<Slot> = (0..=parsed.words.len())
        .map(|stelle| {
            let prev = stelle
                .checked_sub(1)
                .and_then(|i| parsed.words.get(i))
                .cloned();
            let next = parsed.words.get(stelle).cloned();
            let umgebung = umgebung_key(prev.as_deref(), next.as_deref());
            let score = wegweiser_score(state, &umgebung);
            let reif = wegweiser_reif(state, &umgebung);
            Slot {
                stelle,
                prev,
                next,
                umgebung,
                score,
                reif,
            }
        })
        .collect();
    // Boredom guard: a predictably successful point raises the wildness —
    // the jump radius widens and the predictable point is avoided this move.
    let gelangweilt = langeweile_vorhersehbar(state, &slots);
    let wildheit = if gelangweilt {
        (wildheit_lesen(state) + state.number("mutation_wildheit_schritt")).min(1.0)
    } else {
        (wildheit_lesen(state) - state.number("mutation_wildheit_schritt")).max(0.0)
    };
    slots.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.umgebung.cmp(&b.umgebung))
    });
    let ende = (1 + (wildheit * (slots.len() - 1) as f64) as usize).min(slots.len() - 1);
    let anfang = usize::from(gelangweilt && ende >= 1);
    // High temperature (or wildness) lets weaker words into the dice's reach.
    let temperatur = (state.number("mutation_temperatur") + wildheit).clamp(0.0, 1.0);
    let mut gewaehlt = None;
    for slot in slots.iter().take(ende + 1).skip(anfang) {
        let pool = kandidaten(state, slot, &parsed.words, eingabe);
        if pool.is_empty() {
            continue;
        }
        let radius = (1 + (temperatur * (pool.len() - 1) as f64) as usize).min(pool.len());
        let rang = (wuerfel.f64() * radius as f64) as usize;
        let (wort, _) = pool[rang.min(pool.len() - 1)].clone();
        gewaehlt = Some((slot, wort));
        break;
    }
    let Some((slot, wort)) = gewaehlt else {
        // No eligible word anywhere: honestly no proposal, no bookkeeping.
        return None;
    };
    // The attempt counts at the attack point either way — failures dampen
    // the score, only external rewards raise it (no self-rewarding).
    wegweiser_zaehlen(state, &slot.umgebung, "versuche");
    bilanz_feld(state, "versuche");
    state
        .extra
        .insert("mutation_wildheit".into(), json!(wildheit));
    let mut worte = parsed.words.clone();
    worte.insert(slot.stelle, wort.clone());
    // Marks stay attached to their words: indices past the slot shift once.
    let marks: Vec<Mark> = parsed
        .marks
        .iter()
        .map(|m| Mark {
            after: if m.after > slot.stelle {
                m.after + 1
            } else {
                m.after
            },
            symbol: m.symbol.clone(),
        })
        .collect();
    let text = language::render(&worte, &marks);
    // Selection without an extra net: the main net's verdict over the mutant
    // vs. the original, threshold from konfig.
    let original = satz_urteil(state, &parsed.words, &parsed.marks);
    let mutante = satz_urteil(state, &worte, &marks);
    if mutante < state.number("mutation_schwellwert") * original {
        bilanz_feld(state, "verworfen");
        memory_push(
            state,
            json!({
                "schicksal": "verworfen", "wort": wort, "stelle": slot.stelle,
                "prev": slot.prev, "next": slot.next,
                "urteil": mutante, "original": original, "zeit": state.interaktionen,
            }),
        );
        return None;
    }
    state.extra.insert(
        "mutation_kandidat".into(),
        json!({
            "wort": wort, "stelle": slot.stelle, "prev": slot.prev, "next": slot.next,
            "kontext": kontext, "runde": state.interaktionen,
        }),
    );
    Some(Vorschlag { wort, text })
}

/// Festigung of an externally rewarded mutant (Kap.-2.5 growth): the word
/// becomes a JUNGE NEURON at the mutation point — a fresh word is grown
/// only now, never by the proposal itself — and the confirmed sequence
/// grows the local helper structure around the point through `hear`, using
/// the same node/alias mechanics as the Kap.-21 ring. The origin stays
/// visible on the node; nothing is deleted. Only `bewerten` calls this,
/// never the proposal path.
fn festigen(state: &mut State, kandidat: &Value) {
    let Some(wort) = kandidat["wort"].as_str() else {
        return;
    };
    let prev = kandidat["prev"].as_str();
    let next = kandidat["next"].as_str();
    if graph::word(state, wort).is_none() {
        let token = wort.to_owned();
        graph::grow(state, std::slice::from_ref(&token), 1);
    }
    let Some(id) = graph::word(state, wort) else {
        return;
    };
    let mut folge: Vec<String> = Vec::new();
    for nachbar in [prev, Some(wort), next].into_iter().flatten() {
        if folge.last().is_none_or(|w| w != nachbar) {
            folge.push(nachbar.to_owned());
        }
    }
    graph::hear(state, &folge);
    if let Some(knoten) = state.neuronen.get_mut(&id) {
        // The confirmation proves one usage: consolidation strengthens the
        // young neuron instead of decaying it.
        knoten.nutzt = knoten.nutzt.saturating_add(1);
        let eintrag = knoten
            .extra
            .entry("mutation_festigung".into())
            .or_insert_with(|| json!([]));
        if let Some(liste) = eintrag.as_array_mut() {
            liste.push(json!({
                "prev": prev, "next": next,
                "runde": kandidat["runde"].as_u64().unwrap_or(0),
            }));
            while liste.len() > 16 {
                liste.remove(0);
            }
        }
    }
    wegweiser_zaehlen(state, &umgebung_aus(kandidat), "erfolge");
    bilanz_feld(state, "festigungen");
    memory_push(
        state,
        json!({
            "schicksal": "festigt", "wort": wort,
            "stelle": kandidat["stelle"].as_u64().unwrap_or(0),
            "prev": prev, "next": next, "zeit": state.interaktionen,
        }),
    );
}

/// A negatively rated mutant damps its attack point: the failed expectation
/// is further evidence against the point, and the failure stays in the
/// memory ledger (Kap.-5).
fn daempfen(state: &mut State, kandidat: &Value) {
    wegweiser_zaehlen(state, &umgebung_aus(kandidat), "versuche");
    bilanz_feld(state, "negativ");
    memory_push(
        state,
        json!({
            "schicksal": "negativ", "wort": kandidat["wort"].clone(),
            "stelle": kandidat["stelle"].as_u64().unwrap_or(0),
            "prev": kandidat["prev"].clone(), "next": kandidat["next"].clone(),
            "zeit": state.interaktionen,
        }),
    );
}

/// The fallback move's answer never becomes the rateable target, so its
/// pending proposal expires unverwertet — honest bookkeeping, no festigung.
pub(crate) fn verfallen(state: &mut State) {
    if state.extra.remove("mutation_kandidat").is_some() {
        bilanz_feld(state, "verfallen");
    }
}

/// External signals decide the pending proposal (model-collapse protection,
/// binding): an explicit +/- rating or the Kap.-18 repetition signal of this
/// move — and only those. The layer never rewards its own wildness; a
/// proposal without a signal stays a proposal.
pub(crate) fn bewerten(
    state: &mut State,
    signale: &language::Signale,
    explizit: Option<i8>,
    ziel: Option<&Value>,
) {
    let Some(kandidat) = state.extra.get("mutation_kandidat").cloned() else {
        return;
    };
    // Without a rateable target nothing was rated this move: the proposal
    // keeps waiting for its answer's signal.
    let Some(ziel) = ziel else {
        return;
    };
    let positiv = explizit == Some(1);
    let negativ = explizit == Some(-1) || signale.wiederholung;
    if !positiv && !negativ {
        return;
    }
    if kandidat["kontext"] != ziel["context"] {
        // The rated answer is not the proposal's answer: the proposal lost
        // its rateable target and expired.
        verfallen(state);
        return;
    }
    state.extra.remove("mutation_kandidat");
    if positiv {
        festigen(state, &kandidat);
    } else {
        daempfen(state, &kandidat);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Node;
    use std::collections::BTreeMap;

    fn knoten(id: &str, staerke: f64, nutzt: u64) -> Node {
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

    fn paar(prev: Option<&str>, wort: &str) -> String {
        serde_json::to_string(&(prev, wort)).unwrap()
    }

    #[test]
    fn satz_urteil_folgt_kanten_und_strukturmarkern() {
        let mut s = State::empty();
        for id in ["lex:a", "lex:b", "lex:c"] {
            s.neuronen.insert(id.into(), knoten(id, 0.5, 1));
        }
        graph::set_edge(&mut s, "lex:a", "lex:b", 0.4);
        graph::set_edge(&mut s, "lex:b", "lex:c", 0.4);
        let beobachtungen = serde_json::json!({
            paar(Some("a"), "b"): {"marks":{".":1},"end":0,"continue":1},
            paar(Some("b"), "c"): {"marks":{".":1},"end":1,"continue":0},
        });
        s.extra
            .insert("structure_observations".into(), beobachtungen);
        let worte = |t: &[&str]| t.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
        let mark = |after: usize| Mark {
            after,
            symbol: ".".into(),
        };
        // The confirmed sequence scores above the same sequence with an
        // unconnected insertion between the confirmed pairs...
        let original = satz_urteil(&s, &worte(&["a", "b", "c"]), &[mark(3)]);
        let mit_fremdwort = satz_urteil(&s, &worte(&["a", "x", "b", "c"]), &[mark(4)]);
        assert!(original > mit_fremdwort);
        // ...and an insertion that shifts a marked word onto an unobserved
        // pair loses the marker support as well.
        let verschoben = satz_urteil(&s, &worte(&["a", "b", "x", "c"]), &[mark(4)]);
        assert!(mit_fremdwort > verschoben);
        assert!(original > verschoben);
    }

    #[test]
    fn umgebung_schluessel_und_gehoert_dort_folgen_den_mustern() {
        assert_eq!(umgebung_key(Some("a"), Some("b")), "[\"a\",\"b\"]");
        assert_eq!(umgebung_key(None, Some("a")), "[null,\"a\"]");
        assert_eq!(umgebung_key(Some("b"), None), "[\"b\",null]");
        let mut s = State::empty();
        s.neuronen.insert("lex:a".into(), knoten("lex:a", 0.5, 1));
        s.neuronen.insert("lex:b".into(), knoten("lex:b", 0.5, 1));
        s.neuronen.insert("lex:c".into(), knoten("lex:c", 0.5, 1));
        // An edge at the base weight means the pair was seen once: the word
        // does not belong there as a pattern yet.
        let basis = s.number("paar_basis");
        graph::set_edge(&mut s, "lex:a", "lex:b", basis);
        assert!(!gehoert_dort(&s, None, Some("a"), "b", None));
        // Seen more than once, the word belongs behind its predecessor.
        graph::set_edge(&mut s, "lex:a", "lex:b", 0.3);
        assert!(gehoert_dort(&s, None, Some("a"), "b", None));
        // A recorded continuation places the word even without a strong edge.
        s.extra.insert(
            "sequence_contexts".into(),
            serde_json::json!({"a\u{1f}b": {"c": 1}}),
        );
        assert!(gehoert_dort(&s, Some("a"), Some("b"), "c", None));
    }

    #[test]
    fn wildheit_und_wegweiser_zaehlen_bleiben_geordnet() {
        let mut s = State::empty();
        assert!((wildheit_lesen(&s) - 0.0).abs() < 1e-9);
        assert!((wegweiser_score(&s, "[\"a\",\"b\"]") - 0.5).abs() < 1e-9);
        assert!(!wegweiser_reif(&s, "[\"a\",\"b\"]"));
        wegweiser_zaehlen(&mut s, "[\"a\",\"b\"]", "versuche");
        for _ in 0..2 {
            wegweiser_zaehlen(&mut s, "[\"a\",\"b\"]", "versuche");
        }
        wegweiser_zaehlen(&mut s, "[\"a\",\"b\"]", "erfolge");
        // Three attempts season the point; one reward scores a third.
        assert!(wegweiser_reif(&s, "[\"a\",\"b\"]"));
        assert!((wegweiser_score(&s, "[\"a\",\"b\"]") - 1.0 / 3.0).abs() < 1e-9);
        // An unexplored point stays neutral.
        assert!((wegweiser_score(&s, "[null,\"a\"]") - 0.5).abs() < 1e-9);
        assert!(!langeweile_vorhersehbar(
            &s,
            &[Slot {
                stelle: 1,
                prev: Some("a".into()),
                next: Some("b".into()),
                umgebung: "[\"a\",\"b\"]".into(),
                score: wegweiser_score(&s, "[\"a\",\"b\"]"),
                reif: wegweiser_reif(&s, "[\"a\",\"b\"]"),
            }]
        ));
        s.validate().unwrap();
    }
}
