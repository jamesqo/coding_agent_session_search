# Required Semantic Search Delivery Plan

Status: approved

## Scope

Replace successful lexical-only operation with one mandatory hybrid retrieval realization; make models a prerequisite for indexing; separate full canonical context from tool-result-free search projections; make ordinary FTS and embedding maintenance proportional to changed messages; and migrate Xenia, dev-macbook, and personal-macbook to protocol version 2 with complete semantic indexes.

Preserve FTS5 lexical candidate generation, exact quantized cosine retrieval, RRF, bounded reranking, complete `view` context, explicit model installation, canonical message identity, provider support, and the existing SSH transport. Do not add another backend, runtime mode, daemon, ANN index, implicit download, role-weighting policy, or timing promise.

Delivery uses phased implementation with plan-final verification. Intermediate phases run focused proof and immediate migration-safety checks; the final phase runs the complete semantic repository, Veritas, deployment, and live-corpus gates once.

## Current Veritas Gate State

Project-bound CLI access and the native `rust-test` producer are available. Before these deltas, Veritas reported 35 covered claims, 53 approved links, current evidence, and no findings. The provisional semantic/tool-projection deltas intentionally introduce missing lock entries and wording drift until implementation evidence is complete. Specification authoring owns those provisional markers; no claim lock or approval is changed during planning.

There are no coverage exclusions. Every new product claim is falsifiable through Rust unit or CLI process evidence. Real-corpus duration measurements are operational observations, not proof substitutes.

## Preservation and Change Contract

| ID | Disposition | Contract |
|---|---|---|
| PC-1 | Preserve | Canonical messages retain complete content, stable identity, ordering, and context-view behavior. |
| PC-2 | Evolve compatibly | Canonical storage gains an internal search projection and forward migration; pre-migration backups own rollback. |
| PC-3 | Replace | Optional semantic backend plus lexical stub becomes one unconditional FastEmbed backend. |
| PC-4 | Replace | Lexical fallback branches become typed model/readiness failures with recommended actions. |
| PC-5 | Evolve compatibly | Normal index uses transaction-sized adaptive FTS maintenance and changed-only embeddings; `--full` retains whole-run rebuilding. |
| PC-6 | Replace | Federation protocol v1 becomes v2 and accepts hybrid success only; v1 nodes become partial compatibility failures. |
| PC-7 | Preserve | FTS5 remains a filtered candidate source inside hybrid search; semantic quality does not mean vector-only retrieval. |
| PC-8 | Preserve | `models install` is the only network/model-acquisition command. |

## Stack and Target Structure

```text
provider parser
  └─ NormalizedMessage
       ├─ canonical content ───────────────→ messages.content → view
       └─ optional search projection ─────→ FTS5 + embeddings

app/cli.rs
  ├─ readiness precedence + typed actions
  ├─ model-first index orchestration
  └─ hybrid-only local composition
       ├─ app/storage.rs
       ├─ app/semantic.rs (unconditional FastEmbed)
       └─ app/federation.rs (protocol v2)

app/tests/cli_contract.rs + module unit tests
  └─ rust-test discovery → Nextest execution → Veritas review/approval
```

No dependency is added. `fastembed` becomes unconditional; `semantic_disabled.rs` and feature branches are removed.

## Components

### Search projection and migration

- Outcome: complete context survives while structured tool-result payloads own no FTS row or embedding.
- Foundation: concrete provider parsers, stable normalized messages, Rusqlite migrations, rebuildable derived state.
- Net-new: nullable projection semantics, searchable-message counts, provider-aware mixed-block extraction, checkpoint invalidation, and derived cleanup.
- Non-goals: tool-call exclusion, role weighting, summaries, prefixes, or canonical-content truncation.
- Claims: `search/tool-results-are-not-searchable`, `search/mixed-message-excludes-tool-result-text`, `view/tool-results-remain-visible`, `storage/tool-search-projection-migrates`.
- Proof: parser fixtures for explicit and mixed tool results; storage migration/FTS/embedding-selection tests; CLI search-plus-view acceptance.

### Adaptive incremental derived indexing

