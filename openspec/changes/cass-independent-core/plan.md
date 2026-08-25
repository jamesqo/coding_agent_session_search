# CASS Independent Core Delivery Plan

Status: approved
Spec: `specs/cass-independent-core/spec.md`
Evidence: fresh Veritas diff/status/report; 16 stable claims, 8 discovered Rust tests, 16 uncovered-claim findings, no drift
Delivery: phased
Verification cadence: plan-final

## Scope

Replace the application in place with a small Rust JSON CLI that indexes only
Claude Code and Codex JSONL histories, stores canonical records in Rusqlite,
searches with SQLite FTS5 plus optional semantic retrieval and reranking, and
exposes only `index`, `search`, `view`, `status`, `forget`, and `models install`.
The legacy implementation is an implementation reference only and will be
deleted. No compatibility surface is preserved.

## Current Veritas Gate State

Project-bound `vtas` CLI access succeeds. Claim and evidence locks have no
drift. The Rust producer discovers eight tests, but no evidence links exist, so
all 16 claims currently have blocking `UNCOVERED_CLAIM` findings. Native tests
remain useful implementation feedback; the final consolidation unit owns
evidence discovery, link review, approval, and terminal Veritas status/report.

## Preservation and Change Contract

| ID | Decision | Rationale | Consequence |
|---|---|---|---|
| PC-1 | Replace the legacy application in place | The approved product boundary deliberately rejects compatibility | Old commands, code, tests, assets, workflows, and documentation are deleted |
| PC-2 | Preserve only current Claude Code and Codex history readability | These are the only named consumers | Two concrete parsers; no provider trait, registry, or FAD dependency |
| PC-3 | Replace all storage with one current Rusqlite schema | SQLite is canonical and backward compatibility is a non-goal | No Frankensqlite, dual backend, migration museum, or salvage path |
| PC-4 | Replace search with FTS5 plus one concrete semantic backend | The product requires lexical fallback, semantic retrieval, fusion, and reranking | No Frankensearch, CASS-owned ANN, model registry, or daemon |
| PC-5 | Remove all human-oriented product surfaces | The stable consumer is an agent using JSON | No TUI, export, alternate encodings, aliases, or presentation framework |

## Stack and Target Structure

- `app/cli.rs`: the complete Clap command surface and JSON response boundary.
- `app/ingestion.rs`: concrete Claude Code and Codex discovery/normalization.
- `app/storage.rs`: current schema, canonical writes, FTS5, context hydration,
  deletion, and embedding persistence.
- `app/semantic.rs`: one concrete local embedding/reranking backend, exact
  cosine candidate search, RRF, and bounded reranking.
- `app/lib.rs` and `app/main.rs`: composition root and JSON error envelope.
- `app/tests/`: public CLI and retained behavior tests executed by Nextest.

Dependencies point inward from CLI to these concrete modules. Parsing, storage,
and rank fusion stay synchronous. Model acquisition may use blocking network
I/O at the command edge; no runtime or service abstraction is introduced.

## Components

### CLI and canonical storage

Outcome: six operational commands, JSON responses, stable identifiers, one
Rusqlite database, idempotent derived-state rebuild, FTS5 retrieval, bounded
context view, and complete conversation deletion. Foundation: the current
`app/` replacement. Owned claims: `cli/*`, `storage/*`,
`search/lexical-returns-distinctive-match`, `view/*`, and `status/*`.

### Supported-provider ingestion

Outcome: only representative Claude Code and Codex JSONL files are discovered
and normalized; malformed lines produce bounded diagnostics without panics.
Owned claims: `ingestion/*`. Non-goal: a provider abstraction or format
compatibility beyond observed current histories.

### Semantic search and models

Outcome: explicit model installation, truthful lexical fallback, exact cosine
semantic candidates, RRF fusion, and bounded cross-encoder reranking using one
portable non-Dickles backend. Owned claims: `models/*` and `semantic/*`.
Selection is conditional on a Linux amd64 and macOS arm64 load-and-infer spike.

### Repository consolidation

