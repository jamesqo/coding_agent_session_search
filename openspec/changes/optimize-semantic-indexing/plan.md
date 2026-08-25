# Optimize Semantic Indexing Delivery Plan

Status: proposed

## Scope

This change accelerates and makes resumable the existing FastEmbed semantic
generation phase. It changes missing-row ordering, duplicate inference reuse,
derived-state transaction boundaries, embedding progress, and focused
measurement. It does not alter provider ingestion, searchable projections,
SQLite schema, model assets, quantized vector representation, exact cosine
retrieval, FTS, RRF, reranking, command surface, or configuration.

## Current Veritas Gate State

The repository-bound Veritas CLI is operational and its configured `rust-test`
producer matches the Rust 2024/Cargo Nextest tooling decision. The fresh
provisional scan sees 80 claims and 85 evidence records; 58 claims are covered,
14 are uncovered, 17 need review, and the repository has 79 current findings.
Those totals include pre-existing project findings plus the newly provisional
semantic-indexing claims. Claim locking and approval are intentionally deferred
until executable tests exist. Veritas remains the authoritative completion gate;
this plan does not reproduce its hashes or approvals.

## Preservation and Change Contract

- Preserve `Backend::embed`, existing quantization, the embedding generation
  string, `message_embeddings` rows, semantic coverage checks, and retrieval.
- Evolve `rebuild_embeddings` from identifier-order row batches returning a
  count into deterministic unique-text batches returning an embedding summary
  and emitting progress.
- Evolve CLI sequencing to commit canonical/FTS state before derived inference.
- Reuse `Storage::checkpoint_writer` for bounded derived commits; do not add a
  second transaction framework or schema.
- Preserve one final JSON response on stdout and the established JSON progress
  convention on stderr.
- Preserve the ignored real-model test boundary: models are supplied only via
  `CASS_TEST_MODELS_DIR` after explicit `cass models install`.

## Stack and Target Structure

```text
app/cli.rs
  index phase sequencing + progress serialization
      |
      v
app/semantic.rs
  exact-text groups -> stable length order -> FastEmbed batches
  -> existing quantizer -> EmbeddingSummary
      |
      v
app/storage.rs
  missing-row query -> vector upserts -> writer checkpoints

app/tests/cli_contract.rs
  stdout/stderr and interruption/resume contracts

ignored real-model tests
  reference equivalence + Xenia-shaped throughput measurement
```

The only model adapter remains concrete FastEmbed. Rusqlite remains the only
database boundary. Unit tests cover pure grouping/accounting; storage tests cover
transactions; CLI integration tests cover public JSON behavior; ignored
real-model tests own inference equivalence and performance evidence.

## Components

### C1: Deterministic inference planner

- Outcome: exact duplicate text is inferred once and unique texts are batched in
  stable length order.
- Foundation: `SearchableMessage`, `Backend::embed`, and existing quantization.
- Net-new work: grouping structure, deterministic ordering, group fan-out, and
  `EmbeddingSummary` counters.
- Non-goals: provider abstractions, tokenizer prepasses, persistent hashes, or
  runtime knobs.
- Claims: `semantic-indexing/repeated-text-reuses-inference` and
  `semantic-indexing/batching-preserves-vectors`.
- Dependencies: none.
- Risk: batch-shape floating-point variation; block acceptance if quantized
  equivalence fails.

### C2: Durable derived checkpoints

- Outcome: canonical/FTS changes become durable before inference, completed
  embedding groups survive interruption, and retry selects only missing rows.
- Foundation: `commit_writer`, `checkpoint_writer`, generation invalidation,
  missing-row selection, and semantic coverage checks.
- Net-new work: phase boundary, bounded checkpoint accounting, and resume tests.
- Non-goals: new tables, migration, background work, or partial-search mode.
- Claims: `indexing/partial-embeddings-resume` plus preservation of the existing
  incremental-indexing claims listed in the modified specification.
- Dependencies: C1 group boundaries.
- Risk: extra commits can reduce throughput; select a private checkpoint target
  from measurement.

### C3: Observable embedding progress

- Outcome: stderr reports cumulative stored, total, inferred, reused, elapsed,
  and rate fields while stdout remains one response.
- Foundation: existing index-progress JSON convention.
- Net-new work: progress event type/callback and final summary wiring.
- Non-goals: terminal UI, colors, progress bars, or configurable cadence.
- Claim: `semantic-indexing/progress-is-monotonic`.
- Dependencies: C1 counters; compatible with C2 checkpoints.
- Risk: excessive output; emit once per completed model batch.

### C4: Reproducible performance acceptance

- Outcome: select batch/checkpoint constants and record at least 220 stored
  vectors/second on Xenia with equivalent quantized results and ranking.
- Foundation: installed real-model test path and the recorded aggregate corpus
  distribution.
