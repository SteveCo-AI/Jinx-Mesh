# Validation 2026-11-06

## Current structural-marker and feedback update

Format and Clippy checks passed. The 46 regression checks cover 25 library,
4 acceptance and 17 security/integration tests. Added checks demonstrate:

- One lexical identity across punctuation variants; missing endings normalize
  to a dot without changing raw archived input.
- Observed punctuation and continuation/stopping patterns have no fixed
  question/answer labels.
- Negative feedback changes the selected continuation only in its rated input
  context; positive feedback and punctuation weights are applied independently.
- Neutral messages leave feedback counts unchanged. Signs never enter vocabulary.
- Ratings survive reopening, including a sign-only rating. Frozen and locked
  sessions cannot apply them, and rejected feedback leaves vault bytes unchanged.
- Legacy punctuation-bearing records are reused after lazy loading without
  deleting or replacing the old node identity.
- Read-only evaluation does not train or write to the vault.

The terminal smoke run passed with hidden credentials, learning, a positive
feedback acknowledgement, status counters, freeze, logout and exit. The release
binary was rebuilt and installed by atomic replacement.

## Initial v2.5.0 validation and historical measurements

The following records describe the initial build before these updates.


## Automated checks

- `cargo fmt -- --check`: passed.
- `cargo clippy --locked --all-targets -- -D warnings`: passed.
- `cargo test --locked --offline`: passed, 36 tests total:
  - 19 library tests (including bounded format-3 pages, graph and archive full
    verification, learned-context similarity, and cold ring/edge retention),
  - 3 independent acceptance tests (cold words after reopen, exact multiline
    archive retention, and continuation through a graph larger than RAM budget),
  - 14 security/integration tests (including freeze, tamper detection, archive
    cursor, and learned-context recall after a fresh login).
- `cargo build --locked --release`: passed. Binary copied to
  `bin/saeugling-linux-x86_64`.

## Terminal smoke run

Ran `start.sh` from a disposable copy with a temporary empty vault. Verified
hidden password setup and login, first-message imitation (`Du bist Jinx ich bin
Lumen` → `du bist jinx ich bin lumen`), `/status`, lexical `/recall`, full
`/verify-archive` and `/verify-graph`, `/freeze`, and `/exit`. The exact user and
assistant messages were returned by archive recall; the two full integrity
checks each succeeded. The smoke vault and test password were in `/tmp`, not in
the release package.

## CPU and memory measurement

Reproduced with `./scripts/benchmark.sh` on the build host's AMD Ryzen 9 5900X
(12 cores, 24 threads). The script builds the benchmark with two Cargo jobs;
`/usr/bin/time` measured the already-built example process. One run reported:

| Measurement | Result |
|---|---:|
| Empty-vault creation | 98 ms |
| Login and unlock | 89 ms |
| 100 steady chat turns | 530 ms total; 5.3 ms mean |
| Peak resident memory | 71,364 KiB |
| Encrypted snapshot after run | 1,264 bytes |
| Encrypted audit journal after run | 21,288 bytes |
| SQLite file after run | 159,744 bytes |

These are measurements from this host, not a low-end-device benchmark or a
performance guarantee. The executable and graph code do not require a GPU or a
pretrained model. A separate run on the intended potato hardware is needed to
assess its real latency and memory use.

## Limits

The live working set is capped at 128 graph nodes; one turn and one
`/continue` load up to four graph pages. Messages and file imports are capped at
128 tokenizer tokens. `/slaap` heals the active neighborhood only. Archive
recall scans 128 pages (4,096 episodes) per request and supplies a `--before`
offset when older episodes remain. `/verify-archive` and `/verify-graph` can
stream-check the entire store at explicit I/O cost.

No benchmark or test here demonstrates human-level language understanding or
shows that this learning method will acquire full grammar. Those remain
questions for future training runs.
