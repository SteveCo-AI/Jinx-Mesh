# WARTUNG — module structure and maintenance backlog

This document records the 2.6.1 structural refactoring (pure moves, zero
behavior change) and the rules plus next candidates for future maintenance
waves. It is referenced from `ARCHITECTURE.md`.

## What was split

Three oversized single files became per-domain module directories. No logic,
message text, fixture key, or public item changed; the version stays 2.6.1.

| Before | After |
|---|---|
| `src/app.rs` (2,067 lines) | `src/app/` — 7 files (2,168 lines incl. module docs) |
| `src/store.rs` (1,250 lines) | `src/store/` — 5 files (1,325 lines) |
| `src/tests.rs` (3,047 lines) | `src/tests/` — 10 files (3,137 lines), still 70 unit tests |

Each split was verified immediately with the same three gates required of a
release: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D
warnings`, and `cargo test --locked` (exactly 111 passed: 86 unit + 25
integration tests, before and after).

## Module map

### `src/app/` — authorization and integration boundary

| File | Lines | Responsibility |
|---|---|---|
| `mod.rs` | 200 | `Session`/`App` state, activation budgets, constructors (`create`/`create_empty`/`open`/`with_policy`), TOTP helper, metadata/changed-graph helpers, test crash hooks; re-exports `Mode`, `SessionPolicy`, `RecallResult` so `lib.rs` paths are unchanged |
| `auth.rs` | 141 | `Mode`, `SessionPolicy`, `tick`/`mode`, `login`, `logout`, `freeze` |
| `message.rs` | 634 | Frozen read-only inference, `message` training move, continuation, `consolidate`, `train_file` import, gate-reject journaling |
| `undo_heal.rs` | 529 | `undo`/`undo_runde` bounded rollback and the `/falsch` + `/echtes_wort` heals |
| `recall_checkpoint.rs` | 241 | `RecallResult` and the paged recall family, idle prefetch step |
| `view.rs` | 89 | `graph_view`, `status`, full archive/graph verification |
| `activation.rs` | 334 | Shared bounded traversal primitive `activate_graph`, merge/checkpoint-budget/source-match helpers (`pub(super)`) |

### `src/store/` — encrypted vault persistence

| File | Lines | Responsibility |
|---|---|---|
| `mod.rs` | 275 | `Import`, `Payload`, `GraphRecord`, `TraversalCheckpoint`, `Store` handle, `aad`/`version`, keyed lookup and root helpers, inline `storage_tests` |
| `access.rs` | 636 | `create`/`open` with process lock, throttle, `unlock` with audit replay, `authenticated`, the single `commit` transaction, sealing helpers |
| `journal.rs` | 79 | Write-ahead op-journal append and crash-recovery sync |
| `archive.rs` | 114 | Paged episode blocks, exact-ID fetch, manifest and full integrity checks |
| `paging.rs` | 221 | Bounded graph record/neighbor pages, checkpoints, streaming full-graph audit |

### `src/tests/` — library test suite by domain

| File | Tests | Domain |
|---|---|---|
| `mod.rs` | — | Shared fixtures (`engine`, `JOURNAL_PASS`, `engine_mit`) and the `quelldateien` source scanner |
| `basics.rs` | 17 | Phase-1 core engine/model/graph/language behavior, crypto roundtrip |
| `journal.rs` | 5 | Op-journal mechanics (Kap. 14) |
| `feedback.rs` | 6 | Explicit feedback and repetition channel (Kap. 18) |
| `undo.rs` | 2 | Cold-source and large-span rollback |
| `fallback.rs` | 12 | Uncertainty fallback (Kap. 19), incl. the scaffold-text source scan |
| `healing.rs` | 12 | Self-healing and dialog heals (Kap. 21/22) |
| `gate.rs` | 4 | Integration gate (Kap. 24) |
| `mutation.rs` | 10 | Novelty operator (Phase 2.5) |
| `release.rs` | 2 | Anti-hardcode release guard (Kap. 20) |

## Deliberate visibility and test-fixture adjustments

These are the only non-move changes; all are internal or test-only:

- `app/activation.rs` helpers `merge_slice`, `activate_graph`,
  `checkpoint_fits`, `source_matches` became `pub(super)` (siblings in
  `app/` use them); `store/archive.rs`
  `verify_archive_manifest` became `pub(super)` (called by `access.rs`
  `unlock`).
- The two Kap.-20 behavior-text scanners
  (`tests/fallback.rs::fallback_geruesttexte_kommen_nicht_aus_dem_code`,
  `tests/release.rs::test_anti_hardcode`) previously read only the top
  level of `src/`. Because the split moved the shipped code into
  subdirectories, both now walk `src/` recursively via
  `tests/mod.rs::quelldateien()`; `test_anti_hardcode` excludes the whole
  `src/tests/` subtree, which replaces the old single-file `tests.rs`
  exception. Guard coverage is preserved, not weakened.

## Rules for further splitting

1. Pure moves only: no renamed public items, no edited messages, no fixture
   changes; `Cargo.toml` stays at the current release version unless the
   release process itself bumps it.
2. The public surface is fixed by `lib.rs`: `App`, `Mode`, `RecallResult`,
   `SessionPolicy`, `Reply`, `Import`. A type may move into a submodule only
   with a re-export that keeps its `crate::`-path identical.
3. `impl` blocks for one type may live in any submodule of the defining
   module; private fields and methods of a parent stay reachable from child
   modules, which is how the split avoided accessor churn. Only genuinely
   cross-module helpers need `pub(super)`.
4. `#[cfg(test)]` test modules for a unit stay inline in that unit's module
   (the release scanner strips their bodies); the library suite lives under
   `src/tests/`. When moving files, re-run both scanner tests — they
   enumerate `src/` recursively and must stay exhaustive.
5. After every file move, run the three gates before continuing:
   `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
   `cargo test --locked` — 111 passed / 0 failed unless a change
   intentionally adds or removes tests.
6. Keep source files under ~600 lines where practical; prefer domain
   boundaries (one behavioral block per file) over mechanical halving.

## Next candidates (maintenance backlog)

Ordered by size and churn risk; all are optional, none blocks a release:

1. `src/app/message.rs` (634) — the dialog move, continuation,
   consolidation, and file import can each become a submodule once any of
   them grows again.
2. `src/store/access.rs` (636) — `create`/`open` (schema and lock) can part
   from `unlock`/`commit` (authentication transaction) when format 4 work
   starts.
3. `src/mutation.rs` (719), `src/healing.rs` (558), `src/language.rs` (521),
   `src/model.rs` (536) — still single files; split by their section
   comments when touched next.
4. The measurement-wave order rule from `ARCHITECTURE.md` still applies:
   structural work must not move code underneath a running experiment wave.
