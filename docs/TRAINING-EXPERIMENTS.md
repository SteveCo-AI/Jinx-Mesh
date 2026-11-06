# Training experiments — research protocol (RW-1 and 10k conditions)

Model version: **2.6.1** · Commits: `2f3988e` (runner), `509cdce` (docs) ·
Date: 2026-11-06 · Language of this repo's docs: English

This document separates the **how we measure** (public, reproducible) from
the **what we currently believe** (see TRAINING.md findings table). It links
the internal plan (FORSCHUNG-RW-1.md lives in the local project folder, not
this repo) to the runner capabilities shipped here.

## Completed measurement series (with sources)

### 10k comparison (2026-11-06, three conditions on fresh vaults)
- A Uniform (85 lines global shuffle), B Curriculum (adjacent pairs then
  loosen), D Dialog loop (tutor line + own echo x2). 10,000 messages each,
  checkpoints every 1000, read-only probes.
- Local artifacts: /home/lumen/projekt-jinx-netz/jinx-mesh-10k/
  (training.json per condition); summary in TRAINING-ERKENNTNISSE-10K.md
  (local). Reproducible with examples/training_run.rs &
  examples/dialog_run.rs.
- Validity: this corpus family only.

### RW-1 E1 — survival of a single experience (runner in this repo)
- Design probe = question never seen trained; ONE controlled intro
  experience; fixed cyclic distractor corpus; grid 0/50/200/500/1000;
  snapshot = status + graph_view + read-only probe answer.
- Run 1: 1 intro message → no complete assignment ever (echo fragments only).
- Run 2: 5 intro messages → real assignment after intro, lost between N=0
  and N=50; lost traces reappear rotating in the active window.
- Artifacts (local, not in repo): jinx-mesh/forschung/RW-1/E1-lauf{1,2}/
  (experiment.json, results.json, AUSWERTUNG.md).

## Measurement infrastructure (shipped, v2.6.1)

- `examples/experiment_runner.rs`: state snapshots on a copied vault; probe
  read-only with interaction assert; crash-safe results.json; active-vault
  guard; anti-hardcode (all knobs in experiment.json).
- Planned extension `reinforce` (E4): post-intro rating messages — build
  pending, do not cite results until measured.
- Read-only probe invariant: `interactions` counter unchanged around probe.

## Open experiment slots (per RW-1 plan)

- E2: isolated saturation variation (identical starts, only competition level
  differs) — Hypothese under test.
- E3: minimal episodic assignment layer (A without vs B with).
- E4: rating as isolated factor (plain repeat / once / plus / minus / combo).
- E5: predicting learning success from pre-state features (no data leakage).

Metrics: Erwerbsrate A (immediately correct / introduced assignments),
S(N) survival at N=0/50/200/500/1000, unconditional late success, first-loss
time, reappearance, competing-path drift. A single lucky hit never counts as
durable learning. Every experiment must separate LERN-blockade (assignment
not in graph) from ABRUF-blockade (in graph but not selected).
