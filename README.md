# Jinx-Mesh 2.5.0

Jinx-Mesh is the project; Jinx is the name of its experimental model; the Rust executable is still named
`saeugling`. A new vault starts with no vocabulary, neurons, connections,
response templates, or training corpus. The graph learns only from the text
provided to it. No conventional pretrained model or external embedding is used.
The executable has no GPU or machine-learning runtime dependency and runs on
the CPU.

## Start with an empty vault

```sh
./start.sh
```

The script creates `private/jinx.db` only when it does not already exist, with
directory permissions set to `0700`; it never initializes over an existing
vault. Choose a Developer username and a password of at least 12 characters. An
Authenticator second factor is optional. In chat, enter `/login` and answer the
separate hidden prompts. Normal messages are learned automatically.

For example, the first message `Du bist Jinx ich bin Lumen` is mirrored as
`du bist jinx ich bin lumen`. That is token imitation, not evidence that the
model already understands identity or pronouns. Learned word order and
punctuation affect later output, which can still be wrong.

## Graph, memory, and bounded loading

The learned graph remains the language core. A contextual recall handler builds
sparse word vectors from weighted connections the graph has learned; it has no
built-in word meanings. These associations can find a passage with related
learned context even when it shares no query word, but they do not establish
semantic understanding.

Vault format 3 stores graph records and exact conversation episodes separately.
Each user message and Jinx reply is encrypted as its own indexed archive block
with a speaker label, timestamp, and stable source ID. Graph nodes keep compact
links to the source episodes where their words were learned. Time metadata is
available for recall and does not become generated text.
`/train` and `/code` update the graph and retain an import digest; they do not
copy the original file into the exact conversation archive.

Inference activates at most 128 graph nodes in a working set, loading records
in small pages. It reports actual page and node counts as work proceeds. While
the terminal waits for a normal message, one bounded prefetch step can continue
a pending graph traversal in the background. It is silent during password and
Authenticator prompts. `/status` shows the latest prefetch state. Use
`/continue` to advance a pending traversal explicitly; this does not train a
second copy of the message.

Messages are limited to 128 tokenizer tokens and 16 KiB of UTF-8 text; file imports are
limited to 128 tokens and 256 KiB. These bounds keep the working set manageable.
Up to 128 new word nodes can be learned per message, within the resident graph
budget. Generated graph paths stop at observed sentence punctuation (`. ! ? …`),
a missing continuation, or a safety cap of 128 word nodes. Existing vaults using
the original twelve-word defaults automatically receive the increased budgets
when loaded by the engine; their learned graph is preserved.
If a graph traversal reaches its per-call budget, the reply is marked as
budget-limited and may be continued. The encrypted traversal checkpoint keeps
its query, compact frontier, visited IDs, and provisional output, and is
discarded when the model revision changes.

## Chat commands

| Command | Function |
|---|---|
| `/login` | Open a Developer learning session |
| `/freeze` | Stop learning and discard the write key |
| `/logout` | Lock the vault and release the loaded state |
| `/status` | Show global graph size, active working set, retrieval handlers, and prefetch state |
| `/recall TEXT` | Search encrypted episodes using lexical and learned-context ranking |
| `/recall --strategy lexical TEXT` | Search exact word matches |
| `/recall --strategy contextual TEXT` | Search learned graph-neighbor context |
| `/recall --strategy temporal TEXT` | Rank matching passages by timestamp |
| `/recall --before OFFSET TEXT` | Continue a bounded search farther back in the archive |
| `/verify-archive` | Stream a complete integrity check of archived episodes |
| `/verify-graph` | Stream a complete integrity check of graph records and references |
| `/continue` | Continue a pending bounded graph traversal |
| `/slaap` | Consolidate the active graph neighborhood and show a healing report |
| `/train FILE` | Learn a UTF-8 text file without executing it |
| `/code FILE` | Learn code tokens as graph observations without executing them |
| `/exit` | Exit and lock the vault |

Archive recall scans at most 128 pages of 32 episodes in one request. The CLI
reports actual page progress and gives a `--before OFFSET` cursor when older
episodes remain. Use the full verifier commands when checking every stored record; login checks
bounded manifests and individual records are authenticated when read.

## Healing and limitations

Consolidation adds aliases for exact duplicate observations and redirects
connections while retaining original neuron IDs. It records malformed
structural references for inspection; isolated or rare words are not treated
as bad data. `/slaap` operates on the currently active neighborhood, not the
entire paged graph.

The model learns word frequency, order, punctuation, and graph associations.
It does not guarantee grammar, stable identity reasoning, factual recall, or
human-level conversation. No word such as `gut`, `nein`, or `wichtig` has an
embedded sentiment or command meaning. These limits are part of the experiment.

## Existing vaults and development

Use `init VAULT` without import to create a clean v3 vault. Importing an older
state is explicit; its existing vocabulary and data are retained. Existing
version-2 vaults remain in their original format and are not automatically
rewritten as v3.

Build and validate with Rust stable:

```sh
cargo build --locked --release
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Security details and operational limitations are in `docs/SECURITY.md`.
Validation results are recorded in `docs/VALIDATION.md`.

To measure vault creation, login, steady chat time, and peak resident memory on
your own machine, run `./scripts/benchmark.sh`. It builds only the benchmark
example with two parallel build jobs and reports these measurements separately;
results depend on the machine and storage device.

## GitHub CI

The tested validation workflow is provided as `docs/ci-workflow.yml.example`.
To enable GitHub Actions, copy it to `.github/workflows/ci.yml` using a GitHub
credential with workflow permissions. CI is not enabled in this initial upload.

## Separate graph window

Chat opens a read-only Qt window showing up to 128 resident word nodes and
512 directed connections. Click a node to highlight its neighbors; hover to see
its learned strength. The drawing updates when snapshots change, without
continuous animation. Lazy-loading diagnostics are no longer printed in chat.
This window needs Python 3 and PySide6; the learning engine remains CPU-only.
Snapshots travel through a private process pipe and are not saved to disk.
Logout and session expiry clear the display; closing the window leaves chat usable.
The view shows the resident graph, not the full network or a literal thought trace.
