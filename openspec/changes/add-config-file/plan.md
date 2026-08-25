# CASS configuration delivery plan

Status: approved

## Scope

Add one optional strict JSON configuration file for explicit local-node
identity, Claude Code/Codex roots, a default 90-day indexing horizon, and
default federation membership. Add global `--config` and `--local-node`,
index-time `--provider`, `--since-days`, and `--all-history`, configured default
search fanout, and configured remote view. Remove provider-root and federation
environment inputs in the same delivery.

Preserve the two product-boundary provider parsers, Rusqlite canonical storage,
mandatory hybrid search, protocol-v2 federation envelopes, fixed SSH deadline,
rank fusion, command set, database/model path flags, and JSON-only output. Do
not add file-size filters, hostname inference, automatic retention,
configurable retrieval internals, remote indexing, synchronization,
abstractions, registries, or dependencies.

Delivery: phased. Verification cadence: phase. Final consolidation: required.

## Current Veritas Gate State

Project-bound `vtas` CLI access and the native `rust-test` producer are
operational. The post-review baseline has 74 claims, 54 evidence declarations,
54 links, 53 approvals, 38 covered claims, 36 uncovered claims, and 97 findings.
Claim and evidence locks have no artifact drift; 12 links need semantic review,
and the 2026-08-25 pre-edit evidence refresh cleared live-provider drift.
Implementation records the affected finding identifiers. Configuration claims
are not treated as covered until new or changed links receive individual
semantic review and explicit human approval.
Pre-existing semantic-search findings remain owned by that earlier change and
cannot be counted as configuration completion.

No coverage exclusion is planned. Every new observable behavior has a concrete
Rust unit, CLI process, fake-SSH, or live deployment boundary.

## Preservation and Change Contract

| ID | Disposition | Contract |
|---|---|---|
| PC-1 | Preserve | Operational commands and JSON-only success/error output remain unchanged. |
| PC-2 | Add | One optional strict version-1 document resolves node inventory, local identity, roots, and indexing horizon before command effects. |
| PC-3 | Remove | All six current `CASS_*_ROOTS` variables and `CASS_SEARCH_NODES` no longer alter behavior; no compatibility shim remains. |
| PC-4 | Restore boundary | Only Claude Code and Codex consume explicit resolved roots; absent configuration retains their built-in roots, while OpenCode, Copilot, Hermes, and Pi discovery is removed. |
| PC-5 | Preserve | Canonical records, tombstones, checkpoints, FTS, embeddings, and provider-scoped complete-scan semantics remain authoritative. |
| PC-6 | Add safely | The default 90-day source-activity horizon bounds admission and work but never deletes previously indexed old sources. |
| PC-7 | Evolve compatibly | Federation selects logical configured names and launches their SSH destinations while preserving protocol v2, fixed timeout, partial failure, merge, and provenance. |
| PC-8 | Preserve | `--db`, `--models-dir`, model installation, semantic readiness, and storage locations remain outside node configuration. |
| PC-9 | Preserve | The implementation stays synchronous and concrete using existing dependencies; no dependency is added. |

## Stack and Target Structure

```text
app/cli.rs
  ├─ typed global/index/search/view flags
  ├─ app/config.rs
  │    ├─ strict private JSON structs
  │    ├─ semantic validation and precedence
  │    └─ immutable node/provider/horizon values
  ├─ app/ingestion.rs
  │    ├─ Claude Code and Codex only
  │    ├─ source-activity horizon
  │    └─ provider/root/age-scoped reconciliation
  └─ app/federation.rs
       ├─ logical node name → SSH destination
       └─ existing protocol-v2 transport and merge

unit tests beside config/ingestion/federation
CLI process tests in app/tests/cli_contract.rs
```

`config.rs` owns the pure resolution pipeline. CLI parses typed raw flags and
composes commands. Neither ingestion nor federation loads configuration or
reads legacy environment inputs independently.

## Components

### Configuration resolver

- Outcome: commands receive one validated, immutable local identity, node
  inventory, provider-root map, and horizon, or a typed pre-effect failure.
- Foundation: Clap globals, Serde JSON, `ProjectDirs`, typed JSON errors.
- Net-new: strict private structs, default/explicit path semantics, split
  document/node/provider validation, exact local-node resolution, precedence,
  90-day/null horizon semantics, and status metadata.
