# CASS Independent Core Delivery Plan

Status: implemented
Spec: `specs/cass-independent-core/spec.md`
Evidence: 16 stable claims, 23 discovered Rust tests, 34 reviewed and approved links, zero findings or drift
Delivery: phased
Verification cadence: plan-final

## Scope

Replace the application in place with a small Rust JSON CLI that indexes
Claude Code, Codex, and Pi JSONL histories, current OpenCode and Hermes SQLite
histories, and GitHub Copilot CLI JSONL event logs; stores canonical records in Rusqlite;
searches with SQLite FTS5 plus optional semantic retrieval and reranking; and
exposes only `index`, `search`, `view`, `status`, `forget`, and `models install`.
The legacy implementation is an implementation reference only and will be
deleted. No compatibility surface is preserved.

## Current Veritas Gate State

Project-bound `vtas` CLI access succeeds. Claim and evidence locks have no
drift. The Rust producer discovers 23 focused tests; all 34 claim/evidence
links were individually reviewed and approved. All 16 claims are covered with
zero blocking or advisory findings.

## Preservation and Change Contract

| ID | Decision | Rationale | Consequence |
|---|---|---|---|
| PC-1 | Replace the legacy application in place | The approved product boundary deliberately rejects compatibility | Old commands, code, tests, assets, workflows, and documentation are deleted |
| PC-2 | Preserve current Claude Code, Codex, OpenCode, GitHub Copilot CLI, Hermes Agent, and Pi history readability | These are the only named consumers | Six concrete ingestion paths; no provider trait, registry, or FAD dependency |
| PC-3 | Replace all storage with one current Rusqlite schema | SQLite is canonical and backward compatibility is a non-goal | No Frankensqlite, dual backend, migration museum, or salvage path |
| PC-4 | Replace search with FTS5 plus one concrete semantic backend | The product requires lexical fallback, semantic retrieval, fusion, and reranking | No Frankensearch, CASS-owned ANN, model registry, or daemon |
| PC-5 | Remove all human-oriented product surfaces | The stable consumer is an agent using JSON | No TUI, export, alternate encodings, aliases, or presentation framework |

## Stack and Target Structure

- `app/cli.rs`: the complete Clap command surface and JSON response boundary.
- `app/ingestion.rs`: concrete Claude Code, Codex, OpenCode, GitHub Copilot
  CLI, Hermes Agent, and Pi discovery/normalization.
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

Outcome: representative Claude Code/Codex/Pi JSONL, current OpenCode/Hermes
SQLite, and GitHub Copilot CLI JSONL histories are discovered and normalized;
malformed records produce bounded diagnostics without panics.
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

Delivery edges: PH-1 -> PH-2 -> PH-3 -> PH-4 -> PH-5 -> PH-6. The order is
deliberate: semantic retrieval consumes canonical messages, deletion follows
replacement behavior, requested provider expansion modifies the consolidated
core, and final proof consumes the result. Initial ready set: PH-1. There are
no safe parallel write groups because the crate manifest, CLI contract, and
shared schema are common ownership seams.

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
- [x] **PH-3 — Add semantic retrieval and explicit models.** Depends on PH-2.
  Consumes canonical messages and lexical results. Produces the portability
  spike record, one selected backend, model install/status state, embeddings,
  exact cosine search, RRF, bounded reranking, and truthful fallback metadata.
  Owns `app/semantic.rs`, semantic schema additions, semantic CLI wiring,
  model assets metadata, and semantic tests. Exit: lexical fallback tests pass;
  both target architectures load and infer with the selected backend; hybrid
  integration tests report and exercise fusion plus reranking.
- [x] **PH-4 — Delete the legacy product and dependency surface.** Depends on
  PH-3. Consumes the behavior-complete replacement. Produces a consolidated
  manifest and repository containing no legacy application, removed provider,
  Dickles/Franken dependency, TUI, export, analytics, sync, daemon, benchmark,
  fuzz, script, workflow, asset, or stale documentation surface. Owns all
  legacy paths outside the retained replacement and OpenSpec/Veritas contract.
  Exit: dependency/provider scans are clean and Rust LOC ceilings pass.
- [x] **PH-5 — Add OpenCode, GitHub Copilot CLI, Hermes, and Pi ingestion.** Depends
  on PH-4. Consumes the consolidated core and produces four concrete ingestion
  paths, CLI root configuration, canonical storage support, and representative
  current-format tests without a provider abstraction or compatibility layer.
  Owns `app/ingestion.rs`, provider additions in `app/cli.rs` and
  `app/storage.rs`, focused fixtures/tests, and matching documentation. Exit:
  all four providers index, search, view, filter, reindex, and fail malformed input
  safely through focused Nextest scenarios.
