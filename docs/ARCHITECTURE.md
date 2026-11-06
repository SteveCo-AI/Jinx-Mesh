# Jinx-Mesh 2.6.1 architecture

`App` is the local authorization and integration boundary. A locked session has
no active key or graph. A frozen session moves its key to read-only session access and cannot train;
the CLI's idle worker also stops when frozen, logged out, or expired.

| Module | Responsibility |
|---|---|
| `model.rs` | Graph state, encrypted-page slices, and explicit activation status |
| `graph.rs` | Tokenization, observed word/context connections, learned output paths |
| `engine.rs` | Graph-only imitation and generation over a bounded active slice |
| `healing.rs` | Non-destructive duplicate aliases, edge consolidation, quarantine report |
| `retrieval.rs` | Ordered lexical, temporal, and learned-context archive handlers |
| `language.rs` | Lexical/structural parsing, context hashing, bounded observation and rating maps |
| `journal.rs` | Encrypted op journal: append, undo (steps/round), crash recovery fails closed |
| `gate.rs` | Integration gate: rules as data, rejects recorded as `integration_reject` |
| `mutation.rs` | Phase-2.5 novelty operator (off by default), folding detection, `/falsch` + `/echtes_wort` healing |
| `app.rs` | Authentication boundary, graph activation, recall, checkpoint, and session coordination (largest coordinator; planned de-splintering, see WARTUNG.md) |
| `store.rs` | Format-2 compatibility and format-3 encrypted paging, manifests, receipts, and archive (planned de-splintering, see WARTUNG.md) |
| `crypto.rs` | Password key derivation, bounded compression, AEAD and key handling |
| `main.rs` | CLI, hidden credentials, truthful progress, and idle prefetch scheduling |
| `tests.rs` | Library test suite (planned split by module, see WARTUNG.md) |

## Format 3 and lazy graph activation

A new vault stores each graph node and its outgoing edges as a separate
authenticated encrypted record. SQLite indexes use keyed HMAC identifiers;
the snapshot contains configuration, counters, and authenticated roots/counts,
not the entire graph. The active in-memory subset is capped at 128 nodes. Reads
fetch bounded seed and neighbor pages, and dirty records are committed without
replacing untouched cold records. Cold ring links and outgoing edges survive
active-slice updates.

The conversation archive is a separate table of individually encrypted exact
user and assistant episodes. Page-based scans are capped at 32 records. Graph
lexical nodes contain bounded source episode IDs, and a retrieval jump can load
up to 32 indexed source episodes. Timestamps and speaker provenance remain
metadata. They are available for recall and status, not emitted into lexical
generation.

Each inference turn and `/continue` loads at most four graph pages. A revision-
guarded encrypted checkpoint holds the query, compact visited IDs, frontier, and
provisional output. One background step per second can advance a pending
checkpoint while the terminal waits for ordinary chat input. Terminal progress
uses cursor save/restore and is paused at hidden credential prompts. Users can
also inspect the last prefetch status with `/status`.

Archive recall uses a controller with registered ordered handlers:

- **lexical** ranks episodes by exact token overlap;
- **contextual** computes sparse cosine similarity from the graph's learned
  weighted lexical neighbors and also rewards observed adjacent pairs;
- **temporal** ranks matching episodes by timestamp when explicitly selected.

No handler loads a pretrained embedding. Recall scans at most 128 archive pages
per request and returns a `--before` cursor for older ranges. `/verify-archive`
and `/verify-graph` stream over the full stored records when complete integrity
checking is requested.

## Learning boundary

An empty vault has no lexical graph or response templates. The learning rules,
tokenization, safety checks, budgets, and controller protocol are programmed
mechanics. The engine has no action executor and does not treat imported demo
metadata as language output. Graph context can support associative recall, but
it does not establish factual or general language understanding. Consolidation
applies only to the active graph neighborhood and does not delete neuron IDs.

## Module journal, gate, and mutation operator (2.6.x)

`journal.rs` persists every learning operation as an authenticated row
before it takes effect; `/undo` rolls back one or more steps or a whole round
and crash recovery discards a session rather than keeping a half-written
vault (fails closed). `gate.rs` encodes integration rules as data and records
rejected inserts. `mutation.rs` contains the optional novelty operator
(configuration-gated, default off) plus folding detection and the
`/falsch` / `/echtes_wort` healing commands — all journal-backed.

## Structural observations and rated replies

`language.rs` separates lexical tokens from positional structural marks and
supplies a default final dot when omitted. The bounded structural side network
learns punctuation and stopping from observed utterance positions. There are no
hardcoded question/answer sentence classes. Context hashes include lexical input
and structural marks. Signed DEV feedback updates context-specific start,
transition and marker weights, while neutral messages only learn language.

Feedback targets contain compact word sequences and structural marks, not
plaintext transcript files. Normal turns persist their target with authenticated
model metadata; continuation/read-only replies update the session target only.
Imports never interpret signs as dialog feedback. Old lexical record identities
remain stored; canonical matching reuses punctuation-bearing aliases lazily.

## Maintenance backlog (2.6.2 wave)

The module structure is sound; no rebuild is needed. Scheduled stepwise work
for the next maintenance wave (not part of a feature release):

1. Split `app.rs` (~2,067 lines): auth, chat/training, undo/heal, import, recall
   coordination each get a clear owner.
2. Split `store.rs` (~1,250 lines): DB access, encryption, graph paging, and
   archive each get a clear owner.
3. Reorganize `tests.rs` (~3,047 lines) by module.
4. Keep this document aligned with the version header each release.

Order rule holds: measurement waves (RW-1) take precedence over structural
refactoring — the experiment measures the shipped 2.6.1 mechanics, so the
measured code must not move underneath a running wave.