- Net-new work: deterministic private-text-free corpus-shape manifest, ignored
  reference/candidate harness, result record, and run instructions.
- Non-goals: CI performance gate or generalized benchmark framework.
- Claim: `semantic-indexing/cold-throughput-target`.
- Dependencies: C1-C3.
- Risk: synthetic text may not model tokenization adequately; confirm the
  selected implementation with a disposable copy of the actual Xenia database.

## Design Justification

The existing retrieval optimization and this change solve different halves of
semantic search: i8 flat-exact makes scanning persisted vectors cheap, while
length-aware deduplicated batching makes creating them cheap. Keeping both
inside the current concrete backend avoids replacing a working relevance model
or introducing an orchestration layer. Separating canonical and derived commits
is safe because semantic coverage, rather than transaction co-residence, already
defines readiness.

## Delivery Plan

- [ ] **PH-1 — Deterministic planner and unit proof (C1):** implement exact-text
  grouping, stable length ordering, group fan-out, and summary counters; add
  native tests for order independence, duplicate reuse, and quantized-vector
  equality. Exit: C1 tests pass and its two provisional claims have executable
  evidence links. Dependencies: none. Parallel group: A.

- [ ] **PH-2 — Checkpointed storage lifecycle (C2):** commit canonical/FTS state
  before inference, checkpoint only complete text groups at a bounded private
  threshold, and resume from missing current-generation rows. Exit: storage and
  CLI interruption tests demonstrate durable partial coverage, not-ready search,
  and missing-only retry. Dependencies: PH-1. Parallel group: B.

- [ ] **PH-3 — Progress contract (C3):** expose embedding counters and emit one
  monotonic stderr JSON event per inference batch without changing final stdout.
  Exit: CLI contract tests parse the event stream and final response and link the
  progress claim. Dependencies: PH-1; may proceed in parallel with PH-2 after
  the summary interface lands. Parallel group: B.

- [ ] **PH-4 — Real-model equivalence and tuning (C4):** build the ignored
  Xenia-shaped harness, compare reference and candidate vectors/ranking, measure
  candidate batch and checkpoint constants, then run against a disposable Xenia
  corpus copy. Exit: recorded result is at least 220 stored vectors/second and
  the cold-throughput claim has explicit evidence. Dependencies: PH-1 through
  PH-3. Parallel group: C.

- [ ] **IG-1 — Integrated correctness gate:** run Cargo Nextest, Clippy with
  warnings denied, rustfmt check, doc tests, and the explicit real-model tests;
  then run fresh Veritas status/report and resolve change-owned blocking
  findings without weakening specs. Exit: all native gates pass and no
  change-owned claim lacks its intended runnable proof. Dependencies: PH-4.

- [ ] **FC-1 — Consolidation, performance record, and rollout:** update this
  plan with measured Xenia cold/warm numbers and selected constants, reconcile
  provisional claim markers, obtain the required Veritas transitions, deploy
  through the existing main-branch workflow, and repeat cold/warm ingestion on
  dev-macbook. Exit: complete artifacts and code are pushed on `main`; deployed
  binaries report the expected version; Xenia and dev-macbook results are
  recorded. Dependencies: IG-1.

Ready set: PH-1. Critical path: PH-1 -> {PH-2, PH-3} -> PH-4 -> IG-1 -> FC-1.

## Traceability and Evidence Assignment

| Claim | Component | Intended executable proof |
|---|---|---|
| `semantic-indexing/repeated-text-reuses-inference` | C1 | Rust semantic unit test with duplicate identifiers and one backend inference per unique text |
| `semantic-indexing/batching-preserves-vectors` | C1/C4 | ignored real-model equivalence test plus retrieval ordering assertion |
| `semantic-indexing/progress-is-monotonic` | C3 | CLI integration test parsing stderr events and single stdout response |
| `semantic-indexing/cold-throughput-target` | C4 | explicitly invoked ignored Rust benchmark evidence on Xenia |
| `indexing/partial-embeddings-resume` | C2 | storage/CLI interruption and resume integration test |
| existing incremental-indexing claims | C2 | retain current linked tests; update only where transaction timing changes |

No claim is excluded as non-falsifiable. Performance proof is runnable but
explicitly invoked rather than part of the ordinary gate. After each phase,
Veritas should transition the owned provisional claims from uncovered or
needs-review toward linked evidence; approval and locking occur only after the
integrated implementation and real-model result exist.

## Risks and Open Questions

- The benchmark, not planning, chooses the final inference batch size and
  checkpoint row target.
- If exact i8 equality changes with batch shape, PH-4 must test narrower batch
  policies; changing the vector contract requires a spec revision rather than an
  implementation exception.
- Dev-macbook end-to-end duration remains an output of FC-1 because its corpus is
  larger than Xenia and was intentionally interrupted before the optimization.