- [x] **PH-6 — Integrated proof and consolidation.** Depends on PH-5. Consumes
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
- **Constraints:** the then-current PC-2 boundary covered Claude Code and Codex;
  concrete parsers, one current schema, and external data never panics. PH-5
  owns the subsequently approved provider expansion.

#### Execution

1. Add CLI-level fixtures proving unsupported JSONL is not discovered, a
   malformed line is reported while valid sibling records index, and running
   index twice replaces rather than duplicates canonical messages. Run the
   focused Nextest test and preserve any sound product-red expectation.
2. Add unit cases for meaningful observed Claude/Codex content shapes and
   deterministic IDs, then make the smallest parser changes needed for green.
3. Run formatting, strict Clippy for all targets, the complete current Nextest
   suite, and doctests. Discovery/status/report are refreshed, while broad
   evidence linking and approval remain assigned to PH-6 under plan-final.

#### Proof

- Targeted: `cargo nextest run -E 'test(/ingestion|unsupported|malformed|reindex/)'`
- Full enabled tests: deferred to PH-6 after the complete current Nextest suite
  runs as an intermediate regression check.
- Evidence: Rust producer discovery must retain or increase the current eight
  declarations; `ingestion/*` links remain pending PH-6 semantic review.
- Coverage exclusions: none.
- Veritas: refresh discovery/status/report; do not approve links in this phase.

#### Exit

- [x] Outcome demonstrated and assigned checks pass for cadence.
- [x] Runnable evidence freshly discovered; required links remain explicitly
  assigned to PH-6 review/approval.
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
links and approval remain assigned to PH-6; no exclusion or deviation was
introduced. PH-3 is now ready.

### PH-3 Execution Contract: Semantic Retrieval and Explicit Models

Status: implemented
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
   and doctests. Refresh Veritas discovery/status/report; PH-6 retains link
   review, approval, and final accumulated proof.

#### Proof

- Targeted: `cargo nextest run -E 'test(/semantic|model|fallback|rrf|cosine/)'`
- Portability: load both selected models and run one embedding plus rerank on
  Linux amd64 and macOS arm64.
- Full enabled tests: deferred to PH-6 after the complete current suite runs as
  an intermediate regression check.
- Evidence: new semantic/model declarations discovered; claim links remain
  pending PH-6 individual semantic review.
- Coverage exclusions: none.
- Veritas: refresh discovery/status/report; do not approve links in this phase.

#### Exit

- [x] Outcome demonstrated and assigned checks pass for cadence.
- [x] Runnable evidence freshly discovered; required links remain explicitly
  assigned to PH-6 review/approval.
- [x] No phase-owned coverage exclusion exists.
- [x] PH-3 outputs are available to PH-4 and remaining work retains an owner.

#### Completion record

Implemented outcome: `fastembed` 6.0.1 on CPU is validated on Linux amd64 and
macOS arm64. On both targets, explicit installation loaded quantized MiniLM and
Jina turbo, produced a 384-dimensional embedding, reranked a smoke pair, and
wrote a 32-file validated asset marker. The installed-model integration test
passed on both targets. CASS now persists derived `f32` embeddings in Rusqlite,
performs exact cosine retrieval, deterministic RRF, and bounded cross-encoder
reranking, while absent/invalid markers remain lexical without creating model
directories. Recency filtering now reaches both lexical and semantic paths.

On Xenia, the model-free Nextest suite passed 16 tests with the model test
skipped; the explicitly enabled real-model test passed separately. Strict
all-target Clippy, formatting, and doctests passed. Veritas discovery now sees
18 evidence declarations with no artifact drift. The 16 uncovered-claim
findings remain assigned to PH-6 link review and approval. No exclusion was
introduced. PH-4 is now ready.

### PH-4 Execution Contract: Delete the Legacy Product

Status: implemented
Depends on: PH-3 (implemented)
Consumes: behavior-complete independent crate and current OpenSpec/Veritas proof
Produces: a small maintained repository with no legacy product or prohibited stack
Owns: every tracked path outside the retained set below
Concurrent siblings: none
Verification cadence: plan-final
Verification role: intermediate

#### Contract

- **Outcome:** the maintained repository consists of the independent `app/`
  crate, focused tests, minimal operator/developer documentation, minimal CI,
  license/toolchain configuration, and the active OpenSpec/Veritas contract.
