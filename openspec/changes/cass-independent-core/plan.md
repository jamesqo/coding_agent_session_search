# CASS Independent Core Delivery Plan

Status: complete

## Scope

Evolve the miniature CASS implementation in place. Preserve its six JSON-only
commands, six concrete provider paths, Rusqlite/FTS5 canonical store, exact
semantic retrieval, RRF, reranking, and explicit model installation. Add only
the feature/build split, incremental indexing lifecycle, durable forgetting,
complete-scan purging, single-writer behavior, schema migrations, and release
delivery specified by this change.

## Current Veritas Gate State

Project-bound `vtas` CLI access succeeds. The fresh status before this revision
reported three blocking missing-approval findings and a drifted `rust-tests`
provider. Revised specification claims remain provisional until wording review,
claim reconciliation, evidence refresh, and approval complete. Veritas remains
the authoritative completion gate; this plan does not reproduce its locks or
approval records.

## Preservation and Change Contract

- Preserve the operational command and JSON response surface.
- Preserve the six-provider boundary and concrete parser organization.
- Preserve Rusqlite as the only SQLite library and FTS5 as mandatory retrieval.
- Preserve explicit-only model acquisition and semantic failure-open-to-lexical
  behavior.
- Evolve the current schema forward; do not add legacy salvage or a second
  backend.
- Replace full reindex/re-embedding behavior with fingerprinted reconciliation.
- Replace forget-as-row-deletion with deletion plus a durable tombstone.

## Stack and Target Structure

- `Cargo.toml`: one default-enabled `semantic` feature and thin release LTO.
- `app/storage.rs`: schema versioning, migrations, writer transaction,
  fingerprints, tombstones, source reconciliation, and incremental mutations.
- `app/ingestion.rs`: complete/incomplete provider scan outcomes and stable
  source/message identities.
- `app/semantic.rs`: feature gate, bounded embedding batches, changed-message
  selection, and lexical fallback on model/inference failure.
- `app/cli.rs`: structured busy/incompatibility/fallback status and incremental
  index counts.
- `app/tests/`: default and lexical-only behavior, migration, concurrency,
  tombstone, purge, and incremental embedding coverage.
- `.github/workflows/`: full prebuilt release binaries and both feature gates.

## Components

### C1 — Build and semantic realization

Owns the single feature boundary, truthful lexical-only behavior, thin LTO, and
default semantic release composition. It does not add another backend or CLI
surface.

### C2 — Canonical storage lifecycle

Owns `user_version`, forward migrations, immediate single-writer acquisition,
source/message fingerprints, tombstones, and transactional deletion. It is the
foundation for incremental indexing and semantic refresh.

### C3 — Incremental provider reconciliation

Owns unchanged-source skipping, changed-message upserts, removed-message
deletion, complete-scan-only source purging, and incomplete-scan preservation.
It consumes C2 and does not abstract the concrete provider implementations.

### C4 — Incremental semantic enrichment

Owns bounded batches for added/changed messages, exact vector replacement, and
invocation-level lexical fallback. It consumes C1 and C3.

### C5 — Distribution and proof

Owns CI release binaries, both feature test gates, migration/concurrency/index
integration tests, documentation alignment, size/dependency scans, and final
Veritas evidence refresh.

## Design Justification

- SQLite's writer transaction is crash-released and already coordinates every
  process touching canonical state; a separate stale-prone lock file would add
  machinery without strengthening the current contract.
- Stable provider/session identity makes tombstones survive source content
  changes. Message content fingerprints minimize embeddings without requiring
  an ANN or cache subsystem.
- Purging only after a complete provider scan distinguishes real disappearance
  from a temporarily unavailable root or malformed source.
- A single default-enabled feature provides the requested fast development
  build without creating a combinatorial feature matrix.

## Delivery Plan

- [x] **PH-1 — Reconcile and approve the revised contract.** Review provisional
  claim wording and boundaries, refresh Rust evidence, resolve the three current
  approval findings, and lock the accepted claim set. Produces an implementation-
  ready Veritas gate. Depends on no implementation phase.
- [x] **PH-2 — Establish build and schema foundations.** Add the semantic Cargo
  feature, lexical-only composition, thin LTO, `user_version` migrations, and
  immediate single-writer behavior with focused tests. Depends on PH-1.
- [x] **PH-3 — Implement durable incremental canonical indexing.** Add source
  and message fingerprints, tombstones, changed-message mutation, removed-
  message cleanup, and complete-scan-only source purging with provider fixtures.
  Depends on PH-2.
- [x] **PH-4 — Implement incremental semantic enrichment and fallback.** Embed
  only added/changed messages in bounded batches, replace stale vectors, and
  preserve truthful lexical results across disabled, missing, load-failed, and
  inference-failed semantic states. Depends on PH-2 and PH-3.
- [x] **PH-5 — Integrate distribution and behavioral proof.** Publish full
  semantic release binaries in CI; run default and no-default feature suites,
  migration/concurrency/tombstone/purge scenarios, and real-model coverage when
  explicitly configured. Depends on PH-4.
- [x] **PH-6 — Final consolidation.** Remove superseded full-rebuild paths and
  stale documentation, verify dependency and LOC boundaries, run fmt, strict
  Clippy, Nextest, doctests, and refresh Veritas evidence and approval state.
  Depends on PH-5.

Initial ready set after claim approval: PH-2. No implementation phases are safe
to parallelize in this compact storage/search seam; PH-5's independent feature
gates may execute concurrently once PH-4 completes.

## Traceability and Evidence Assignment

- PH-2 owns distribution feature claims, schema migration/rejection, and
  concurrent-writer behavior through Cargo checks and storage integration tests.
- PH-3 owns unchanged/changed indexing, complete/incomplete scan behavior, and
  persistent forget through deterministic provider/storage integration tests.
- PH-4 owns installed/missing/failing semantic realization through fake-model
  unit seams plus the explicitly configured real-model integration test.
- PH-5 owns official binary composition and end-to-end default/lexical-only CLI
  evidence.
- PH-6 owns independence, size ceilings, final Rust evidence discovery, and
  Veritas status/report review. No claim is excluded as non-falsifiable.

## Risks and Open Questions

- The writer transaction may be held during model inference. Measure before
  introducing a second coordination mechanism or moving inference outside the
  transaction.
- Some provider formats lack stable message event IDs. Use the narrowest stable
  provider identity available and a deterministic ordinal fallback; fixtures
  must cover insertions, edits, and removals.

## Completion Record

- Restored all six declared provider paths after identifying that the stale
  two-provider retarget had landed after the OpenCode, Copilot, Hermes, and Pi
  additions.
- Added the default-enabled semantic feature, lexical-only build, thin LTO,
  default-feature release workflow, and both CI feature gates.
- Added schema version 3 migrations, immediate writer serialization, source and
  message fingerprints, incremental mutation, durable tombstones, complete-scan
  purging, bounded embeddings, and lexical fallback with failure diagnostics.
- Verified 30 default-feature tests, 26 lexical-only tests, strict Clippy in
  both realizations, formatting, doctests, a locked release build, strict
  OpenSpec validation, and a zero-finding Veritas report covering 27 claims.
