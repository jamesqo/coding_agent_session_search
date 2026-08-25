# Progressive Semantic Readiness Delivery Plan

Status: proposed

Spec: `openspec/changes/progressive-semantic-readiness/specs/`

Evidence: fresh project-bound Veritas status and claim diff on 2026-08-25;
scoped claim wording remains provisional pending user review and lock

Delivery: phased

Verification cadence: plan-final

## Scope

Deliver partial semantic readiness, deterministic recent-first embedding
checkpoints, compatible-generation rollover, and node-local coverage reporting
without changing CASS's models, vector format, exact search, fusion, reranking,
providers, commands, or explicit model-install boundary.

The change evolves the version-9 single-generation vector schema into a
version-10 serving/target schema. It does not add lexical fallback, a daemon,
background work, ANN indexing, provider abstractions, configuration knobs, or
dependencies. Superseded vectors are temporary migration state, not retained
release history.

The plan-final cadence is deliberate: storage migration and protocol safety
receive immediate focused proof in their owning phases, while the cached full
Nextest, Clippy, formatting, doctest, and Veritas gates run once over the
integrated changeset. This follows the user's request to avoid repeatedly
paying for broad low-signal test runs while never deferring a red focused check,
data-integrity proof, or compatibility transition.

## Current Veritas Gate State

Project-bound CLI access is operational through:

```text
vtas --project /home/james/scratch/coding_agent_session_search ...
```

The fresh repository-wide status reports 95 claims, 91 evidence declarations,
104 links, 107 approvals, 62 covered claims, 13 uncovered claims, and 91
findings. The current change contributes 15 added claims and 5 changed claims.
Its provisional wording accounts for 15 missing lock entries and five stale
lock entries; the new claims do not yet have implementation evidence or
approval. Other missing approvals, orphan approvals, and uncovered claims
already exist outside this change and are baseline repository work, not hidden
inside this delivery.

No claim is excluded from coverage: every changed behavior can be falsified by
Rust unit or CLI integration tests through the configured `rust-test` producer
and executed by cargo-nextest. Before PH-1 code edits, the user-reviewed wording
must be locked and a plan-base gate recorded. Final completion requires no
unresolved finding attributable to these 20 claim deltas and no worsening of
the repository-wide baseline.

## Preservation and Change Contract

| ID | Decision | Rationale | Consequence |
|---|---|---|---|
| PC-1 | Preserve mandatory hybrid retrieval and typed failure; evolve only its readiness boundary | Partial coverage must remain semantic and must not reintroduce the removed lexical mode | FTS, vectors, RRF, and reranking all use one serving-generation subset |
| PC-2 | Replace one-row-per-message vector storage with transient multi-generation storage | A complete compatible generation must remain searchable while its replacement builds | Schema 10 keys vectors by message and generation and stores serving/target state |
| PC-3 | Preserve exact i8 cosine search, FastEmbed models, text projection, FTS5, RRF, rerank limits, and model installation | These are measured foundations and explicitly outside the change | No relevance algorithm, model asset, or dependency change is permitted |
| PC-4 | Evolve federation protocol 2 to protocol 3 | An old response cannot prove covered-subset semantics or provide node coverage | Rolling deployment exposes typed incompatible-node outcomes until all binaries match |
| PC-5 | Preserve canonical ingestion, tombstones, provider-scan safety, JSON-only output, and the six-command CLI | Progressive readiness is derived-state behavior | No parser, config, command, or canonical identity redesign is included |
| PC-6 | Remove obsolete all-or-nothing coverage and destructive stale-generation helpers after consumers migrate | Parallel old/new readiness paths would obscure the serving authority | One coverage snapshot and one generation-transition path remain at consolidation |

## Stack and Target Structure

```text
app/semantic.rs
  generation + embedding-space identity
  recency checkpoint-window planner
  length-aware FastEmbed batches
                 ↓
app/storage.rs (schema 10)
  embedding_generations + semantic_state
  serving/target coverage snapshot
  generation-filtered FTS + exact vectors
                 ↓
app/cli.rs
  one read snapshot + readiness + JSON
                 ↓
app/federation.rs
  protocol 3 + per-node coverage metadata
```

Production remains synchronous Rust with Rusqlite, SQLite FTS5, and the one
concrete FastEmbed backend. Tests remain beside their owning modules plus
`app/tests/cli_contract.rs`; no alternate test framework or dependency is
introduced.