- **Existing foundation:** `Cargo.toml`, `Cargo.lock`, `app/`, `LICENSE`,
  `rust-toolchain.toml`, `.cargo/config.toml`, the active
  `openspec/changes/cass-independent-core` artifacts, `veritas.toml`, and
  `.veritas` locks remain.
- **Net-new work:** rewrite `README.md`, `AGENTS.md`, `.gitignore`, Nextest
  configuration, and `.github/workflows/ci.yml` to describe and test only the
  replacement.
- **Removed:** legacy `src/`, `tests/`, `benches/`, `fuzz/`, `scripts/`, docs,
  web/assets, packaging/install/release machinery, build script, RCH, UBS,
  ACFS, Beads/Gas City state, stale workflows, old skills/configuration, and
  every removed-provider or product-surface artifact.
- **Not included:** semantic behavior changes, release publishing, migration or
  compatibility work, evidence-link approval, or OpenSpec archival.
- **Claims and findings:** `independence/no-dickles-franken-surface` and final
  size accounting; its current uncovered finding remains assigned to PH-6.
- **Constraints:** PC-1 and PC-5; deletions are authorized, explicit path lists
  are reviewed before execution, and git history is the recovery mechanism.

#### Execution

1. Rewrite retained README/developer/CI/ignore/Nextest surfaces so no stale
   command, dependency, provider, or workflow remains.
2. Review tracked path groups, then delete the legacy product, tests, docs,
   assets, packaging, scripts, workflows, and Beads/Dickles development
   machinery in explicit top-level groups. Preserve unrelated untracked
   machine-managed agent skill directories.
3. Scan manifests, lockfile, source, tests, workflows, docs, and build surfaces
   for prohibited Dickles/Franken names and removed provider/product terms.
   Count production/test Rust and tracked files.
4. Run model-free Nextest, the explicitly enabled installed-model test,
   formatting, strict Clippy, doctests, and minimal CI syntax inspection.
   Refresh Veritas discovery/status/report; PH-6 retains link review and final
   accumulated proof.

#### Proof

- Targeted: prohibited-name/provider scans and tracked-file/LOC accounting.
- Native: full current Nextest plus the explicit installed-model test, strict
  Clippy, formatting, and doctests.
- Evidence: independence scan declaration added if Veritas supports it;
  otherwise the runnable repository test owns the claim.
- Coverage exclusions: none.
- Veritas: refresh discovery/status/report; do not approve links in this phase.

#### Exit

- [x] Outcome demonstrated and assigned checks pass for cadence.
- [x] Runnable evidence freshly discovered; required links remain explicitly
  assigned to PH-6 review/approval.
- [x] No phase-owned coverage exclusion exists.
- [x] PH-4 outputs are available to PH-5 and remaining work retains an owner.

#### Completion record

Implemented outcome: 3,685 tracked legacy files were removed from the product
and development surfaces. Locally modified `.beads` state was removed from the
Git index but preserved on Xenia and ignored, preventing data loss while future
clones remain Beads-free. The repository now tracks 35 files and contains 2,043
Rust lines under `app/`, including focused tests. README, AGENTS, Nextest, ignore
rules, and CI now describe only the replacement.

The maintained manifest, lockfile, source, tests, workflow, documentation, and
Cargo/Nextest/toolchain configuration are clean for the prohibited
Dickles/Franken, RCH, UBS, and ACFS surfaces. A runnable repository regression
protects this boundary. On Xenia, 18 model-free Nextest tests and the separately
enabled real-model test passed; strict all-target Clippy, formatting, doctests,
diff hygiene, file accounting, LOC accounting, and scans passed. Veritas now
discovers 19 focused evidence declarations with no artifact drift. The 16
uncovered claims remain assigned to PH-6 link review and approval. No exclusion
was introduced. PH-5 is now ready.

### PH-5 Execution Contract: OpenCode, GitHub Copilot CLI, Hermes, and Pi Ingestion

Status: implemented
Depends on: PH-4 (implemented)
Consumes: consolidated core, canonical schema, and concrete
Claude Code/Codex ingestion
Produces: current OpenCode, GitHub Copilot CLI, Hermes, and Pi discovery,
normalization, storage, filtering, and focused proof for PH-6
Owns: `app/ingestion.rs`, provider/root additions in `app/cli.rs` and
`app/storage.rs`, `app/tests/cli_contract.rs`, `README.md`, and `AGENTS.md`
Concurrent siblings: none
Verification cadence: plan-final
Verification role: intermediate

#### Contract