Outcome: only the replacement application and focused tests remain; prohibited
dependencies/providers and removed product surfaces are absent; Rust size
ceilings pass. Owned claim: `independence/no-dickles-franken-surface` plus the
specification's size boundary.

## Design Justification

DJ-1: exact cosine search is selected because the approved corpus has no
measured need for ANN and the requirement explicitly rejects speculative index
machinery. Its cost is linear embedding scans; retirement condition is a
representative benchmark demonstrating unacceptable latency.

DJ-2: lexical fallback remains a first-class path because models are explicitly
optional and may be absent. The added cost is realized-mode metadata and two
candidate paths. It retires only if the semantic requirement changes.

DJ-3: plan-final verification is used because the replacement is already
isolated under `app/` and broad legacy gates are being deleted. Intermediate
units still run focused tests; compile errors, data-loss failures, and executed
red checks are never deferred.

## Delivery Plan

Delivery edges: PH-1 -> PH-2 -> PH-3 -> PH-4 -> PH-5. The order is deliberate:
semantic retrieval consumes canonical messages, deletion follows replacement
behavior, and final proof consumes the consolidated repository. Initial ready
set: PH-1. There are no safe parallel write groups because the crate manifest,
CLI contract, and shared schema are common ownership seams.

- [x] **PH-1 — Establish independent lexical core.** Consumes the approved
  contract. Produces the `app/` crate composition, JSON CLI, Rusqlite schema,
  FTS5 search, status, view, forget, and focused CLI/storage tests. Owns
  `Cargo.toml`, `Cargo.lock`, `app/lib.rs`, `app/main.rs`, `app/cli.rs`, and
  `app/storage.rs`. Exit: Nextest, strict Clippy, formatting, and doctests pass
  for the replacement crate.
- [x] **PH-2 — Harden supported-provider ingestion.** Depends on PH-1. Consumes
  canonical storage and produces representative Claude Code/Codex fixtures,
  stable normalization, unsupported-provider exclusion, malformed-record
  diagnostics, and idempotent reindex coverage. Owns `app/ingestion.rs` and
  ingestion-focused tests. Exit: focused Nextest scenarios for every
  `ingestion/*` claim pass.
- [ ] **PH-3 — Add semantic retrieval and explicit models.** Depends on PH-2.
  Consumes canonical messages and lexical results. Produces the portability
  spike record, one selected backend, model install/status state, embeddings,
  exact cosine search, RRF, bounded reranking, and truthful fallback metadata.
  Owns `app/semantic.rs`, semantic schema additions, semantic CLI wiring,
  model assets metadata, and semantic tests. Exit: lexical fallback tests pass;
  both target architectures load and infer with the selected backend; hybrid
  integration tests report and exercise fusion plus reranking.
- [ ] **PH-4 — Delete the legacy product and dependency surface.** Depends on
  PH-3. Consumes the behavior-complete replacement. Produces a consolidated
  manifest and repository containing no legacy application, removed provider,
  Dickles/Franken dependency, TUI, export, analytics, sync, daemon, benchmark,
  fuzz, script, workflow, asset, or stale documentation surface. Owns all
  legacy paths outside the retained replacement and OpenSpec/Veritas contract.
  Exit: dependency/provider scans are clean and Rust LOC ceilings pass.
- [ ] **PH-5 — Integrated proof and consolidation.** Depends on PH-4. Consumes
  the complete replacement and produces final formatting, strict Clippy,
  full Nextest, doctest, dependency scan, LOC accounting, evidence discovery,
  per-link semantic review, authorized approvals, and terminal Veritas
  status/report. Owns final test/evidence adjustments and removal of temporary
  spike residue. Exit: every retained behavior passes, every claim has reviewed
  evidence or an explicit specification-owned exclusion, and no blocking
  Veritas finding remains.

### PH-2 Execution Contract: Harden Supported-Provider Ingestion

Status: implemented
Depends on: PH-1 (implemented)
Consumes: current Rusqlite conversation replacement and JSON CLI index response
Produces: provider-boundary, format, malformed-record, and reindex proof for PH-3
Owns: `app/ingestion.rs`, ingestion scenarios in `app/tests/cli_contract.rs`
Concurrent siblings: none
Verification cadence: plan-final
Verification role: intermediate