## Components

### C-1 — Semantic generation storage

- Outcome: schema 10 can hold one steady-state generation or two compatible
  rollover generations and compute a consistent coverage snapshot.
- Foundation: schema migrations, writer checkpoints, cascading message
  deletion, quantized-vector validation, and read-only status snapshots in
  `app/storage.rs`.
- Net-new work: composite vector key, generation metadata, singleton state,
  target preparation, checkpoint promotion, cleanup, and migration-9 fixture.
- Owned claims: `progressive-readiness/compatible-rollover-keeps-serving`,
  `progressive-readiness/compatible-rollover-switches-atomically`,
  `semantic/compatible-generation-rolls-over`, and
  `indexing/complete-scan-purges-missing-source`.
- Dependency: none after the contract gate. Consumers depend on its concrete
  coverage and generation APIs.
- Tests/evidence: storage unit tests for migration preservation, incompatible
  target reset, partial promotion, compatible retention, atomic completion,
  cascade cleanup, corrupted state rejection, and read-only older-schema
  behavior.
- Non-goals: generic compatibility negotiation, model loading, or retained
  historical releases.

### C-2 — Deterministic embedding progression

- Outcome: every committed checkpoint is the newest deterministic missing
  prefix modulo complete duplicate-text groups, and resume performs no repeated
  inference for committed target rows.
- Foundation: exact-text reuse, byte-length ordering, bounded FastEmbed waves,
  JSON progress, and durable writer checkpoints in `app/semantic.rs` and
  `app/storage.rs`.
- Net-new work: recency sort keys, recency-ranked duplicate groups, bounded
  checkpoint windows, length sorting inside each window, and semantic-aware
  checkpoint commits.
- Owned claims: `progressive-readiness/newest-first`,
  `progressive-readiness/interruption-resumes`,
  `indexing/partial-embeddings-resume`, and
  `indexing/full-rebuild-is-explicit`.
- Dependency: C-1 state and composite writes.
- Tests/evidence: pure planner tests for timestamp fallbacks/ties/groups and
  storage integration tests that interrupt after a checkpoint, inspect the
  committed prefix, resume only missing rows, and preserve serving state during
  `--full` replacement.
- Non-goals: work stealing, adaptive batching, or configurable checkpoint size.

### C-3 — Covered local retrieval and truthful JSON

- Outcome: local search and status use one committed coverage snapshot; every
  successful result is hybrid and covered by exactly its reported serving
  generation.
- Foundation: FTS5 BM25, exact semantic vector loading, RRF, bounded reranking,
  typed readiness/model errors, and JSON response structs.
- Net-new work: `SemanticCoverage`, read-snapshot lifecycle,
  serving-generation joins in FTS, coverage-bound exact vectors, partial/zero
  readiness rules, and response serialization.
- Owned claims: `progressive-readiness/partial-search-is-semantic`,
  `progressive-readiness/results-use-one-generation`,
  `progressive-readiness/zero-coverage-fails`,
  `progressive-readiness/coverage-is-reported`,
  `semantic/partial-coverage-reranks-with-models`,
  `semantic/zero-coverage-fails-search`,
  `status/partial-coverage-is-ready`,
  `status/rollover-distinguishes-generations`,
  `status/zero-coverage-recommends-index`,
  `status/semantic-search-ready`, and
  `status/zero-searchable-messages-can-be-ready`.
- Dependencies: C-1 coverage/state and C-2 checkpoint behavior.
- Tests/evidence: storage candidate-boundary tests and CLI contract tests for
  zero, partial, complete, empty, and rollover JSON; one concurrency test holds
  a read snapshot while a writer commits and proves results/counters do not
  mix snapshots.
- Non-goals: a fleet aggregate, lexical fallback, or response-schema discovery.

### C-4 — Federated coverage protocol

- Outcome: protocol-3 nodes carry their own coverage metadata while rank merge
  remains unchanged and deterministic.
- Foundation: versioned stdin envelopes, bounded SSH workers, node outcomes,
  and rank-only merge in `app/federation.rs`.
- Net-new work: protocol bump, coverage deserialization/validation, remote
  outcome propagation, and top-level-local/per-remote JSON assembly.
