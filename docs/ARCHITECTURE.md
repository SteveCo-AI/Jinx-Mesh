# Jinx 2.5.0 architecture

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
| `app.rs` | Authentication boundary, graph activation, recall, checkpoint and session coordination |
| `store.rs` | Format-2 compatibility and format-3 encrypted paging, manifests, receipts, and archive |
| `crypto.rs` | Password key derivation, bounded compression, AEAD and key handling |
| `main.rs` | CLI, hidden credentials, truthful progress, and idle prefetch scheduling |

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