- Outcome: ordinary refresh mutates only changed canonical and embedding rows, uses row-level FTS work for small transaction deltas, and switches to equivalent bulk FTS work at a measured cutoff; explicit full/generation rebuild remains available.
- Foundation: source checkpoints, conversation reconciliation, per-message FTS maintenance, and `messages_needing_embeddings`.
- Net-new: transaction-local FTS mutation accounting, deterministic strategy selection, pre-commit FTS finalization, explicit-full optimize maintenance, normal/full orchestration split, and affected-row assertions.
- Claims: `indexing/unchanged-source-is-skipped`, `indexing/only-changed-messages-refresh`, `indexing/canonical-and-fts-are-atomic`, `indexing/full-rebuild-is-explicit`, plus preserved purge/incomplete-scan/forget behavior. Strategy selection and row-versus-bulk equivalence are internal proofs, not Veritas claims.
- Proof: a reproducible row-versus-bulk benchmark; storage strategy, rollback, and equivalence tests; and two-pass CLI refresh fixtures. Recorded timing selects the cutoff but is not a normative duration promise.

### Mandatory semantic readiness

- Outcome: every successful index/search is model-backed and every supported binary contains the backend.
- Foundation: current FastEmbed models, exact quantized vectors, generation identity, hybrid search, and JSON errors/status.
- Net-new: model-first index precondition, exact searchable embedding coverage, actionable errors/status, removal of fallback fields and feature/stub code.
- Claims: `search/fts-contributes-to-hybrid`, semantic readiness/failure claims, model acquisition claim, distribution claim, and status claims.
- Proof: CLI process tests for action precedence/no-write failure; real-model ignored integration fixture; Cargo/CI manifest assertions; pure status/readiness tests.

### Semantic federation

- Outcome: v2 aggregate results contain only local/remote hybrid successes while unready or old nodes remain explicit partial outcomes.
- Foundation: bounded SSH runner, versioned JSON envelopes, deterministic rank merge, and remote outcomes.
- Net-new: protocol bump and successful-response mode validation.
- Claims: `federated-search/semantic-unready-node-is-partial-failure`, `federated-search/local-semantic-readiness-is-required`, `federated-search/successful-nodes-are-hybrid`.
- Proof: fake-SSH mixed readiness/version tests plus deployed three-node smoke.

## Design Justification

`search_projection` is tri-state because the current corpus contains ordinary messages, pure tool results, and mixed Claude blocks. A role flag cannot represent mixed content; storing a complete duplicate search string for every ordinary message would offset database savings. `NULL` therefore means canonical content, empty means context-only, and nonempty means filtered mixed content. This long-lived schema choice retires only if canonical messages are later normalized into independently addressable content blocks.

Protocol v2 prevents a rolling old binary from presenting lexical fallback as valid semantic success. The temporary incompatibility cost is bounded by existing partial-failure behavior and retires when all deployed nodes run v2.

## Delivery Plan

- [ ] **PH-1 — Make derived refresh adaptive and transactional.** Depends on: none. Benchmark row-level versus bulk FTS maintenance, declare the measured cutoff, queue transaction-local FTS mutations, finalize row-level or bulk FTS state before every checkpoint commit, optimize only after explicit full rebuilds, retain explicit full deferral/rebuild, and prove changed-only embedding selection. Exit: strategy-boundary, rollback, replacement, disappearance, two-pass incremental, and bulk-equivalence tests pass; the benchmark record justifies the cutoff. Owns index orchestration, storage FTS finalization, benchmark harness, and derived-maintenance tests.
- [ ] **PH-2 — Add search projections and safe schema migration.** Depends on: PH-1. Extend normalized messages and storage schema; implement Claude Code and Codex tool-result filtering, searchable counts, migration invalidation, FTS projection use, embedding selection, and canonical view preservation. Exit: focused parser/storage/migration/tool-output acceptance tests pass. Owns ingestion/storage and their tests.
- [ ] **PH-3 — Require semantic readiness and remove lexical build/fallback.** Depends on: PH-2. Add typed readiness declarations, load models before writer/discovery, enforce exact searchable embedding set coverage, propagate inference errors, add actionable error/status state, remove fallback fields, make FastEmbed unconditional, delete the stub and feature branches, and update CI. Exit: missing assets fail without database writes; ready real-model test remains explicit; all supported builds enforce semantic search. Owns Cargo, CI, CLI, semantic module, and readiness tests.
- [ ] **PH-4 — Upgrade federation to semantic protocol v2.** Depends on: PH-3. Add the protocol/readiness matrix, bump search and view envelopes, reject old/lexical successes, preserve partial errors, and require local readiness. Exit: fake v1, lexical-v2, unready-v2, ready-v2, and local-unready cases pass with deterministic outcomes. Owns federation module and its tests.
- [ ] **PH-5 — Final proof, migration, and deployment.** Depends on: PH-1, PH-2, PH-3, PH-4. Run full repository/Veritas gates; stage models; back up databases; deploy; full-index all three machines; and execute live quality/federation smoke. Exit: claims and evidence are clean, CI/deploy are green, all statuses are ready, tool-only terms are absent from search but present in view, federated nodes report hybrid, and rollback backups exist. Owns generated Veritas state through CLI, deployment state, backups, and final plan records.

