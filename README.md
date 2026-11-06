# Jinx-Mesh 2.6.0

Jinx-Mesh is the project; Jinx is the name of its experimental model; the Rust executable is still named
`saeugling`. A new vault starts with no vocabulary, neurons, connections,
response templates, or training corpus. The graph learns only from the text
provided to it. No conventional pretrained model or external embedding is used.
The executable has no GPU or machine-learning runtime dependency and runs on
the CPU.

## What is new in 2.6.0

This minor release merges the infant learning mechanics of the Python
reference into the Rust runtime. Everything listed here ships as a mechanism
with test coverage — none of it is a claim of understanding, and the
[Healing and limitations](#healing-and-limitations) section below still
applies in full.

- **Op-journal and `/undo`** — every learning move journals its exact old and
  new values before committing; each step, a range, or a whole round rolls
  back verbatim (see [Op-Journal and rollback](#op-journal-and-rollback)).
- **Anchor words and implicit feedback** — the two anchor roots are
  configuration data; implicit channels rate the previous reply from the
  following move, and anchor maturation lets the anchor weight decay when
  implicit signals agree without it.
- **Uncertainty fallback** — a mechanically unclear signal appends one
  configurable follow-up question instead of another blind guess, throttled
  per theme.
- **Self-healing** — `/falsch` and `/echtes_wort` fold or unfold words,
  consolidation damps contradicted edges into a suspicion ledger instead of
  deleting them, and every heal is gated by the verification-level registry.
- **Integration gate and anti-hardcode test** — each learning commit is
  checked against data-column rules before it persists, and a test scans the
  shipped sources for hardcoded behavior strings.
- **Novelty/mutation layer** — proposes at most one out-of-pattern word per
  answer and never self-rewards; only external signals (`+`, an anchor word,
  implicit use) confirm a mutant. It ships disarmed: arm it by setting
  `mutation_aktivier` to `true` in the `konfig` section of the start state
  used at `init` (the default start state is `fixtures/start.json`). Existing
  vaults keep their stored konfig unchanged.

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
budget. Generated graph paths stop at learned ending patterns, a missing
continuation, or a safety cap of 128 word nodes. Sentence-type meanings are not
assigned to punctuation. Existing vaults using
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
| `/freeze` | Stop learning; reload graph records for read-only inference |
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
| `/falsch X ... Y` | Fold a misspelled word X into the canonical word Y (pattern from `heal_befehl_muster`, write mode only) |
| `/echtes_wort X` | Promote a folded word X back to a standalone LEX neuron (pattern from `heal_befehl_muster`, write mode only) |
| `/undo` | Roll back the last committed learning step exactly |
| `/undo n` | Roll back the last n committed learning steps |
| `/undo round` | Roll back the round from the most recent consolidation (inclusive) |
| `/train FILE` | Learn a UTF-8 text file without executing it |
| `/code FILE` | Learn code tokens as graph observations without executing them |
| `/exit` | Exit and lock the vault |

Archive recall scans at most 128 pages of 32 episodes in one request. The CLI
reports actual page progress and gives a `--before OFFSET` cursor when older
episodes remain. Use the full verifier commands when checking every stored record; login checks
bounded manifests and individual records are authenticated when read.

## Op-Journal and rollback

Every learning move (a dialog turn, a file import, or a consolidation round)
writes an encrypted journal entry before its state change commits. Each entry
stores the exact old and new values of the affected section — neurons, edge
weights, and learned `state.extra` runs — so `/undo` restores the stored old
values verbatim instead of approximating them. The journal lives in its own
sealed store table and is capped by the `journal_kap` config key (default 500,
oldest entries expire). Dialog bookkeeping (interaction counter, archive
episodes) is history and is deliberately not rolled back; only learning is
reversible. If the process dies between the journal write and the commit, the
next login discards the unconfirmed step and `/status` reports it honestly.

## Healing and limitations

Consolidation adds aliases for exact duplicate observations and redirects
connections while retaining original neuron IDs. It records malformed
structural references for inspection; isolated or rare words are not treated
as bad data. `/slaap` operates on the currently active neighborhood, not the
entire paged graph.

The model learns word frequency, order, punctuation, and graph associations.
It does not guarantee grammar, stable identity reasoning, factual recall, or
human-level conversation. No word such as `gut`, `nein`, or `wichtig` has an
embedded sentiment or command meaning. The 2.6.0 mechanisms — journal
rollback, anchor words, the uncertainty fallback, healing, the integration
gate, and the mutation layer — are control and repair mechanics over learned
data; they do not add semantic understanding, and every reply is still
assembled only from learned words and transitions. These limits are part of
the experiment.

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

Frozen inference reloads bounded graph neighborhoods on demand without updating
the vault, training counters, archive, or checkpoints. Its decryption key remains
in session memory for reading and is cleared on logout or session expiry.
Write restrictions are enforced by the application, not by a separate cryptographic key.

## Structural markers and DEV feedback

Words and punctuation observations are stored separately. `Jinx`, `Jinx!`, and
`Jinx?` share one lexical identity in new training. The symbols `. ! ? : ;`
(and internal commas and ellipses) are structural observations. Missing terminal
punctuation is normalized to `.` for learning; raw input remains exact in the
encrypted archive. No marker is pre-labelled as a question, command, or answer.
End/continuation counts are learned from observed utterances, so punctuation
inside a multi-sentence message does not impose a fixed output stop.

In an authenticated DEV session, prefix the next message with `+` or `-` to rate
the immediately preceding model reply. Its context includes the input words and
structural markers. The following text is ordinary new training material:

```text
+ Mein Architekt heißt Lumen.
- Ich heiße Jinx. Mein Architekt heißt Lumen.
```

A prefix alone applies a rating without introducing words. Without a prefix,
there is no rating; normal language learning continues. Feedback is a separate
control signal and never enters vocabulary. A system acknowledgement confirms
successful application. Frozen and locked sessions cannot apply feedback.
`/status` includes feedback counts.

Feedback adjusts bounded context-specific scores for start words, transitions,
and observed punctuation. It does not weaken every use of the same words.
Scores are clamped to [-2, 2] in steps of 0.25; feedback cannot delete neurons.
Structural observations are bounded to 4096 contexts; feedback to 256 input
contexts with up to 256 transition/marker entries each. These are learned side
networks stored as encrypted graph metadata. This is experimental reinforcement,
not a guarantee of question understanding or semantic generalization.

Older punctuation-bearing node records are retained and reused lazily through
bounded alias lookups. This does not perform a full global neuron consolidation.
A previous reply target survives reopening after a normal training turn. Explicit
continuations and read-only inference update the in-memory target; background
prefetch never rates or replaces the last visible reply.

## Anchor words and implicit feedback

There is no ritual feedback word. The two anchor roots are configuration data:
`anker_plus` (default `super`) and `anker_minus` (default `nein`) in the `konfig`
section of the start state. They are never hardcoded in code, and their words are
ordinary learned LEX nodes with no special status — editing the config redirects
the mechanism. When a message contains a configured anchor word as a standalone
token, the previous reply is rated additionally to the explicit `+`/`-` system
with the weight `anker_gewicht` (default 0.5), on top of the implicit share.

The primary channels are implicit and need no anchor word at all. Channel 1
(execution competence) is read from the next training move by comparing two
token sequences — pure structure recognition, no word lists: repeating the
previous question word for word is one negative rating step; moving on to
something new confirms lightly (`implizit_positiv`, default 0.1). An explicit
`+`/`-` rating is the definitive signal of its move and replaces the implicit
one; silence stays neutral. Implicit signals adjust the same bounded
context-specific scores as explicit feedback and never enter the explicit
feedback counters. Channel 2 (result value) is a reserved data slot
(`kanal2_letzte`); the Block-3 fallback fills it from usage.

Anchor maturation: every anchor-free implicit signal is recorded per anchor in
`anker_maturing` (rolling window of `anker_reif_fenster`, default 50 cases).
Once the window matches an anchor's signal in at least
`anker_reif_ueber_einstimmig` (default 85%) of cases, the anchor weight steps
down by `anker_reif_schritt` (default 0.05) toward `anker_gewicht_min`
(default 0.0) — like a child understanding praise without the word. The lowered
weight is learned data in `state.extra`, so the op-journal records every step
exactly, rolls it back on `/undo`, and marks the move's entry with the art
`feedback`. Vaults predating this mechanism load unchanged; the engine restores
the missing config keys from the fixture defaults.

## Uncertainty fallback

The Kap.-19 fallback is a mechanical uncertainty reaction, not introspection.
After a training move that brought no anchor word and no explicit `+`/`-`
rating, the signal is unclear when the move's conversation carries
contradictory context-feedback values in its own context: the strongest
positive AND the strongest negative value both reach `fallback_unklar_min`
(default 0.25; the rating channels already clamp scores to [-2, 2], which is
the band's upper end). The compared values are the answer path's transitions
and every input-to-answer word pair.

Instead of another bare guess, the move appends ONE utterance to the normal
answer. Its building blocks are configuration data, never code strings:
question scaffolds from `fallback_frage_geruest` and the learned conversation
word carrying the strongest contradiction, inserted at the `{wort}`
placeholder. The scaffold variants rotate by a usage counter
(`fallback_geruest_zaehler`), so no static text repeats. Per theme (input
digest, counted in `fallback_pro_thema`) at most `fallback_drossel_max`
(default 2) questions are asked; after that the move appends the configured
status announcement from `fallback_status_durchsage` instead of a further
question.

The utterance never becomes a rateable reply: the move stores no feedback
target, so no `+`/`-` rating, no repetition comparison, and no anchor
maturation can consume it — rating it fails honestly. The user's answer to
the question is a normal learning move and fills the Kap.-18 channel-2 result
slot `kanal2_letzte` from usage (`offen` while the question waits, `ergebnis`
with the recorded answer). Fallback events are ordinary `learn` entries in
the op-journal, so `/undo` rolls the question bookkeeping back exactly.
Vaults predating this mechanism load unchanged; the engine restores the
missing config keys from the fixture defaults.

## Self-healing (Kap. 21/22)

Wrongly learned words must not pass on their error forever, but healing is a
mechanism, never hardcoded behavior. Every heal runs on a verification level
from the `verifikations_ebenen` registry in `state.extra` — data, restored from
the fixture defaults for vaults predating the mechanism. Jinx-Mesh starts with
exactly the two levels that have real sources: `wort`
(`kontext_aehnlichkeit`) and `struktur` (`konkurrierende_pfade`). Each level
carries a source designator, a `verification_credit` counter, and a
`heil_schwelle`. Without a registered source no heal may run on the level —
commands fail loudly, the automatic folding stays off. Every confirmed heal
grows the level's credit and lowers its `heil_schwelle` by `heil_schritt`
(default 0.1) toward `heil_schwelle_min`; failed heals (alias cycles found and
quarantined by the healing pass, a fold the verification source forbade) raise
it again toward `heil_schwelle_max`.

Spelling folding: before `grow` creates a new neuron, the closest resident
canonical LEX node within the Levenshtein distance `folding_schwelle`
(default 2, own O(n*m) implementation, no new dependency) with at least
`folding_min_nutzt` (default 2) uses gets the word folded in — no independent
node. Instead a helper neuron keeps the misspelling visible: `ring` points to
the canonical word, `neuron_aliases` carries the alias entry, the node is
marked `folding_origin`, and from then on `word()` resolves the token to the
canonical node so its edges route through the alias. The fold only runs when
the word level's verification evidence (normalized Levenshtein similarity)
reaches the level's `heil_schwelle`; the code corpus namespace (`code:word:`,
`code:char:`) never folds. The thresholds are learnable: a `/falsch`
correction of words farther apart than the effective distance raises the
learned `folding_schwelle` in `state.extra` — the config value stays the
editable start value.

Dialog healing follows the pattern data in `heal_befehl_muster` (default
`"{x} ist {y}"` for `/falsch`, `"{x}"` for `/echtes_wort`) — the code contains
only mechanics, editing the config redirects the commands. `/falsch X ist Y`
folds X into the ring of Y: every edge from or to X is rewritten onto Y
(conflicting pairs keep the stronger weight), the origin stays visible, and
the words' records load on demand so the heal also reaches words outside the
currently resident slice. `/echtes_wort X` removes X's ring and alias
references and makes it a regular standalone LEX neuron again. Both commands
are write-mode only — `/freeze` refuses them like DEV feedback — and every
heal is one op-journal entry of the art `heal`, exactly rollbackable with
`/undo`.

Consistency check: each consolidation round inspects every edge the confirmed
path weights contradict — weight below `verdacht_schwelle` (default -0.5)
while both end nodes reached `verdacht_min_nutzt` (default 3) uses. The
structure level's verification source must be registered and its
`heil_schwelle` gates how strong the contradiction must be; a confirmed
contradiction damps the edge by `daempf_faktor` toward zero and appends it to
the `verdachtsverzeichnis` ledger (`kind`, `a`, `b`, `alt`, `neu`, `zeit`)
instead of deleting anything — failed attempts stay as memory.

## Integration gate (Kap. 24) and anti-hardcode (Kap. 20)

Every learning commit — a dialog move, a file import, or a consolidation
round — passes the integration gate before it persists. The gate is pure
mechanics; the rules are data columns in `gate_regeln` (konfig section of
the start state): `erwartet_neue_knoten_max` (default 128, the resident
learning budget), `erwartet_entfernt` (default `false` — learning never
removes nodes), `erwartet_gewicht_range` (default [-1, 1], the legal edge
weight band), and `pflichtfelder_extra` (default empty — `state.extra` keys
a move must leave present). A rule change is a JSON edit with no code
change; a declared column the registry cannot interpret fails loudly
instead of passing silently. A rejected move commits nothing: the state is
restored exactly, the terminal shows the honest system notice
`Integration-Prüfung fehlgeschlagen: <reasons>`, and the refusal is
recorded as an op-journal entry of the art `integration_reject` (confirmed
by a bookkeeping commit of the unchanged state, so it survives a reload and
rolls back with `/undo` like every entry). Vaults predating the gate load
unchanged; the engine restores the rule defaults from the fixture.

The Kap.-20 anti-hardcode rule is enforced by a test (`test_anti_hardcode`)
that scans every shipped `src/*.rs` file (the test file itself is the test
filter) for string literals carrying behavior: complete sentences of more
than three words, or any configured anchor word, question scaffold, or
status announcement from `fixtures/start.json` — raw or with the `{wort}`
placeholder filled in. Error, validation, and terminal system messages are
the allowed exception category; a behavior formulation in code fails the
test suite. A second test feeds simulated violations through the same
scanner to prove the guard itself fires.

## Novelty operator / mutation layer (Phase 2.5)

The third building block next to the lexical graph and the structure
markers: a proposal mechanism for novelty. Every switch and threshold is
konfig data (`mutation_*` keys in the start state); the layer ships
disarmed — `mutation_aktivier` defaults to `false` and only the vault's
konfig arms it. A training move draws once against `mutation_rate`
(deterministic SHA-256 seed from the state, xorshift64* dice — the same
state always proposes the same mutation) and may then insert at most ONE
word into the answer: a learned word or a fresh input token (a word the
import budget never grew) that the learned patterns do NOT place at the
chosen spot (no recorded continuation, no edge seen more than once).
`mutation_temperatur` (default 0.2, conservative) regulates how far down
the strength ranking the dice may reach.

Selection uses no extra network: the main net scores the mutant like any
sentence — average confirmed edge weight of consecutive pairs plus
structure-marker consistency — and compares it to the original.
`mutation_schwellwert` (default 0.8) decides candidate vs. discard; a
discard stays in `mutations_memory` (damped, Kap.-5: failures remain
visible) and dampens its attack point.

Model-collapse protection (binding): the layer may only PROPOSE.
Festigung happens solely through external signals — an explicit `+`, a
configured anchor word, or the Kap.-18 implicit channel of the following
move. A confirmed mutant grows as a JUNGE NEURON at the mutation point (a
fresh word is grown only by the confirmation, never by the proposal) with
the origin visible on the node; a negatively rated mutant damps its attack
point. Nothing self-rewards: without an external signal the festigung
counter never moves, run after run. In `/freeze` and locked mode the layer
never runs — it lives exclusively in the training move.

The Mutations-Wegweiser (`mutations_wegweiser`, bounded, rotated) learns
the success landscape of attack points — which (prev, next) environments
brought externally rewarded mutants — never the word itself. Once a point
becomes predictably successful (score over `mutation_langeweile_schwelle`,
seasoned by `mutation_langeweile_ab_versuchen` attempts), the boredom
guard raises the layer's wildness by `mutation_wildheit_schritt`: the jump
radius widens and the predictable point is avoided; without a predictable
point the wildness relaxes again. The system stays at the edge of its
ability, where novelty exists; the concrete outcome stays dice-rolled.
