# Live findings log

Model version: **2.6.1** · Date: 2026-11-06
Status values: **Bestätigt** (reproduced by measurement, scope named) ·
**Beobachtet** (occurred, cause open) · **Hypothese** (plausible, unproven) ·
**Widerlegt** (not confirmed by experiment) · **Überholt** (older model
version). Add new rows at the top, keep old ones with their status — a stale
row must stay visible so agents don't re-chase fixed deficits.

| Date | Finding | Status | Validity scope |
|---|---|---|---|
| 2026-11-06 | E1 runs 3+4+5: +/- feedback anchors ONLY the edges literally contained in the last answer (birth→sechster survives 1000 distractions; november falls in all three reinforce orders — on fragment answers, on full intro outputs, and with the probe question as input). Feedback acts on last-answer edges in the last-input context and cannot fix a question→answer chain the answer does not literally contain | **Bestätigt** | v2.6.1, E1 runner with reinforce (commit e05777e), 3 orders × 1000 distractions, identical series |
| 2026-11-06 | Feedback context-addressing for question→answer chains is the shared open blocker of BOTH Python-Säugling (anchor channels) and Rust 2.6.1 (context_feedback) — matches the known parallel finding "rating needs context addressing, not a word scalar" | **Beobachtet** (root cause code-level) | v2.6.1 E1 runs 3-5 + pre-2.6.1 Python parallel |
| 2026-11-06 | trim() evicts the lexicographically smallest context key instead of the oldest (serde_json::Map = BTreeMap w/o preserve_order; src/language.rs trim()) — positiv-confirmed contexts can be evicted at the 256 limit | **Bestätigt** (code-level) | v2.6.1; impact measurement pending (E1 state capture) |
| 2026-11-06 | E1 run 2: 5x reinforced intro → complete assignment immediately, lost between N=0 and N=50 distractions; partial trace ("sechster" or "november" alone) rotates back into the active window later | **Bestätigt** | v2.6.1, empty template vault, 81-line cyclic distractor corpus, E1 runner commit 2f3988e |
| 2026-11-06 | E1 run 1: single (1x) experience establishes NO complete assignment at any time — only echo fragments of the intro | **Bestätigt** | same scope, 1 intro message |
| 2026-11-06 | Lost word traces ROTATE (reappear in the active window at N=500/1000) instead of being deleted | **Beobachtet** | v2.6.1, E1 runs 1+2 |
| 2026-11-06 | Mirror attractor from ~200 distraction messages on; frozen vocabulary growth at total=329 after CP200 | **Beobachtet** | v2.6.1, E1 |
| 2026-11-06 | Curriculum exact-greeting stability CP1000→CP10000; new-formation freeze ~CP2000 | **Bestätigt** | v2.6.1 mechanics, 10k corpus family |
| 2026-11-06 | Echo creates real assignments (e.g. "Wer ist Lumen?"→"lumen ist mein architekt!" @CP3000) but they get overpainted by later echo chains | **Bestätigt** | v2.6.1, 10k dialog condition |
| 2026-11-06 | Throughput decay 10→3.5 msg/s is commit-overhead + echo growth, NOT think-path (activation constant) | **Bestätigt** | v2.6.1, 10k checkpoint timings |
| 2026-11-06 | Targeted ± feedback stabilizes probe assignments | **Hypothese** — E4 planned (reinforce runner build pending) | — |
| 2026-11-06 | Edge saturation is the dominant retention lever | **Hypothese** — E2 open | — |
| 2026-11-06 | Anchor words ("super"/"nein") improve signal | **Widerlegt/Überholt** — mechanic removed by V01 decision | pre-2.6.1 |
| 2026-11-06 | implicit_positiv channel (continuation = positive) helps learning | **Widerlegt/Überholt** — removed by V01 decision | pre-2.6.1 |