Dependency edges: `PH-1 → PH-2 → PH-3 → PH-4 → PH-5`.

Initial ready set: `PH-1`. The indexing, schema, readiness, and federation phases are serialized because each consumes the prior boundary and shares central storage/CLI types. Live model installation on the three machines is parallel inside PH-5 after local proof passes.

Under plan-final cadence, apply records a green plan-base Nextest baseline. Each implementation phase writes its failing declarations immediately before production changes, then runs focused affected tests plus compile/format checks; migration data-integrity tests are immediate and may not be deferred. PH-5 runs canonical full proof once.

### PH-1 execution brief: adaptive transactional FTS

Status: approved

- **Outcome:** Every ordinary writer checkpoint commits canonical and matching FTS state together, choosing row-level or bulk FTS maintenance from one measured deterministic cutoff; tiny refreshes no longer trigger whole-corpus rebuilding.
- **Existing foundation:** Source/message fingerprints, bounded ingestion checkpoints, per-row FTS SQL, full rebuild SQL, changed-only embedding selection, and the green plan-base suite (the isolated federation process-classification flake passed its one diagnostic retry).
- **Net-new work:** Transaction-local changed-ID staging, strategy selection and observability in storage tests, pre-commit finalization, FTS-only bulk rebuilding, explicit full-mode optimization, a reproducible crossover benchmark, and CLI two-pass proof.
- **Not included:** Search projections, parser changes, schema migration, semantic readiness/fallback removal, Cargo features, federation, deployment, and live models remain PH-2 through PH-5.
- **Claims:** `indexing/unchanged-source-is-skipped`, `indexing/only-changed-messages-refresh`, `indexing/canonical-and-fts-are-atomic`, `indexing/full-rebuild-is-explicit`; strategy selection and row/bulk equivalence remain uncited internal proof.
- **Constraints:** No new dependency, async runtime, adaptive governor, persisted tuning state, or timing assertion. FTS maintenance completes before each ordinary checkpoint commit. Embeddings are not deleted or regenerated merely because FTS chooses bulk mode.

Execution:

1. Add failing storage tests for transaction staging, the exact cutoff boundary, changed content, replaced message IDs, source/conversation disappearance, and row-level and bulk rollback. Assertions inspect canonical rows, FTS terms, checkpoints, and embeddings independently. Focused proof: `cargo nextest run storage::tests`.
2. Implement a connection-local temporary changed-ID table. Reconciliation stages IDs rather than mutating FTS immediately, and the table's distinct row count is the transaction work measure. Before ordinary `checkpoint_writer` and `commit_writer`, delete/reinsert only staged IDs below the cutoff or rebuild FTS at/above it; clear staging only after successful finalization. The cutoff denominator is the larger of pre- and post-transaction corpus size so deletions do not cross early; replaced identifiers intentionally contribute both removed and inserted FTS IDs.
3. Split FTS-only rebuild from embedding invalidation. `index --full` keeps deferred whole-run rebuilding and owns explicit FTS5 `optimize`; ordinary bulk crossover must preserve current embeddings and does not optimize.
4. Add independent-database equivalence coverage across insertion, content replacement, identifier replacement, and removal, including ordered IDs and provider/day/limit filters. Add a two-pass CLI fixture proving a tiny delta does not report a full search rebuild.
5. Run a reproducible ignored benchmark against representative small and copied production-sized databases at 1, 10, 100, 1,000, 10,000, and percentage deltas. Record measurements and select one conservative ratio; run focused tests, format, and refresh Veritas evidence/status.

Benchmark record (Xenia, 2026-08-25; debug test build; three repetitions with alternating strategy order; median milliseconds):