- Non-goals: hostname calls, config writing, migration, generic merging,
  secrets, database/model paths.
- Claims: PH-1 owns document/node/provider/horizon validation and local-node
  resolution; PH-2 owns public discovery, status, error shape, and CLI
  precedence; the environment-input claim completes only after PH-3.
- Proof owner: PH-1 resolver units, PH-2 CLI boundary tests, and PH-3
  environment/federation tests.

### Provider selection, horizon, and reconciliation authority

- Outcome: configured roots scan only selected concrete providers, recent
  sources are admitted by default, and restricted, incomplete, or age-filtered
  runs cannot purge state outside their authority.
- Foundation: `ProviderRoots`, two direct index calls, observed paths,
  complete-scan purge, checkpoints, and tombstones.
- Net-new: exhaustive two-provider selection, one run-start cutoff, explicit
  resolved roots, and provider/root/age-scoped completeness tests.
- Non-goals: parser registry, ad hoc root override, glob language, file-size
  filters, retention, remote indexing.
- Claims: provider boundary/configured roots, bounded CLI provider selection,
  partial-provider preservation, inaccessible-root preservation, and recency
  admission/non-retention.
- Proof owner: PH-2 parser, controlled-clock, storage-preservation, and CLI
  tests.

### Configured federation

- Outcome: stable logical names select safe unique SSH destinations for default
  or explicit hybrid fanout and remote view without recursion.
- Foundation: bounded fake-SSH runner, concurrent fanout, protocol v2, partial
  outcomes, deterministic merge, and origin-aware view.
- Net-new: resolved node value, default-enabled selection, explicit override of
  `search:false`, local-node rejection, provenance/transport separation, and a
  nonrecursive hidden worker boundary.
- Non-goals: SSH discovery, arbitrary remote command, timeout knob, retries,
  remote indexing.
- Claims: node selection/validation, configured default fanout, concurrent
  fanout, nonrecursive worker, and remote view.
- Proof owner: PH-3 pure selection, fake-process, aggregate, and CLI tests.

## Delivery Plan