- Owned claim: `progressive-readiness/federated-coverage-is-node-local`.
- Dependency: C-3 response contract.
- Tests/evidence: federation unit tests for protocol-2 rejection, protocol-3
  acceptance, differing node coverage, failed-node omission, and unchanged
  merged rankings; CLI integration fixture for the final envelope.
- Non-goals: summed fleet coverage, cross-node generation matching, or remote
  indexing.

## Design Justification

The material long-lived choices are already recorded as BJ-1 through BJ-3 in
`design.md`. No additional abstraction, dependency, compatibility layer, or
operational mechanism is introduced by this plan. Phase ownership is
sequential because `app/storage.rs` and `app/cli.rs` are compact shared seams;
attempting parallel edits would create merge conflict without independent
delivery value.

## Delivery Plan

- [ ] **PH-0 — Contract approval and plan-base gate.** Depends on nothing.
  Present the proposal/spec/design/plan for user review, reconcile any wording
  change through the spec workflow, lock the 20 changed claim meanings, and
  record a clean Rust plan-base using the repository's four canonical commands.
  Capture fresh Veritas diff/status/report output. Exit: wording is locked with
  no live/lock drift for scoped claims; baseline compile/harness/infrastructure
  is green; any unrelated repository Veritas findings are explicitly unchanged.

- [ ] **PH-1 — Schema 10 and generation-state foundation.** Depends on PH-0.
  Implement C-1 exclusively in `app/storage.rs` and its module tests. Preserve
  complete exact-current version-9 databases on migration, reject malformed
  state, and prove target preparation, partial promotion, compatible retention,
  atomic switch/cleanup, canonical deletion, and rollback. Run the focused
  storage migration/state tests immediately; any failure blocks. Exit: the
  storage API exposes one validated coverage snapshot and one checkpoint
  transition path with all C-1 claims linked to passing native evidence.

- [ ] **PH-2 — Newest-first checkpoint scheduling.** Depends on PH-1. Implement
  C-2 in `app/semantic.rs` plus the bounded missing-row seam in
  `app/storage.rs`. Replace global length ordering with recency checkpoint
  windows and keep length ordering inside each window. Adapt progress accounting
  without changing its output fields. Run planner, duplicate-group, checkpoint,
  resume, and explicit-full focused tests immediately. Exit: interruption leaves
  a deterministic recent covered subset, resume selects only missing target
  rows, and C-2 claim links have passing evidence.

- [ ] **PH-3 — Partial local search and coverage responses.** Depends on PH-2.
  Implement C-3 in `app/storage.rs`, `app/semantic.rs`, `app/cli.rs`, and focused
  CLI tests. Load models before beginning the retrieval snapshot, constrain both
  candidate sources to the selected serving generation, serialize coverage,
  and remove obsolete complete-only helpers. Run candidate-universe,
  read-snapshot, readiness-precedence, JSON-contract, and no-fallback focused
  tests immediately. Exit: zero coverage fails, partial and empty coverage obey
  the specs, uncovered FTS matches cannot escape, and all C-3 claims have
  passing native evidence.

- [ ] **PH-4 — Federation and integrated behavior gate.** Depends on PH-3.
  Implement C-4 in `app/federation.rs`, `app/cli.rs`, and federation/CLI tests;
  bump the private protocol to 3. Then exercise the whole local lifecycle on a
  disposable database: migrate, interrupt after one checkpoint, search, resume,
  complete, and simulate a compatible rollover. Exercise a mixed-coverage
  federated fixture and confirm merge ranks remain stable. Protocol and
  migration focused tests are immediate blockers. Run the ignored real-model
  hybrid integration test explicitly through cargo-nextest with
  `CASS_TEST_MODELS_DIR=/home/james/.local/share/cass/models`; this proves the
  partial covered path reaches actual embedding, fusion, and reranking without
  downloading assets. Exit: every scoped claim has a concrete passing test
  declaration/link, protocol 2 is rejected, the real backend passes, and the
  integrated JSON contract is internally consistent.

- [ ] **PH-5 — Consolidation and terminal evidence.** Depends on PH-4. Remove
  superseded helpers, dead schema branches introduced only during development,
  duplicate fixtures, and stale all-or-nothing wording while preserving the
  locked behavior. Run `cargo fmt --check`, full cargo-nextest, strict Clippy,
  and doctests with the AGENTS-specified persistent target/temp environment.
  Reconcile Veritas claim links, perform semantic link review, approve passing
  evidence, and run final project-bound claims diff/status/report. Exit: all
  scoped claims are locked, covered, reviewed, approved, and drift-free; all
  Rust gates pass; the repository-wide Veritas baseline is not worsened; one
  reviewable commit is ready on `main` for push and fleet deployment.

