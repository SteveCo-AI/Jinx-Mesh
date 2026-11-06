# Jinx 2.5.0 implementation record

## Implemented

- Fresh format-3 vaults begin with no lexical graph content or response corpus.
- The learned graph remains the only language-generation core. Context-conditioned
  successor counts retain observed order and punctuation.
- Exact user and assistant turns are stored as separately encrypted, indexed
  archive episodes with timestamps, speaker labels, and graph-node source links.
- Graph data is stored as encrypted per-node records addressed through keyed
  HMAC indexes. The login snapshot contains metadata and authenticated manifest
  values rather than a full graph copy.
- Inference uses a 128-node working set and bounded graph pages. Progress callbacks
  report actual nodes, pages, and linked archive records loaded.
- A revision-guarded traversal checkpoint retains a compact frontier and visited
  IDs. `/continue` resumes it without retraining; the CLI also advances one
  bounded checkpoint step per second while waiting at the ordinary chat prompt.
- `/recall` routes through ordered lexical, learned-context, and temporal
  handlers. Context vectors come only from learned graph edge weights. Each call
  scans at most 128 archive pages and reports a `--before` cursor when more old
  episodes remain.
- `/verify-archive` and `/verify-graph` perform complete streaming integrity
  checks without loading the entire archive or graph into memory.
- Consolidation retains original neuron IDs, records aliases for exact duplicate
  observations, merges learned connections, and reports structural quarantine.
  The CLI identifies that consolidation covers the active neighborhood.
- Existing format-2 vaults are read in their original format; conversion to a
  new format-3 vault is explicit rather than automatic.

## Limits

The working set is bounded to 128 nodes. A single message or file import is
limited to 128 tokenizer tokens; larger text must be divided into portions.
One input turn loads at most four graph pages before returning a partial result;
one `/continue` also loads at most four pages, while idle prefetch advances one.
The checkpoint has a fixed size and visited-ID ceiling, after which it reports
that continuation is unavailable.

Normal login checks small archive and graph manifests and the archive sequence
count. Every selected block is AEAD-authenticated when read. A complete record-
by-record scan is available through the explicit verification commands, which
take I/O proportional to the stored data. The v3 lazy path does not decrypt the
entire graph on login.

Graph context can improve associative retrieval when learned connections exist.
It supplies no pretrained concepts, language model, or factual reasoning. The
project still needs new training runs to measure language acquisition quality.
Timestamps remain recall metadata and are excluded from generated lexical text.

## Verification and packaging

Completed: formatting, the locked test suite, Clippy with warnings denied, CLI
smoke on a fresh disposable vault, release build, benchmark run, and release
archive/checksum generation. Exact test counts, the host benchmark, and limits
are recorded in `VALIDATION.md`.
