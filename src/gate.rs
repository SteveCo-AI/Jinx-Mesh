//! Kap. 24 integration gate: the commit boundary of every learning step.
//!
//! Jinx-Mesh has no lab actions yet (`aktion: None` everywhere), so the gate
//! is the common mechanism over learning commits — a dialog move, a file
//! import, or a consolidation round. Before such a move commits, its exact
//! op diff (the journal entry the Block-1 mechanism already builds) is
//! checked against the rule data in `gate_regeln` (konfig, fixture
//! defaults). The gate is mechanics only; every rule VALUE is data, never a
//! code string (Kap. 20).
//!
//! Open/Closed: the registry below is the closed set of column interpreters.
//! Changing a rule — a threshold, a weight band, a mandatory field — is a
//! JSON edit in the vault state with no code change; a future level (e.g.
//! the Kap.-24 action integration) registers its columns the same way. A
//! declared column the registry cannot interpret fails loudly instead of
//! passing silently — an unchecked rule would be fake safety.
//!
//! Reject: the move commits nothing. The caller restores the pre-move state
//! exactly and records the refusal as an op-journal entry of the art
//! `integration_reject` (no ops — nothing changed), confirmed by a
//! bookkeeping commit so it survives a reload and rolls back with /undo
//! like every entry. The honest system notice carries the mechanical rule
//! findings; it is a system message, never a model answer.

use crate::journal::Op;
use crate::model::State;
use serde_json::Value;

/// Konfig key holding the rule data columns.
pub(crate) const KONFIG_KEY: &str = "gate_regeln";

/// One rule column: its name in the konfig data, the commit-time check over
/// the pre-move state and the proposed ops, and the shape/range check
/// `State::validate` runs on the configured value.
struct Spalte {
    name: &'static str,
    pruefen: fn(&State, &[Op], &Value) -> Vec<String>,
    konfig_pruefen: fn(&Value) -> bool,
}

/// The rule registry: adding a rule means adding one entry here plus its two
/// checkers — the values always stay data in fixtures/start.json.
const REGISTRY: &[Spalte] = &[
    Spalte {
        name: "erwartet_neue_knoten_max",
        pruefen: pruef_neue_knoten,
        konfig_pruefen: konfig_knoten_max,
    },
    Spalte {
        name: "erwartet_entfernt",
        pruefen: pruef_entfernt,
        konfig_pruefen: konfig_bool,
    },
    Spalte {
        name: "erwartet_gewicht_range",
        pruefen: pruef_gewicht_range,
        konfig_pruefen: konfig_range,
    },
    Spalte {
        name: "pflichtfelder_extra",
        pruefen: pruef_pflichtfelder,
        konfig_pruefen: konfig_felder,
    },
];

/// Whether an extra field survives the move: a whole-key op carries the new
/// presence, sub-field ops keep the parent object, an untouched key keeps
/// its pre-move presence.
fn feld_da_nach(vorher: &State, ops: &[Op], feld: &str) -> bool {
    match ops.iter().find(|op| op.ziel == feld) {
        Some(op) if op.kind == "extra" => op.neu.is_some(),
        Some(_) => true,
        None => vorher.extra.contains_key(feld),
    }
}

fn pruef_neue_knoten(_vorher: &State, ops: &[Op], wert: &Value) -> Vec<String> {
    let Some(max) = wert.as_f64() else {
        return Vec::new();
    };
    let max = max.max(0.0) as usize;
    let anzahl = ops
        .iter()
        .filter(|op| op.kind == "neuron" && op.alt.is_none())
        .count();
    (anzahl > max)
        .then(|| format!("{anzahl} neue knoten, erlaubt {max}"))
        .into_iter()
        .collect()
}

fn pruef_entfernt(_vorher: &State, ops: &[Op], wert: &Value) -> Vec<String> {
    if wert.as_bool().unwrap_or(true) {
        return Vec::new();
    }
    let entfernt: Vec<&str> = ops
        .iter()
        .filter(|op| op.kind == "neuron" && op.neu.is_none())
        .map(|op| op.ziel.as_str())
        .collect();
    if entfernt.is_empty() {
        return Vec::new();
    }
    let beispiele = entfernt
        .iter()
        .take(4)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    vec![format!("{} knoten entfernt ({beispiele})", entfernt.len())]
}

fn pruef_gewicht_range(_vorher: &State, ops: &[Op], wert: &Value) -> Vec<String> {
    let (Some(lo), Some(hi)) = (
        wert.get(0).and_then(Value::as_f64),
        wert.get(1).and_then(Value::as_f64),
    ) else {
        return Vec::new();
    };
    ops.iter()
        .filter(|op| op.kind == "kante")
        .filter_map(|op| {
            let gewicht = op.neu.as_ref().and_then(Value::as_f64)?;
            (!(lo..=hi).contains(&gewicht)).then(|| match &op.name {
                Some(ziel) => {
                    format!(
                        "kante {} -> {ziel} mit {gewicht}, erlaubt [{lo}, {hi}]",
                        op.ziel
                    )
                }
                None => format!("kante {} mit {gewicht}, erlaubt [{lo}, {hi}]", op.ziel),
            })
        })
        .collect()
}