Dependency edges:

```text
PH-0 → PH-1 → PH-2 → PH-3 → PH-4 → PH-5
```

Initial ready set: `PH-0`.

Safe parallel groups: none. The implementation is small and each phase consumes
the preceding storage/response contract; shared ownership of `app/storage.rs`
and `app/cli.rs` makes parallel code edits counterproductive. Cargo-nextest
still parallelizes independent test execution at the integration gates.

## Traceability and Evidence Assignment

| Phase | Claim ownership | Evidence transition |
|---|---|---|
| PH-0 | all 15 added and 5 changed meanings | provisional markers reviewed and locked; stale/missing scoped lock findings cleared |
| PH-1 | rollover storage and purge claims owned by C-1 | new storage declarations discovered, linked, semantically reviewed, then approved after focused pass |
| PH-2 | newest-first, interruption/resume, and full-index claims owned by C-2 | planner/checkpoint declarations discovered and linked after focused pass |
| PH-3 | partial/zero/complete local retrieval and status claims owned by C-3 | storage plus CLI declarations linked only where each test directly falsifies the scenario |
| PH-4 | node-local federation claim owned by C-4; cross-component scenarios rechecked | protocol/integration declarations linked and all scoped missing-evidence findings cleared |
| PH-5 | no new behavior | native discovery refreshed, every changed link reviewed, passing evidence approved, final drift/status/report recorded |

No `[[coverage.exclude]]` entries are planned. Gherkin scenarios remain
normative documentation; Rust declarations are the runnable evidence. One test
may support several claims only when its assertions independently exercise each
named behavior. Existing evidence for the five changed claims must be reviewed
against the new meaning rather than automatically reused.

Canonical final commands:

```bash
env TMPDIR=/home/james/scratch/cass-tmp \
  CARGO_TARGET_DIR=/home/james/scratch/cass-targets/integrated \
  CARGO_INCREMENTAL=0 cargo nextest run

env TMPDIR=/home/james/scratch/cass-tmp \
  CARGO_TARGET_DIR=/home/james/scratch/cass-targets/integrated \
  CARGO_INCREMENTAL=0 cargo clippy --all-targets -- -D warnings

cargo fmt --check

env TMPDIR=/home/james/scratch/cass-tmp \
  CARGO_TARGET_DIR=/home/james/scratch/cass-targets/integrated \
  CARGO_INCREMENTAL=0 cargo test --doc
```

Commit/deploy boundary: do not push or deploy a partially migrated phase. Push
the single integrated commit after PH-5. The existing `main` workflow then
cross-compiles and deploys; post-deploy status and local/federated search smoke
checks are operational verification, not a substitute for PH-5 evidence.

## Risks and Open Questions

- The schema migration and read/write overlap are the material risks. PH-1 owns
  transactional preservation/rollback proof; PH-3 owns snapshot consistency.
- Temporary vector duplication can be large on the Macs. Cleanup is part of the
  atomic completed switch, and the deployment smoke records database size
  before and after completion.
- A real cold backfill is not placed in ordinary tests. After the integrated
  binary deploys, run one interrupted production-shaped dev-macbook index and
  report time-to-first-checkpoint, vectors/second, partial-search latency,
  resume time, total time, and warm-refresh time.
- The real-model integration declaration is ignored by the ordinary suite but
  is not optional evidence: PH-4 runs it explicitly against already installed
  Xenia assets, and PH-5 preserves that passing evidence during reconciliation.
- The independent-review policy would normally select two reviewers because
  this plan includes persistence, migration, concurrency, and protocol change.
  Independent agent delegation is unavailable under the current collaboration
  policy. The author performed both architecture/risk and claims/evidence/test
  critiques locally: they resolved an ambiguous completed-target state, added
  the required real-model proof, and confirmed that each of the 20 claim deltas
  has exactly one component owner. These critiques are not independent
  assurance.
- There are no unresolved product questions. Any proposed change to models,
  compatibility identity, response shape, or the partial-search boundary
  returns to the spec/design artifacts before implementation.
