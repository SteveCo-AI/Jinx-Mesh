# Training Jinx-Mesh — current state (2.6.x research wave)

Model version this document was validated against: **2.6.1**
Commit: `2f3988e` (runner) / docs wave `b3c66e7` · Date: 2026-11-06
Validity: results below were measured on the **10k corpus family
(Jinx/Lumen/SteveCo tutor lines)** and on **E1 single-experience runs** on a
fresh vault. They describe the mechanics shipped in 2.6.1. Any newer commit
must re-check this document (see "Doc test" below).

## Scope and honesty rules

- This document separates **verified procedure** from **experimental** and
  **hypothesis**. Anything labeled Experimental or Hypothesis must not be
  treated as an architecture decision.
- Training behavior depends on corpus, message order, graph state at the time
  a message arrives, and feedback. Results without those conditions recorded
  are not reproducible and must not be cited as evidence.
- Every training record must name: model version, commit, date, corpus, and
  validity scope.

## How to train (verified procedure, 2.6.1)

1. Fresh vault: `saeugling init <path>.db` (or `template.db` copies via the
   experiment runner). Empty vault = no vocabulary; it learns only from fed
   text.
2. Corpus hygiene: filter meta lines out of tutor corpora; fix grammar BEFORE
   training — the model inherits typos 1:1. (Measured: corrupted lines were
   reproduced verbatim in the 10k runs.)
3. Curriculum first phase: repeat adjacent Q/A pairs (25x measured stable),
   THEN loosen phases. Verified: the greeting "hallo jinx!" stayed exact from
   checkpoint 1000 to 10000.
4. Echo loops (own output fed back) create real assignments only sporadically
   — they are a research instrument, not a reliable training method. Do not
   treat "echo produces bindings" as a guarantee.
5. Read-only probe questions (`infer_read_only`) never count as learning
   turns; the runner asserts interaction counts.
6. The +/- DEV feedback prefix rates the PREVIOUS model answer in its context
   (step 0.25, clamp ±2, forbidden in /freeze). Signal quality depends on
   context addressing — rate the actual answer, not a future one.

## How to run the experiment runners (2.6.1, actual commands)

The runners are `cargo` examples and need the developer's vault login, so
they always ask for hidden credentials (rpassword needs a real TTY — smoke
tests run inside `script -qec`):

```
cargo run --release --example experiment_runner -- <experiment-dir>
printf 'Lumen\n<password>\n\n' | script -qec \
  'cargo run --release --example experiment_runner -- <dir>' /dev/null
```

- `<dir>` needs `experiment.json` (fields: `initial_vault`, `probe`,
  `intro`, `distractors`, `grid`, optional `user`); the runner copies the
  template to `<dir>/working.db`, fails on an existing `working.db` (no
  half-run restarts), never touches the active vault.
- `results.json` is rewritten after every snapshot (crash-safe partial data).
- Probe questions in `experiment.json` must NOT appear in the corpus — verify
  with a word scan (measured: 'geburtstag/november/sechster' absent from the
  10k corpus = valid E1 probe).

## Documented findings (status-controlled)

| Finding | Status | Scope |
|---|---|---|
| Curriculum repetition builds stable exact greetings (B, CP1000→10000) | **Bestätigt** (within 10k corpus conditions) | v2.6.1 mechanics, 10k corpus family |
| After ~2000 messages almost nothing new forms (B froze mirror attractors from CP2000) | **Bestätigt** (same conditions) | v2.6.1, 10k corpus |
| Echo produces real assignments only sporadically, they get overpainted | **Bestätigt** | v2.6.1, 10k dialog condition |
| Message rate decays ~10→3.5 msg/s from commit overhead growth, NOT think-path growth | **Bestätigt** (checkpoint timings) | v2.6.1, 10k |
| A single experience without reinforcement establishes NO complete assignment (echo fragments only) | **Bestätigt** | v2.6.1, E1 run 1 (empty vault, 81-line distractor corpus, 1 intro message) |
| 5× reinforced intro establishes a real assignment immediately, lost again between N=0 and N=50 distractions | **Bestätigt** | v2.6.1, E1 run 2 (same setup, 5 intro messages) |
| Lost word-traces reappear rotating in the active window (rotation, not deletion) | **Beobachtet** (mechanism open) | v2.6.1, E1 runs 1+2 |
| Mirror attractors from ~200 distraction messages | **Beobachtet** | v2.6.1, E1 |
| Gezieltes ±Feedback on probe questions stabilizes assignments | **Hypothese** (next E4 measurement) | — |
| Saturation of competing edges is THE retention lever | **Hypothese** (E2 open) | — |
| Anchor words ("super"/"nein") as signal source | **Widerlegt/Überholt** (removed in V01) | pre-2.6.1 |
| implicit_positiv channel | **Widerlegt/Überholt** (removed in V01) | pre-2.6.1 |
| trim() Map-order bug (lexicographically smallest key evicted instead of oldest) | **Bestätigt** (code-level, language.rs:79) | v2.6.1, no E1 measurement yet |

## Experimental methods under investigation (NOT recommendations)

- The "pipette" sequence (pair imprinting → echo curation → targeted ±) is
  the current test series, not the final method. Its parts are under
  RW-1 measurement (E2 saturation, E4 rating).
- The `reinforce` runner extension (planned, ZCode build) adds post-intro
  rating messages for E4.

## Doc test (project routine)

If learning mechanics, feedback logic, or the training protocol change, check
this document in the same change: version/commit validity header, verified
procedures, findings table. A stale findings row is worse than a missing one.