fn pruef_pflichtfelder(vorher: &State, ops: &[Op], wert: &Value) -> Vec<String> {
    let Some(felder) = wert.as_array() else {
        return Vec::new();
    };
    felder
        .iter()
        .filter_map(Value::as_str)
        .filter(|feld| !feld_da_nach(vorher, ops, feld))
        .map(|feld| format!("pflichtfeld {feld} fehlt nach dem schritt"))
        .collect()
}

fn konfig_knoten_max(wert: &Value) -> bool {
    wert.as_f64()
        .is_some_and(|v| v.is_finite() && (0.0..=1_000_000.0).contains(&v))
}

fn konfig_bool(wert: &Value) -> bool {
    wert.as_bool().is_some()
}

fn konfig_range(wert: &Value) -> bool {
    let Some(band) = wert.as_array() else {
        return false;
    };
    if band.len() != 2 {
        return false;
    }
    let (Some(lo), Some(hi)) = (
        band.first().and_then(Value::as_f64),
        band.get(1).and_then(Value::as_f64),
    ) else {
        return false;
    };
    lo.is_finite()
        && hi.is_finite()
        && lo <= hi
        && (-1.0..=1.0).contains(&lo)
        && (-1.0..=1.0).contains(&hi)
}

fn konfig_felder(wert: &Value) -> bool {
    let Some(felder) = wert.as_array() else {
        return false;
    };
    felder.len() <= 32
        && felder.iter().all(|feld| {
            feld.as_str()
                .is_some_and(|f| !f.is_empty() && f.len() <= 256)
        })
}

/// Kap.-24 gate check of one proposed learning diff: the pre-move state, the
/// journal entry's ops, and the rule data. Returns the rejection reasons;
/// an empty list accepts the move. A missing or shapeless rule value checks
/// nothing (the conservative default; Engine::new restores the fixture
/// columns, so in practice they are present).
pub(crate) fn verify(vorher: &State, ops: &[Op], regeln: &Value) -> Vec<String> {
    let Some(spalten) = regeln.as_object() else {
        return Vec::new();
    };
    let mut gruende = Vec::new();
    for spalte in REGISTRY {
        if let Some(wert) = spalten.get(spalte.name) {
            for befund in (spalte.pruefen)(vorher, ops, wert) {
                gruende.push(format!("{}: {befund}", spalte.name));
            }
        }
    }
    // A declared column the registry cannot interpret must never pass
    // silently — the move is rejected until the rule data is honest.
    for name in spalten.keys() {
        if !REGISTRY.iter().any(|spalte| spalte.name == name) {
            gruende.push(format!("{name}: unbekannte regelspalte"));
        }
    }
    gruende
}

/// Shape/range check for `State::validate`: every known column must be
/// usable and every declared column must be known. The key itself stays
/// optional so vaults predating the gate load unchanged.
pub(crate) fn konfig_pruefen(regeln: &Value) -> bool {
    let Some(spalten) = regeln.as_object() else {
        return false;
    };
    if spalten.len() > 16 {
        return false;
    }
    for spalte in REGISTRY {
        if let Some(wert) = spalten.get(spalte.name) {
            if !(spalte.konfig_pruefen)(wert) {
                return false;
            }
        }
    }
    spalten
        .keys()
        .all(|name| REGISTRY.iter().any(|spalte| spalte.name == name))
}

/// The state's configured rule data. The engine restores the fixture
/// defaults for vaults predating the mechanism; a state without the key
/// yields the empty rule set and checks nothing (conservative default).
pub(crate) fn regeln(state: &State) -> Value {
    state
        .konfig
        .get(KONFIG_KEY)
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}))
}