#### Contract

- **Outcome:** current Claude Code and Codex JSONL histories index with stable
  canonical IDs, while unsupported roots are ignored and malformed records are
  counted without terminating valid-file ingestion.
- **Existing foundation:** concrete parsers, non-following recursive discovery,
  stable BLAKE3 message IDs, transactional conversation replacement, and the
  end-to-end index/search test remain.
- **Net-new work:** black-box unsupported-provider and malformed-record cases;
  provider-specific current-format edge coverage; idempotent repeated-index
  assertions; narrowly scoped parser fixes exposed by those tests.
- **Not included:** embeddings, semantic models, ranking, legacy-tree deletion,
  migration/salvage, watch mode, and any provider abstraction.
- **Claims and findings:** all `ingestion/*` claims and their current
  `UNCOVERED_CLAIM` findings.
- **Constraints:** PC-2 and PC-3; only Claude Code and Codex, concrete parsers,
  one current schema, external data never panics.

#### Execution

1. Add CLI-level fixtures proving unsupported JSONL is not discovered, a
   malformed line is reported while valid sibling records index, and running
   index twice replaces rather than duplicates canonical messages. Run the
   focused Nextest test and preserve any sound product-red expectation.
2. Add unit cases for meaningful observed Claude/Codex content shapes and
   deterministic IDs, then make the smallest parser changes needed for green.
3. Run formatting, strict Clippy for all targets, the complete current Nextest
   suite, and doctests. Discovery/status/report are refreshed, while broad
   evidence linking and approval remain assigned to PH-5 under plan-final.

#### Proof

- Targeted: `cargo nextest run -E 'test(/ingestion|unsupported|malformed|reindex/)'`
- Full enabled tests: deferred to PH-5 after the complete current Nextest suite
  runs as an intermediate regression check.
- Evidence: Rust producer discovery must retain or increase the current eight
  declarations; `ingestion/*` links remain pending PH-5 semantic review.
- Coverage exclusions: none.
- Veritas: refresh discovery/status/report; do not approve links in this phase.

#### Exit

- [x] Outcome demonstrated and assigned checks pass for cadence.
- [x] Runnable evidence freshly discovered; required links remain explicitly
  assigned to PH-5 review/approval.
- [x] No phase-owned coverage exclusion exists.
- [x] PH-2 outputs are available to PH-3 and remaining work retains an owner.

#### Completion record

Plan-base revision: `3c6d52d4`. Before PH-2 edits, eight Nextest tests and one
doctest passed; formatting was clean. The worktree also contains pre-existing
Gas City and roadmap changes outside PH-2 ownership, which remain untouched.

Implemented outcome: modern Codex custom tool call/output records now share the
same concrete normalization path as function calls. CLI-level proof establishes
that unsupported-provider records are ignored, malformed lines are counted
without losing valid records, and repeated full indexing does not duplicate
canonical rows. `cargo nextest run` passed 11/11 tests; strict all-target Clippy,
formatting, and the library doctest passed. Veritas discovery added three
evidence declarations and retained eight, with no artifact drift. All claim
links and approval remain assigned to PH-5; no exclusion or deviation was
introduced. PH-3 is now ready.

### PH-3 Execution Contract: Semantic Retrieval and Explicit Models

Status: approved
Depends on: PH-2 (implemented)
Consumes: canonical message rows, FTS5 candidates, and the JSON command boundary
Produces: model assets/readiness, message embeddings, exact semantic search,
RRF fusion, and bounded cross-encoder reranking for PH-4
Owns: `Cargo.toml`, `Cargo.lock`, `app/semantic.rs`, semantic additions to
`app/storage.rs`, `app/cli.rs`, `app/lib.rs`, and semantic CLI tests
Concurrent siblings: none
Verification cadence: plan-final
Verification role: intermediate

#### Contract

- **Outcome:** `models install` explicitly acquires and validates one embedding
  and one reranking model; indexing embeds messages when those validated assets
  exist; search otherwise remains lexical and truthful, or realizes hybrid
  retrieval with exact cosine candidates, RRF, and bounded reranking.
