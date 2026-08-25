## Context

CASS currently stores at most one vector per message and treats the build's
current embedding generation as both the only writable generation and the only
searchable generation. `cass index` commits vectors in bounded transactions,
but `search` and `status` require complete current-generation coverage. The
checkpoints therefore provide crash recovery without providing earlier search
readiness. Missing vectors are also selected by message identifier and then
globally reordered by text length, so checkpoint order has no useful recency
meaning.

The existing semantic generation hash includes model identity, vector format,
and inference-policy details. Some future generation changes can still share a
query embedding space, such as a batching-policy revision, while a model or
vector-space change cannot. That distinction is not currently represented.

Federated search returns one local `SearchResponse` plus per-remote
`NodeOutcome` metadata. Each node reads and searches its own SQLite database,
so readiness and coverage are necessarily node-local.

## Goals / Non-Goals

**Goals:**

- Serve hybrid search from any nonempty committed subset covered by one usable
  embedding generation.
- Make the lexical and semantic candidate universes identical for every search
  and keep each command on one committed SQLite snapshot.
- Commit recent missing messages first without giving up exact-text reuse or
  length-aware FastEmbed batches.
- Preserve a complete compatible serving generation during target replacement
  and switch generations atomically.
- Report enough node-local state to distinguish zero, partial, complete, and
  rollover coverage.

**Non-Goals:**

- Lexical fallback, background work, additional models, ANN search, a generic
  generation manager, or configuration for scheduling and checkpoint policy.
- Loading an old incompatible query model or preserving superseded generations
  after a completed switch.

## Ownership and Boundaries

`app/storage.rs` owns schema version 10, generation metadata, the singleton
serving/target state, coverage snapshots, newest-first missing-row selection,
generation-constrained FTS and vector reads, checkpoint promotion, and obsolete
generation cleanup. It receives concrete generation and embedding-space
identifiers from the semantic layer and has no dependency on model types.

`app/semantic.rs` owns the two deterministic identifiers, exact-text grouping,
checkpoint-window planning, length ordering inside a window, FastEmbed work,
and quantization. It passes a complete checkpoint window to storage before
asking storage to commit and reconsider serving state.

`app/cli.rs` loads the concrete models, opens one read transaction for the
coverage decision and hybrid retrieval, and serializes one shared
`SemanticCoverage` value. Local search puts that value at the response top
level. Federated aggregation retains the local value there and copies each
successful remote value into that remote's `NodeOutcome`; an outcome that
failed has no coverage object.

`app/federation.rs` bumps the private SSH protocol to version 3 because a
successful response now requires coverage metadata and the covered-subset
search semantics. It does not aggregate node counts into a misleading global
percentage.

The dependency direction remains concrete:

```text
FastEmbed identity + embedding work
                ↓
      SQLite generation state
        ↙                 ↘
covered FTS candidates   covered exact vectors
        ↘                 ↙
             RRF + rerank
                  ↓
      node-local coverage JSON
```

## Decisions

### Store multiple generations only during rollover

Change `message_embeddings` to use `(message_id, generation)` as its primary
key. Add an `embedding_generations` table mapping a generation identifier to an
embedding-space identifier, and add a singleton `semantic_state` row containing
nullable `serving_generation` and `target_generation` values. Foreign keys and
canonical message deletion continue to remove derived vectors.

The embedding-space identifier covers the pinned embedding model, pooling and
output dimensions, and persisted quantization/vector interpretation. The
generation identifier additionally covers inference and serialization policy.
Equal space identifiers permit current query vectors to score either
generation; unequal identifiers never do. These are concrete hashes produced
by `semantic.rs`, not a compatibility interface or registry.

On target preparation, storage records the current generation and space. If a
different serving generation is complete and has the same space identifier, it
remains serving. Otherwise there is no usable old serving generation; the
target becomes serving at its first nonempty committed checkpoint. When target
coverage becomes complete, the checkpoint transaction changes
`serving_generation`, aligns `target_generation` to the same generation, and
deletes superseded vector and metadata rows atomically. Target preparation
performs the same transition immediately when copied or previously committed
rows already provide complete target coverage. At steady state CASS therefore
stores one generation; two generations exist only during compatible rollover.

### Treat coverage and retrieval as one SQLite snapshot

Define one concrete `SemanticCoverage` value with nullable
`serving_generation` and `target_generation`, `serving_vectors`,
`target_vectors`, `searchable_messages`, `pending_vectors`, and `complete`.
Counts use distinct searchable message identifiers, never total rows across
generations. Pending is saturating `searchable_messages - target_vectors`.
Zero searchable messages produce null generations, zero counts, and complete
coverage.

After model loading, local search begins a deferred SQLite read transaction,
reads semantic state and counts, and keeps that snapshot through FTS candidate
selection, vector loading, fusion, document loading, and reranking input
selection. The FTS query joins `message_embeddings` on both message identifier
and the chosen serving generation. Exact-vector loading uses the same
generation. No uncovered row can therefore enter either candidate list, and a
concurrent embedding checkpoint cannot make response coverage disagree with
the searched set.

