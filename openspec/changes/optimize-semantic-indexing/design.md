## Context

CASS already separates canonical messages from rebuildable semantic vectors, but
`cass index` currently holds one SQLite writer transaction across ingestion,
FTS maintenance, all model inference, and all embedding writes. Missing messages
are loaded in identifier order and passed to FastEmbed in fixed groups of 32.
FastEmbed pads each group to its longest tokenized member, so identifier-order
batches repeatedly make short messages pay the inference cost of unrelated long
messages. The measured Xenia corpus has 18,875 searchable rows, 13,345 distinct
texts, and enough length skew that sorting by character length changes the
median batch maximum from 7,011 characters to 237 characters. About 29 percent
of rows duplicate searchable text.

The existing `i8-per-vector-symmetric;cosine=quantized-flat-exact` generation
already makes persisted-vector scanning compact and fast. This change targets
creation of those vectors, not their format or retrieval.

## Goals / Non-Goals

**Goals:**

- Reduce redundant transformer work through deterministic length-aware batches
  and exact-text inference reuse.
- Commit canonical and FTS state before the long inference phase, then make
  derived-vector progress resumable through bounded checkpoints.
- Make actual inference work and stored-vector progress visible as JSON on
  standard error.
- Select constants from an explicit Xenia benchmark while preserving vector and
  ranking behavior.

**Non-Goals:**

- Change the installed embedding model, quantization, exact cosine search, RRF,
  or reranking.
- Add a scheduler, async runtime, daemon, GPU path, ANN index, or tuning surface.
- Change canonical content, search projections, or the 90-day source cutoff.
- Put performance benchmarks in the ordinary test or CI gate.

## Ownership and Boundaries

- `app/semantic.rs` owns exact-text grouping, length ordering, inference
  batching, quantization, progress accounting, and the embedding summary.
- `app/storage.rs` owns selection of missing current-generation rows, bounded
  embedding writes, and SQLite checkpoints. Canonical and FTS commits remain a
  storage concern.
- `app/cli.rs` owns phase sequencing and the one final JSON response. It commits
  ingestion and FTS before starting derived embeddings and passes a progress
  sink to semantic indexing.
- The FastEmbed backend remains the only model implementation. Semantic code
  depends directly on it; storage never depends on model types.
- Retrieval consumes the same generation string and persisted i8 rows as before
  and does not know how inference work was scheduled.

## Decisions

### Group by exact searchable text before inference

Missing rows are grouped by byte-for-byte identical `search_projection` (or
canonical content when no projection exists). Each unique text is inferred once
per run, quantized once, and copied to every message identifier in that group.
All identifiers in a group are written before a checkpoint boundary, so a
resumed run cannot split one duplicate group across committed and uncommitted
work.

This uses an in-memory map over the rows already loaded by the current code and
adds no schema, persistent content hash, or text join. A persistent reuse index
was rejected because it expands canonical schema and invalidation logic for a
benefit not yet demonstrated beyond within-run duplicates.

### Order unique texts by length, then stable content order

Unique groups are ordered by UTF-8 byte length and then their text bytes before
forming inference batches. Byte length is a cheap monotonic proxy for tokenizer
work and clusters similarly sized inputs, substantially reducing BatchLongest
padding without invoking the tokenizer twice. The secondary key makes behavior
independent of message identifier and discovery order.

The accepted batch size is a compile-time constant selected by the focused
benchmark. It is not exposed as configuration. Candidates include the current
32 and larger batches; the chosen value must meet throughput and equivalence
criteria on Xenia rather than being assumed in advance.

### Preserve the established quantized vector contract

Every inferred f32 vector continues through the existing per-vector symmetric
i8 quantizer and norm calculation. Identical text uses the single resulting
quantized vector for all occurrences. The generation string and database row
format do not change. Reference-path tests compare quantized bytes and norms,
and a focused retrieval test compares result ordering.

### Separate durable canonical state from checkpointed derived state

After ingestion, CASS finalizes FTS and commits the writer transaction before
embedding inference begins. It then starts a new writer transaction, removes
stale-generation vectors, and writes completed exact-text groups. After a
bounded number of stored rows, it calls the existing writer checkpoint primitive
and continues in a fresh immediate transaction. The final checkpoint commits
remaining rows.

Coverage remains the readiness authority. A stopped run can leave partial
current-generation vectors, but `search` and `status` continue to reject that
database as semantically ready until every searchable message has a current
vector. The next `index` selects only missing rows. Stale-generation removal is
committed with the first derived checkpoint; until then rollback preserves the
previous state.

The checkpoint row target is a private constant chosen large enough to avoid
per-transaction overhead and small enough to bound lost inference. Checkpoints
never divide an exact-text group, even if one group exceeds the target.

### Emit progress from completed inference batches

Semantic indexing returns an `EmbeddingSummary` containing stored vectors,
actual model inferences, and duplicate reuses. After each inference batch, a
progress callback emits one newline-delimited JSON object to standard error with
the specified cumulative counters, elapsed time, and stored-vector rate. The
normal final `IndexResponse` remains the only standard-output object. Progress
write failures remain non-fatal, matching existing ingestion progress behavior.

### Benchmark outside ordinary gates

An ignored real-model benchmark test uses `CASS_TEST_MODELS_DIR` and a checked,
deterministic manifest that reproduces the measured Xenia length buckets and
duplicate multiplicities without committing private session text. It compares
the reference identifier-order path to candidate length-aware batching,
records stored and inferred throughput, and checks quantized-vector and ranking
equivalence. The repository records the command, machine, corpus-shape manifest,
batch size, and result in the change plan when the implementation is accepted.

## Tooling Compatibility

- Implementation language: Rust 2024 edition.
- Native tests: Rust unit and integration tests executed with Cargo Nextest;
  ignored real-model measurement is invoked explicitly with Cargo test/Nextest
  and `CASS_TEST_MODELS_DIR`.
- Veritas evidence: the configured native `rust-test` producer in
  `veritas.toml` supports the Rust tests and `veritas-test-macros` claim links.
  No cross-language bridge or fallback framework is required.

## Risks / Trade-offs

- Transformer floating-point results can theoretically vary with batch shape.
  Quantized-vector equivalence tests prevent accepting a faster policy that
  changes persisted results; if FastEmbed cannot meet that invariant, batching
  changes must be narrowed rather than relaxing the requirement silently.
- Grouping retains all missing searchable text plus grouping metadata in memory,
  as the current implementation already retains all missing messages. The
  additional map is bounded by unique missing texts and avoids text copies where
  ownership can be moved.
- More SQLite commits increase WAL/checkpoint overhead. A bounded private row
  target and benchmark measurement balance durability against write cost.
- Committing canonical state before vectors means an interrupted index is
  intentionally not searchable. This is explicit and recoverable through the
  existing semantic-coverage check.
- Character length imperfectly predicts token count, especially across scripts.
  It is chosen for simplicity; the benchmark determines whether it is adequate.

## Migration / Rollback

No database migration is needed because generation strings and vector rows are
unchanged. Existing complete databases remain complete; incomplete databases
resume by selecting missing rows. Rollback to the previous binary remains safe:
it can read all committed vectors and will regenerate any missing ones, though
it loses resumability during its own single transaction.