| Corpus | Changed | Row-level | Bulk FTS | Result |
|---:|---:|---:|---:|---|
| 25,000 synthetic | 1 / 10 / 100 / 1,000 | 7.3 / 7.1 / 7.7 / 13.1 | 159.6 / 162.2 / 160.7 / 161.3 | row-level |
| 25,000 synthetic | 10,000 (40%) | 71.7 | 165.2 | row-level |
| 25,000 synthetic | 21,250 (85%) | 158.1 | 162.3 | row-level |
| 25,000 synthetic | 22,500 (90%) | 171.0 | 158.1 | bulk |
| 25,000 synthetic | 25,000 (100%) | 180.3 | 162.3 | bulk |
| 23,592 copied live | 1 / 10 / 100 / 1,000 | 19.2 / 19.1 / 24.5 / 112.1 | 2,370.8 / 2,382.4 / 2,438.3 / 2,499.1 | row-level |
| 23,592 copied live | 10,000 (42%) | 1,346.2 | 2,426.4 | row-level |
| 23,592 copied live | 20,053 (85%) | 2,279.1 | 2,487.8 | row-level |
| 23,592 copied live | 21,232 (90%) | 2,554.2 | 2,439.5 | bulk |
| 23,592 copied live | 23,592 (100%) | 2,837.9 | 2,541.7 | bulk |

Both corpus shapes crossed between 85% and 90%. PH-1 therefore selects bulk FTS maintenance at 90% of the larger pre/post-transaction canonical message count; smaller transaction deltas use row-level maintenance. This is a deterministic ratio, not runtime tuning. Explicit `index --full` remains the operator-controlled bulk path and alone runs `optimize`.

Ownership: `app/storage.rs`, `app/cli.rs` index orchestration, focused ingestion/CLI tests, and the PH-1 benchmark record. Concurrent siblings: none. Verification cadence: plan-final. Verification role: intermediate.

PH-1 exit:

- Required: strategy, mutation, rollback, disappearance, and equivalence tests pass without wall-clock assertions.
- Required: benchmark measurements and the selected cutoff are recorded; the normal suite is green.
- Required: evidence discovery is current and PH-1 claim links have been individually reviewed before approval.
- Required: PH-2 consumes stable transaction-finalization and FTS projection seams.

## Traceability and Evidence Assignment

| Claim group | Owning phase | Runnable evidence |
|---|---|---|
| Tool-result exclusion and retained view | PH-2 | provider parser units, storage projection tests, CLI search/view fixture; PH-2 owns green evidence/link review |
| Migration invalidates derived readiness | PH-2 | supported-schema migration test with canonical preservation and checkpoint invalidation |
| Adaptive changed-only and explicit full indexing | PH-1 | strategy-boundary, rollback, replacement, disappearance, equivalence, and two-pass CLI index tests plus the recorded FTS crossover benchmark; PH-1 owns green evidence/link review |
| Required models/embeddings/inference | PH-3 | typed-error CLI tests, no-write assertion, real-model integration; PH-3 owns green evidence/link review |
| FTS contribution within hybrid | PH-3 | ready hybrid result fixture retaining lexical candidate metadata |
| Status action precedence | PH-3 | table-driven missing-model/database/embedding/ready CLI cases |
| Every build includes semantic | PH-3 | Cargo manifest/CI contract test and actual deployment build |
| Semantic federation | PH-4 | fake-SSH protocol/readiness matrix; PH-4 owns green evidence/link review; live PH-5 smoke supplements it |

PH-5 runs: full Nextest; strict Clippy; rustfmt; doctests; OpenSpec strict validation; Veritas claim reconciliation, evidence discovery, link review and approvals; clean status/report; CI/deployment; three local statuses/searches; federated search and remote view. There are no `[[coverage.exclude]]` entries.

## Risks and Open Questions

- Initial semantic generation duration is unknown and likely model/token-bound. PH-5 measures it per machine and remains persistent; no semantic timing threshold is invented. PH-1's FTS strategy cutoff is separately benchmarked and recorded before selection.
- Cold FastEmbed startup may approach the existing five-second SSH deadline. The live smoke records this; an observed timeout returns to a separate deadline decision rather than silently weakening semantic readiness.
- The schema migration forces a one-time reparse because flattened canonical text cannot reconstruct mixed structured blocks. Pre-migration copies preserve tombstones and own rollback.
- Tool invocation arguments remain searchable by deliberate scope; only tool-result/output payloads are context-only. General tool-message weighting remains open for a later evidence-driven relevance change.
- No behavioral or architecture question remains open for implementation.