Search is available when models are installed, derived FTS state is clean, and
the snapshot has either a nonempty serving generation or an empty searchable
corpus. A nonempty corpus with zero usable serving vectors remains
`search-not-ready`.

### Use recency-ordered checkpoint windows

Select missing target rows by
`COALESCE(message.created_at, conversation.updated_at,
conversation.created_at, 0) DESC`, then message identifier ascending. Group
byte-identical searchable text once, assigning each group the order of its
newest member and retaining all member identifiers. Build checkpoint windows
in that group order up to the existing private row target without splitting a
duplicate group.

Within one window, sort groups by text byte length and stable text order before
forming FastEmbed batches. Commit only after every group in the window has been
written. This retains the measured padding reduction while making every
checkpoint a deterministic newest-first prefix modulo atomic duplicate groups.
Resume queries omit committed target rows and reconstruct the same order for
the remainder.

### Report coverage per federated node

A local successful `SearchResponse` includes `semantic_coverage`. During
federation, that top-level object continues to describe the local node, and
each successful remote `NodeOutcome` includes its own object. The merge does
not add counts because corpora may overlap and nodes may be at different
generations. Protocol version 3 rejects older successful envelopes that cannot
prove the covered-subset contract.

### BJ-1 — Separate serving and target generations

- Decision: persist explicit serving and target identifiers instead of deriving
  both from the build's current generation.
- Scenario: a batching-policy release needs several minutes to rebuild vectors
  even though the prior vectors remain query-compatible and complete.
- Source/owner: user-requested progressive readiness and atomic rollover.
- Simpler behavior considered: delete every old generation before rebuilding.
- Scope cost: one metadata table, one singleton row, and temporary duplicate
  vector storage; retire only if generation changes always alter model space.

### BJ-2 — Preserve length-aware inference inside recency windows

- Decision: order checkpoint windows by recency but length-sort groups inside
  each atomic window.
- Scenario: global length sorting improved cold embedding throughput, while
  identifier order made partial coverage arbitrary.
- Source/owner: measured `optimize-semantic-indexing` results and the new
  newest-first requirement.
- Simpler behavior considered: send every message strictly in timestamp order.
- Scope cost: a small deterministic window planner; retire if measurements show
  padding-aware ordering no longer matters for the selected model backend.

### BJ-3 — Keep coverage node-local

- Decision: expose local top-level coverage and coverage on each successful
  remote outcome without computing a fleet-wide percentage.
- Scenario: Xenia may be complete while a Mac is halfway through a different
  corpus, and replicated conversations make summed totals non-meaningful.
- Source/owner: existing rank-only federation boundary and requested truthful
  readiness reporting.
- Simpler behavior considered: sum all node counters into one object.
- Scope cost: one optional object per successful outcome; retire only if CASS
  gains a canonical deduplicated fleet inventory.

## Tooling Compatibility

- Implementation language: Rust 2024 edition.
- Native runner: cargo-nextest 0.9.143 for unit and CLI integration tests;
  Cargo remains the doctest runner.
- Veritas producer: the configured `rust-test` producer supports the focused
  Rust declarations and claim links in this change.
- Evidence access: project-bound `vtas` CLI fallback is operational; no
  cross-language bridge or fallback framework is required.
- No dependency or unsafe Rust is introduced.

## Risks / Trade-offs

- Compatible rollover temporarily approaches twice the vector storage. The
  completed switch deletes superseded rows in the same transaction, bounding
  duplication to the active rebuild.
- A read transaction held through exact scan and reranking can retain WAL pages
  while indexing commits. Searches are short relative to cold indexing; WAL
  growth is preferable to coverage/result skew. Measure before introducing a
  more elaborate snapshot protocol.
- A large duplicate-text group can exceed the checkpoint row target. It remains
  atomic by design, so that one checkpoint may be larger than normal.
- Recency fallback timestamps are approximate for messages lacking timestamps.
  Stable identifier ordering makes the approximation reproducible.
- Protocol version 3 creates a temporary compatibility failure during rolling
  deployment. Existing federation already exposes per-node incompatibility and
  preserves healthy results.
- Schema version 10 is not readable by the previous binary. Deployment backups
  remain the rollback boundary.

## Migration / Rollback

Schema migration 9 to 10 rebuilds `message_embeddings` with the composite key,
copies valid rows, creates generation metadata/state, and advances
`user_version` in one transaction. If the copied rows belong to the build's
exact current generation and cover the searchable corpus, that generation is
recognized as both serving and target without re-embedding. Unknown or
incomplete legacy generation rows remain non-serving and may be removed when
the current target is prepared.

Before fleet deployment, back up each database. Deploy protocol version 3 to
all three machines, verify that previously complete databases report complete
coverage, then exercise an interrupted disposable backfill to confirm partial
search and resume before indexing production databases. During a compatible
future rollover, allow temporary vector growth and verify that the old serving
generation remains named until the atomic switch.

Rollback before migration is a binary redeploy. After schema 10 opens, restore
the version-9 backup before redeploying the prior binary. Source histories and
model assets remain unchanged, so a database rebuild is the fallback if no
backup is available.

## Open Questions

None. The implementation should keep checkpoint size and all compatibility
identities private and deterministic.