- [ ] **PH-1 — Add the internal strict configuration core.** Depends on: none.
  Create `config.rs`, the configuration error constructor, bounded default and
  explicit path handling, split version/field/provider/root/node/horizon
  validation, and exact local-node resolution. Do not expose flags or status
  fields yet. Prove absent-default behavior; explicit missing/file/directory
  handling; nested unknown fields/version/providers; lexical unique absolute
  roots without filesystem probing; unique bounded names and SSH destinations;
  1/36500/90/null horizon cases; valid/invalid overrides; and no-config override
  rejection. Exit: focused units plus full Nextest, strict Clippy, rustfmt,
  doctests, strict OpenSpec validation, refreshed Veritas discovery/status, and
  no new PH-1-owned finding. Owns `app/config.rs`, its module declaration,
  configuration error construction, and unit tests.

  Because the resolver has no production consumer until PH-2, PH-1 may carry
  one module-level temporary `dead_code` allowance whose comment names PH-2 as
  its removal owner. PH-1 and PH-2 form one non-landable activation unit; PH-2
  exit requires removal of that allowance before strict Clippy.

  **PH-1 execution contract — approved.** Plan revision: current worktree at
  base `4bca8d38`. Status: in progress. Depends on: none. Consumes the existing
  `AppError`, `ProjectDirs`, and Serde. Produces a validated immutable
  configuration value for PH-2 and PH-3. Owns `app/config.rs`, its
  `app/lib.rs` module declaration and configuration error constructor, and
  colocated units; concurrent siblings: none. Verification cadence: phase;
  role: phase-final. This is an internal checkpoint and is not independently
  releasable as configuration support.

  Contract: load an absent optional default or one strict regular JSON file;
  validate document version/fields, unique logical names and SSH destinations,
  exact local identity, two-provider unique absolute roots, and a 1–36500-day
  or all-history horizon. CLI supplies typed raw overrides and `config.rs` owns
  resolution. The document's own local identity must be valid before a valid
  override selects another configured node; an override without a loaded file
  errors. Input is capped at 1 MiB; symlinks to regular files are accepted;
  loaded paths are canonical; roots are validated lexically without probing.
  PH-2 owns all public flags, status projection, local command pre-effect
  loading, indexing consumption, and root-variable removal. PH-3 owns
  federation consumption and node-variable removal. No CLI exposure,
  environment removal, provider removal, ingestion change, SSH change,
  dependency, writer, model load, or config creation belongs here.

  Execution increments:

  1. Add `config.rs` private Serde document types, semantic validation, default
     path discovery, immutable resolved values, and table-driven units covering
     ordinary, all nested unknown-field levels, unsupported providers, empty
     provider maps, lexical roots, remote nonexistent roots, duplicate and
     boundary cases. Focused proof: `cargo nextest run -E
     'test(/config::tests::/)'` after verifying the selector with `cargo nextest
     list`.
  2. Add bounded regular-file loading, deterministic default-path failure,
     canonical status path, exact local resolution, and typed configuration
     errors. Prove missing, unreadable, directory, malformed, exactly-1-MiB and
     1-MiB-plus-one, broken/symlink-to-directory rejection,
     symlink-to-regular acceptance, no-config override, invalid file identity
     despite an override, and valid alternate-node override cases. A missing
     `ProjectDirs` result is a typed configuration error, never a relative
     fallback.
  3. Expose the module internally to the crate without changing current CLI
     parsing or responses; lower layers receive no partial config in PH-1.

  Proof: baseline Nextest at `4bca8d38` passed 52/52 with two skipped. Before
  edits, `vtas evidence discover` cleared live `rust-tests` drift; the baseline
  remains 74 claims, 54 evidence declarations, 54 links, 53 approvals, 38
  covered claims, 36 uncovered claims, and 97 findings. Targeted commands use
  the persistent Xenia paths. Phase exit also requires full Nextest, strict
  Clippy, rustfmt, doctests, strict OpenSpec validation, fresh discovery and
  status/report. New configuration declarations cite only the exact claims
  they prove. No coverage exclusion is allowed, and no agent approves evidence
  links; approval remains with the user or explicitly delegated human.

  Exit:

  - [ ] All internal PH-1 behavior and focused tests pass.
  - [ ] Canonical phase gate passes and runnable evidence is freshly discovered.
  - [ ] New/changed links are semantically reviewed and explicitly approved.
  - [ ] The immutable resolved config is available to PH-2/PH-3; later work
        remains owned there.

  Completion record: pending.

- [ ] **PH-2 — Activate local configuration and safe indexing atomically.**
  Depends on: PH-1. Expose `--config` and `--local-node`; validate configuration
  before all six public commands while hidden federation workers remain
  config-blind; add the exact nested status projection and stable error wire
  shape; replace ingestion environment lookup with resolved roots; retain only
  Claude Code and Codex discovery; add repeatable `--provider`, `--since-days`,
  and `--all-history`; and authorize reconciliation only after
  every selected provider root was inspected. Prove provider deduplication,
  unsupported-provider rejection, horizon precedence/conflicts, inclusive
  cutoff from one controlled run clock, complete-conversation admission,
  all-history mode, built-in fallback, inaccessible/disappearing-root
  preservation, old-source non-deletion, and Codex-only refresh preserving all
  Claude state. Also prove a malformed default/explicit file wins before each
  public command's model/database/scan/SSH effects, exact status JSON, hidden
  worker bypass, and the six provider-root environment variables having no
  effect. Exit: focused CLI/ingestion/storage tests plus the PH-1 native and
  Veritas gates. Owns global/provider/horizon flags, status, CLI loading,
  `ProviderRoots`, index orchestration, reconciliation authority, provider
  cleanup, root-environment removal, and tests.

- [ ] **PH-3 — Resolve configured federation and integrate commands.** Depends
  on: PH-1 and PH-2. Replace raw/environment nodes with configured logical names
  and unique SSH destinations; implement enabled default fanout, explicit
  deduplication, explicit selection of `search:false` nodes, local selection
  rejection, configured remote view, and nonrecursive hidden workers; delete
  `CASS_SEARCH_NODES`. Preserve the fixed command, timeout, protocol v2, typed
  errors, partial success, and deterministic provenance. Prove no-config local
  behavior, explicit-without-config error, default/subset matrices,
  unknown/local failures before local search or SSH, logical names distinct
  from destinations, fake-SSH nonrecursion, and `CASS_SEARCH_NODES` having no
  effect. Exit: focused federation/CLI
  tests plus the PH-1 native and Veritas gates. Owns federation selection/node
  types, search/view integration, and tests.