/// Honest system notice for a rejected move (Kap. 24): a mechanical system
/// message, never a model answer — the reasons are rule findings, not
/// behavior formulations.
pub(crate) fn hinweis(gruende: &[String]) -> String {
    format!("Integration-Prüfung fehlgeschlagen: {}", gruende.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn op(
        kind: &str,
        ziel: &str,
        name: Option<&str>,
        alt: Option<Value>,
        neu: Option<Value>,
    ) -> Op {
        Op {
            kind: kind.into(),
            ziel: ziel.into(),
            name: name.map(str::to_owned),
            alt,
            neu,
        }
    }

    fn neue_knoten_op() -> Op {
        op(
            "neuron",
            "lex:neu",
            None,
            None,
            Some(json!({"staerke": 0.2})),
        )
    }

    fn entfernte_knoten_op() -> Op {
        op(
            "neuron",
            "lex:alt",
            None,
            Some(json!({"staerke": 0.2})),
            None,
        )
    }

    fn kante_op(gewicht: f64) -> Op {
        op(
            "kante",
            "lex:a",
            Some("lex:b"),
            Some(json!(0.2)),
            Some(json!(gewicht)),
        )
    }

    fn zustand() -> State {
        let mut s = State::empty();
        s.extra.insert("kanal2_letzte".into(), json!({}));
        s
    }

    fn regeln() -> Value {
        State::empty().konfig[KONFIG_KEY].clone()
    }

    #[test]
    fn verify_akzeptiert_regelkonforme_zuege_und_leere_regeln() {
        let vorher = zustand();
        let ops = vec![
            neue_knoten_op(),
            kante_op(0.5),
            op(
                "extra_feld",
                "kanal2_letzte",
                Some("offen"),
                None,
                Some(json!(1)),
            ),
        ];
        assert!(verify(&vorher, &ops, &regeln()).is_empty());
        // The conservative default: no rule data checks nothing.
        assert!(verify(&vorher, &ops, &json!({})).is_empty());
        assert!(verify(&vorher, &ops, &json!(5)).is_empty());
        assert!(verify(&State::empty(), &[], &regeln()).is_empty());
    }

    #[test]
    fn verify_meldet_jede_verletzte_datenspalte() {
        let vorher = zustand();
        // Too many new nodes for the configured cap.
        let gruende = verify(
            &vorher,
            &[neue_knoten_op()],
            &json!({"erwartet_neue_knoten_max": 0}),
        );
        assert_eq!(
            gruende,
            vec!["erwartet_neue_knoten_max: 1 neue knoten, erlaubt 0".to_owned()]
        );
        // Removals the data did not declare as expected...
        let gruende = verify(
            &vorher,
            &[entfernte_knoten_op()],
            &json!({"erwartet_entfernt": false}),
        );
        assert_eq!(gruende.len(), 1);
        assert!(gruende[0].starts_with("erwartet_entfernt: 1 knoten entfernt"));
        // ...and allowed when the data declares them expected.
        assert!(verify(
            &vorher,
            &[entfernte_knoten_op()],
            &json!({"erwartet_entfernt": true})
        )
        .is_empty());
        // A new edge weight outside the configured band.
        let gruende = verify(
            &vorher,
            &[kante_op(-0.5)],
            &json!({"erwartet_gewicht_range": [0.0, 1.0]}),
        );
        assert_eq!(gruende.len(), 1);
        assert!(gruende[0].starts_with("erwartet_gewicht_range: kante lex:a -> lex:b"));
        // A mandatory field the move removes.
        let gruende = verify(
            &vorher,
            &[op("extra", "kanal2_letzte", None, Some(json!({})), None)],
            &json!({"pflichtfelder_extra": ["kanal2_letzte"]}),
        );
        assert_eq!(
            gruende,
            vec![
                "pflichtfelder_extra: pflichtfeld kanal2_letzte fehlt nach dem schritt".to_owned()
            ]
        );
        // ...one that never existed before the move...
        let gruende = verify(
            &State::empty(),
            &[],
            &json!({"pflichtfelder_extra": ["kanal2_letzte"]}),
        );
        assert_eq!(gruende.len(), 1);
        // ...while a sub-field op keeps the parent object alive.
        assert!(verify(
            &vorher,
            &[op(
                "extra_feld",
                "kanal2_letzte",
                Some("offen"),
                Some(json!(1)),
                None
            )],
            &json!({"pflichtfelder_extra": ["kanal2_letzte"]})
        )
        .is_empty());
        // A declared column the registry does not know never passes silently.
        let gruende = verify(&vorher, &[], &json!({"neue_spalte": 1}));
        assert_eq!(
            gruende,
            vec!["neue_spalte: unbekannte regelspalte".to_owned()]
        );
    }

    #[test]
    fn konfig_spalten_werden_nach_struktur_und_wert_geprueft() {
        assert!(konfig_pruefen(&regeln()));
        assert!(!konfig_pruefen(&json!([])));
        assert!(!konfig_pruefen(&json!({"erwartet_neue_knoten_max": -1.0})));
        assert!(!konfig_pruefen(
            &json!({"erwartet_neue_knoten_max": "zwei"})
        ));
        assert!(!konfig_pruefen(&json!({"erwartet_entfernt": "vielleicht"})));
        assert!(!konfig_pruefen(&json!({"erwartet_gewicht_range": [-1.0]})));
        assert!(!konfig_pruefen(
            &json!({"erwartet_gewicht_range": [0.5, -0.5]})
        ));
        assert!(!konfig_pruefen(
            &json!({"erwartet_gewicht_range": [-2.0, 1.0]})
        ));
        assert!(!konfig_pruefen(&json!({"erwartet_gewicht_range": "band"})));
        assert!(!konfig_pruefen(&json!({"pflichtfelder_extra": [3]})));
        assert!(!konfig_pruefen(&json!({"pflichtfelder_extra": [""]})));
        assert!(!konfig_pruefen(&json!({"neue_spalte": 1})));
        // Single columns stay individually optional.
        assert!(konfig_pruefen(&json!({"erwartet_entfernt": true})));
    }
}