- **Outcome:** current OpenCode sessions from `opencode.db`, GitHub Copilot CLI
  sessions from `session-state/<id>/events.jsonl`, and Hermes Agent sessions
  from `state.db`, plus Pi sessions from `~/.pi/agent/sessions/**/*.jsonl`,
  index into the same canonical records and work through
  search, provider filters, view, and idempotent reindexing.
- **Existing foundation:** concrete ingestion functions, stable BLAKE3 IDs,
  transactional conversation replacement, one Rusqlite schema, and the JSON
  index/search/view surface remain.
- **Net-new work:** four explicit roots, read-only OpenCode and Hermes database
  parsers, Copilot CLI and Pi event parsers, six-provider storage constraint,
  representative fixtures, malformed-record accounting, and concise documentation.
- **Not included:** provider traits or registries, FAD code, legacy OpenCode or
  Hermes file storage, VS Code Copilot Chat workspace storage, Copilot cloud
  history, Pi SQLite/native stores, Oh My Pi, migrations, remote discovery, or
  compatibility heuristics.
- **Claims and findings:** existing `ingestion/*` claims and their uncovered
  findings; the claim IDs remain stable because the provider requirement is
  expanded rather than replaced.
- **Constraints:** PC-2 and PC-3; external provider databases are opened
  read-only, a malformed row/event cannot panic, and only current documented
  storage surfaces are accepted.

#### Execution

1. Add representative black-box fixtures for current OpenCode
   `session`/`message`/`part` tables, Copilot CLI `user.message` plus
   `assistant.message` events, Hermes `sessions`/`messages` tables, and Pi v3
   `session` plus nested `message` JSONL entries.
   Establish red tests for index/search/filter/view and idempotent reindexing.
2. Add explicit CLI roots and concrete parser functions. Open OpenCode and
   Hermes with Rusqlite read-only flags and normalize textual content; parse
   Copilot and Pi events line-by-line and ignore non-conversation entries.
3. Extend the canonical provider constraint and error accounting without a
   schema-version or provider abstraction. Update minimal user/developer docs.
4. Run focused provider Nextest, the model-free suite, formatting, strict
   Clippy, and doctests. Refresh Veritas discovery/status/report; PH-6 retains
   link review, approval, fresh-clone proof, and final accumulated gates.

#### Proof

- Targeted: `cargo nextest run -E 'test(/opencode|copilot|hermes|pi/)'`.
- Native: complete current model-free Nextest, strict Clippy, formatting, and
  doctests as an intermediate regression check.
- Evidence: provider scenarios are compiler-discoverable Rust tests;
  `ingestion/*` links remain pending PH-6 individual semantic review.
- Coverage exclusions: legacy OpenCode/Hermes storage, VS Code Copilot Chat,
  cloud history, Pi SQLite/native stores, and Oh My Pi are scope boundaries, not exclusions from a retained
  requirement.
- Veritas: refresh discovery/status/report; do not approve links in this phase.

#### Exit

- [x] Outcome demonstrated and assigned checks pass for cadence.
- [x] Runnable evidence freshly discovered; required links remain assigned to
  PH-6 review/approval.
- [x] No phase-owned coverage exclusion exists.
- [x] PH-5 outputs are available to PH-6 and remaining work retains an owner.

#### Completion record

Implemented outcome: CASS discovers current OpenCode `opencode.db`, GitHub
Copilot CLI `session-state/<id>/events.jsonl`, and Hermes Agent `state.db`
through three concrete code paths. Both external databases open read-only.
Provider filters, stable IDs, context view, malformed JSON accounting, skipped
non-conversation records, and idempotent replacement are exercised by focused
black-box fixtures. Legacy OpenCode/Hermes storage, VS Code Copilot Chat, and
cloud history remain outside the approved boundary. It also discovers current
Pi version-3 JSONL sessions under an explicit/default sessions root, retains
user/assistant/tool-result text plus searchable thinking and tool calls, and
counts malformed lines without losing valid siblings. Pi SQLite/native stores
and Oh My Pi remain outside the boundary.

On Xenia, the complete model-free Nextest suite passed 22 tests with one
explicit-model test skipped; the installed-model hybrid test passed separately.
Strict all-target Clippy, formatting, doctests, and diff hygiene passed. The
repository remains 35 tracked files with 2,106 production Rust lines and 761
test Rust lines. Veritas refresh and final approval review remain assigned to
PH-6; no coverage exclusion was introduced. PH-6 is now ready.

### PH-6 Execution Contract: Integrated Proof and Consolidation