- [ ] **PH-4 — Consolidate repository code and proof.** Depends on: PH-3.
  Remove obsolete environment/provider helpers, tests, and docs; collapse
  temporary resolution seams; update help and example configuration; and
  confirm no new abstraction or dependency survived. Exit: zero legacy
  environment reads, only two discovery providers, full native gates green,
  strict OpenSpec validation, and current Veritas evidence/status/report with
  every new or changed link separately reviewed and explicitly approved by the
  user or an explicitly delegated human. Agents stop rather than self-approve.
  Owns docs, generated Veritas state through CLI commands, and final plan
  records.

- [ ] **PH-5 — Roll out and prove the three-machine fleet.** Depends on: PH-4
  and separate external-write authority. Create and test a restorable database
  backup on Xenia, dev-macbook, and personal-macbook. Stage one validated config
  per machine atomically; deploy the binary; run read-only status and local
  search smoke tests; only then run configured indexing. Require green CI and
  cross-deployment, then verify each node's identity, roots, horizon, local
  hybrid search, default three-node federation, explicit subset, and remote
  view. Exit: backup/restore path recorded, deployment green, and live smoke
  results recorded. Owns deployment configuration/state and rollout records.

Dependency edges: `PH-1 → PH-2 → PH-3 → PH-4 → PH-5`. Initial ready set:
`PH-1`. Native tests within a phase run in parallel through Nextest. Fleet
rollout is deliberately separated from repository completion.

## Traceability and Evidence Assignment

| Claim group | Component | Phase | Runnable evidence |
|---|---|---|---|
| Internal config discovery and loading | Configuration resolver | PH-1 | bounded file/path resolver units |
| Document, node, provider, root, and horizon validation | Configuration resolver | PH-1 | table-driven validation tests |
| Public pre-effect failures, status, and error wire shape | Configuration resolver / CLI | PH-2 | six-command process matrix and semantic JSON tests |
| Local identity and precedence | Configuration resolver / CLI | PH-1, PH-2 | resolver units and CLI matrix |
| Provider-root environment inputs have no effect | Provider selection | PH-2 | isolated child-process indexing tests |
| Federation environment input has no effect | Configured federation | PH-3 | isolated child-process fake-SSH test |
| Two-provider boundary and CLI selection | Provider selection | PH-2 | ingestion fixtures and CLI tests |
| Horizon admission and non-retention | Provider selection | PH-2 | controlled-clock boundary and old-row preservation tests |
| Restricted/incomplete-scan preservation | Provider selection | PH-2 | two-provider and unavailable-root storage tests |
| Default/explicit node selection and validation | Configured federation | PH-3 | pure selection matrix and fake-SSH tests |
| Concurrent fanout, provenance, and nonrecursion | Configured federation | PH-3 | fake-SSH aggregate/worker CLI tests |
| Configured remote view | Configured federation | PH-3 | fake-SSH view request/response test |
| Three-machine operational contract | All | PH-5 | per-node status/search, federation, and remote-view smoke |

Each implementation phase runs the persistent-path Nextest, strict Clippy,
rustfmt, and doctest commands from `AGENTS.md`. After test changes, `vtas
evidence discover` refreshes declarations. Every changed or new claim/evidence
link receives individual semantic review and explicit human approval; passing
tests never implies approval. OpenSpec strict validation runs in every phase.

There are no coverage exclusions. Commit, push, deployment, database backup,
and fleet configuration require the user's separate implementation or
external-write authority; this plan alone performs none of them.

## Risks and Open Questions

- The preceding semantic deployment and full-index benchmark must finish before
  PH-5, so performance results remain attributable to that change.
- Strict configuration means all three files must be updated together when
  inventory changes. Synchronization remains external to CASS.
- File modification time is a fast source-activity proxy. Copied or touched
  histories may be admitted, but complete conversations remain coherent.
- The 90-day default changes first-run coverage. `--all-history` is the explicit
  archival path; PH-2 cannot complete without proof that the horizon never
  silently becomes retention.
- No behavioral, architecture, evidence-support, or delivery-mode question
  remains open for implementation.