- **Existing foundation:** current message IDs, canonical rows, FTS5 search,
  status model fields, and lexical fallback response shape remain.
- **Net-new work:** `fastembed` 6.0.1 with minimal Rustls/ONNX features,
  `AllMiniLML6V2Q` embeddings, `JINARerankerV1TurboEn`, an installation marker,
  compact `f32` embedding blobs, pure rank/cosine/RRF helpers, and bounded model
  inference at index/search boundaries.
- **Not included:** model registry, daemon/IPC, ANN/HNSW, background download,
  multiple quality tiers, GPU-specific providers, or legacy semantic assets.
- **Claims and findings:** `models/download-is-explicit`,
  `semantic/missing-models-fall-back`, and `semantic/hybrid-reranks-with-models`
  plus their current `UNCOVERED_CLAIM` findings.
- **Constraints:** PC-3 and PC-4; no implicit model acquisition, one concrete
  backend, synchronous storage/search, derived embeddings rebuildable from
  canonical messages.

#### Execution

1. Add pure unit tests for cosine bounds, deterministic RRF fusion, embedding
   blob round trips, and model-marker readiness. Add CLI fallback tests proving
   index/search/status never acquire absent assets.
2. Add the minimal `fastembed` dependency and compile/load/infer smoke on Linux
   amd64 and macOS arm64. A failure on either target blocks backend selection;
   do not add a second backend.
3. Implement explicit installation into the CASS model directory and write the
   readiness marker only after both model sessions execute a smoke inference.
4. Persist one embedding per message, exact-scan semantic candidates, fuse
   lexical/semantic ranks with RRF, rerank a bounded candidate set, and expose
   realized/fallback mode truthfully. Add model-present integration coverage
   without downloading during ordinary test discovery.
5. Run focused semantic tests, full current Nextest, formatting, strict Clippy,
   and doctests. Refresh Veritas discovery/status/report; PH-5 retains link
   review, approval, and final accumulated proof.

#### Proof

- Targeted: `cargo nextest run -E 'test(/semantic|model|fallback|rrf|cosine/)'`
- Portability: load both selected models and run one embedding plus rerank on
  Linux amd64 and macOS arm64.
- Full enabled tests: deferred to PH-5 after the complete current suite runs as
  an intermediate regression check.
- Evidence: new semantic/model declarations discovered; claim links remain
  pending PH-5 individual semantic review.
- Coverage exclusions: none.
- Veritas: refresh discovery/status/report; do not approve links in this phase.

#### Exit

- [ ] Outcome demonstrated and assigned checks pass for cadence.
- [ ] Runnable evidence freshly discovered; required links remain explicitly
  assigned to PH-5 review/approval.
- [ ] No phase-owned coverage exclusion exists.
- [ ] PH-3 outputs are available to PH-4 and remaining work retains an owner.

#### Completion record

Pending. The selected backend is `fastembed` 6.0.1 using ONNX Runtime on CPU;
the selection remains conditional on the two-target inference smoke.

## Traceability and Evidence Assignment

- PH-1 owns CLI, storage, lexical, context-view, and status evidence.
- PH-2 owns every ingestion claim and fixture.
- PH-3 owns explicit model acquisition, fallback, and hybrid/reranking evidence.
- PH-4 owns independence scanning and LOC accounting.
- PH-5 owns producer discovery, individual semantic link review, approval, and
  the final authoritative status/report refresh.

No claim is excluded. Requirement prose and scenarios are documentation;
Rust unit/integration tests are runnable evidence. Passing Nextest runs do not
by themselves create Veritas evidence or approval.

## Risks and Open Questions

- Real provider formats may contain additional current record shapes; PH-2 must
  test representative local histories before declaring its boundary complete.
- The semantic backend remains intentionally undecided until the portability
  spike; selecting it is implementation work within the already-approved
  concrete-backend decision, not permission to add a registry or daemon.
- Veritas macro packaging is under upstream review. Native implementation may
  continue, but PH-5 cannot close until citations can be linked and approved.
- Legacy deletion is destructive but explicitly authorized by PC-1; git history
  remains the recovery mechanism.