Status: implemented
Depends on: PH-5
Consumes: consolidated independent repository and all phase-owned tests
Produces: reviewed evidence links, terminal gates, and an archive-ready change
Owns: test citation attributes, temporary Veritas macro vendor path, evidence
and approval locks, final plan record, and any final proof-only corrections
Concurrent siblings: none
Verification cadence: plan-final
Verification role: plan-final

#### Contract

- **Outcome:** every locked claim is connected to semantically appropriate
  passing Rust evidence, final native/fresh-clone gates pass, no blocking
  Veritas finding remains, and all implemented phases become complete.
- **Existing foundation:** 23 focused evidence declarations, 16 locked claims,
  clean claim/evidence drift, the Xenia/macOS real-model caches, and the minimal
  CI workflow remain.
- **Net-new work:** vendor the exact released `veritas-test-macros` 0.1.0 crate
  as the documented fallback, add compiler-checked claim attributes, review and
  approve each link, validate a fresh clone, and record terminal results.
- **Not included:** product behavior, provider expansion, release publishing,
  migration compatibility, or Veritas implementation changes.
- **Claims and findings:** all 16 claims and their reviewed evidence links.
- **Constraints:** no claim is deleted or excluded; each link is reviewed
  individually; native test success and evidence approval remain distinct.

#### Execution

1. Vendor the unmodified released macro source from Veritas release
   `1b3a2db`, add a portable path dev-dependency, and convert existing claim
   comments to `#[veritas::claims(...)]` attributes.
2. Run focused/native tests and evidence discovery. Inspect every new link for
   trigger, bounds, assertion, and ownership; approve only exact reviewed pairs.
3. Clone the pushed repository into an isolated directory and run formatting,
   strict Clippy, Nextest, and doctests. Confirm GitHub CI if the repository has
   Actions enabled; absence of a run is reported rather than silently treated
   as green.
4. Run the accumulated Xenia model-free and installed-model gates, macOS arm64
   installed-model test if stale, prohibited scans, LOC/file accounting, and
   final Veritas diff/status/report. Remove temporary residue but retain the
   macro vendor fallback until upstream #54 is deployed.

#### Proof

- Native: fresh-clone and canonical Xenia formatting, strict Clippy, full
  Nextest, installed-model test, and doctests.
- Cross-platform: previously current Linux amd64 and macOS arm64 model
  load/inference records remain current unless product/model code changes.
- Evidence: all 16 claims linked to reviewed Rust declarations and approved
  using current refs; zero drift and zero blocking findings.
- Coverage exclusions: none.
- Veritas: discovery, per-link review, approval, final diff/status/report.

#### Exit

- [x] Outcome demonstrated and all accumulated checks pass.
- [x] Every claim has current reviewed/approved runnable evidence.
- [x] No coverage exclusion or blocking Veritas finding remains.
- [x] Repository is archive-ready and no implementation work remains.

#### Completion record

Commit `613b7fec` was pushed to `main` and synchronized to `master`, then cloned
from GitHub into an isolated directory. The fresh clone passed formatting,
strict all-target Clippy, 22 parallel model-free Nextest tests, one doctest,
and the separately enabled installed-model hybrid/reranking test. Canonical
Xenia gates passed identically before the push.

The maintained repository contains 35 tracked files, 2,106 production Rust
lines, and 761 test Rust lines. Veritas reports 16 claims, 23 evidence items,
34 reviewed/approved links, zero uncovered claims, zero drift, and zero
findings. No coverage exclusion was introduced. Cursor and Aider remain
unsupported by explicit user choice; no implementation residue was added.

## Traceability and Evidence Assignment

- PH-1 owns CLI, storage, lexical, context-view, and status evidence.
- PH-2 owns every ingestion claim and fixture.
- PH-3 owns explicit model acquisition, fallback, and hybrid/reranking evidence.
- PH-4 owns independence scanning and LOC accounting.
- PH-6 owns producer discovery, individual semantic link review, approval, and
  the final authoritative status/report refresh.

No claim is excluded. Requirement prose and scenarios are documentation;
Rust unit/integration tests are runnable evidence. Passing Nextest runs do not
by themselves create Veritas evidence or approval.

## Risks and Open Questions

- Real provider formats may contain additional current record shapes; PH-2 must
  test representative local histories before declaring its boundary complete.
- The selected FastEmbed backend has passed Linux amd64 and macOS arm64
  load/inference proof; adding a registry or daemon remains outside scope.
- The exact released Veritas macro source remains vendored as the documented
  fallback until upstream packaging is deployed.
- Legacy deletion is destructive but explicitly authorized by PC-1; git history
  remains the recovery mechanism.
