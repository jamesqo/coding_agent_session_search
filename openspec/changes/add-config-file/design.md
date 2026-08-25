## Context

CASS currently resolves provider roots from provider-specific environment
variables and resolves federation targets from raw SSH aliases supplied by an
environment variable or `--node`. It also retains discovery paths beyond the
repository's Claude Code/Codex product boundary. The CLI already has concrete
provider-root and federation modules, so configuration should resolve values
once at the application boundary and feed those modules without introducing
provider or transport abstractions.

The configuration file is optional for local-only use. A loaded file represents
a shared node inventory but carries one explicit `local_node` selection; a
machine may keep its own copy or use `--local-node` when sharing identical
bytes. Database and model paths remain outside the inventory.

## Goals / Non-Goals

**Goals:**

- Parse and validate one strict version-1 JSON document before command effects.
- Resolve one concrete local node, enabled provider roots, default remote nodes,
  SSH destinations, and a default 90-day indexing horizon through a
  deterministic precedence pipeline.
- Keep ingestion and federation concrete, synchronous, and JSON-only.
- Preserve provider-scoped purge safety for restricted index runs.
- Remove legacy environment inputs rather than maintaining compatibility code.

**Non-Goals:**

- Hostname detection, config synchronization, SSH key management, remote index
  execution, watches, file-size filters, automatic retention, or configurable
  retrieval internals.
- A provider registry, plugin interface, generic config merge framework, or
  compatibility schema migration layer.

## Ownership and Boundaries

Add one concrete `app/config.rs` module owning deserialization, validation,
default path resolution, local-node selection, CLI/config/default precedence,
and immutable resolved command inputs. CLI parsing supplies typed raw override
values; the resolver returns owned concrete structs and typed
`AppError::configuration` failures. It does not open SQLite, discover files,
load models, or start processes.

`app/cli.rs` owns only flag parsing and command composition. It loads
configuration once, passes typed overrides to `config.rs`, then passes the
resolved provider selection and horizon to `ingestion.rs` and resolved remote
nodes to `federation.rs`. Global database and model path resolution remains
unchanged.

`app/ingestion.rs` retains only the Claude Code and Codex parsers. Replace
environment lookup inside `ProviderRoots::new` with an explicit resolved
provider map. Provider selection is a concrete exhaustive operation over those
two names; no trait or registry is added. A single run-start timestamp resolves
the cutoff, and a source excluded by age is not evidence that it disappeared.

`app/federation.rs` receives `{name, ssh}` values rather than treating a node
name as an SSH destination. Result origins and `view --node` continue to use the
stable logical name. Process execution receives the SSH destination as a single
argument after `--`.

Hidden federation workers branch before configuration discovery and execute
only their structured local operation. Explicit config/local-node flags are
invalid in worker mode. This prevents a remote machine's malformed or
search-enabled default configuration from breaking or recursively expanding a
protocol request.

Dependency direction remains:

```text
CLI flags + config.json
          ↓
   concrete resolver
      ↙         ↘
 ingestion    federation
      ↓           ↓
  SQLite      fixed SSH command
```

## Decisions

### Strict JSON version 1

Use Serde's strict unknown-field rejection on private concrete structs. Validate
semantic invariants in one pass after deserialization: version, unique names
and SSH destinations, local-node membership, the two provider names, unique
absolute nonempty roots, a positive bounded `since_days` value or null, and
bounded SSH values. No new dependency is needed.

Read at most 1 MiB and accept symlinks whose targets are regular files. The
loaded file path is canonicalized for status. Root paths are checked lexically,
not canonicalized or probed, because remote-node roots are not expected to
exist locally. Empty provider maps are valid for search-only nodes.

The default path comes from the existing `directories::ProjectDirs`
configuration directory and ends in `config.json`. `--config` replaces that
path. An absent default file yields `None`; every other read or validation
failure is typed. If `ProjectDirs` cannot supply an absolute configuration
directory, resolution fails instead of consulting the current directory.

### Explicit resolution object

Produce a small resolved value containing the loaded path, optional local node,
provider roots, indexing horizon, and candidate remotes. CLI overrides are
applied while creating this value, not lazily inside ingestion or federation.
This gives every command one source of truth and keeps precedence out of lower
layers. Every public command validates an existing default file or explicit
file before opening models, a database writer, or SSH. Hidden federation
workers never load configuration. Configuration errors use exit code 9, are
nonretryable, and carry no recommendation.

Public activation is atomic at the consumer boundary. The first implementation
phase builds and proves the internal parser/resolver without exposing flags or
status fields. The second phase exposes `--config`/`--local-node`, status, and
local indexing together, so CASS never reports roots or a horizon that indexing
ignores. Federation begins consuming the same resolved inventory in the third
phase.

### Logical node names remain provenance

Federated result origins store the configured node `name`; only process launch
uses `ssh`. This preserves stable user-facing node identity when an SSH alias or
Tailscale address changes.

### Provider-scoped completeness and recency

Build an explicit selected-provider set for each index run. Discovery reports
completeness independently for those providers, and purge runs only for a
selected provider whose every authoritative configured root was successfully
inspected. There is no ad hoc root override. Missing or unreadable roots make
the provider scan incomplete and preserve its stored state.

The resolved horizon defaults to 90 days. `--since-days` selects another
positive duration and `--all-history` disables the cutoff. CASS computes one
inclusive cutoff from the run-start clock, uses source modification time as the
fast eligibility signal, and ingests the complete conversation from eligible
sources. Ineligible old sources are outside reconciliation authority, so this
feature bounds new work without silently becoming data retention.

### Configuration shape

```json
{
  "version": 1,
  "local_node": "xenia",
  "nodes": [
    {
      "name": "xenia",
      "ssh": "xenia",
      "search": true,
      "providers": {
        "claude-code": { "roots": ["/home/james/.claude/projects"] },
        "codex": { "roots": ["/home/james/.codex/sessions"] }
      },
      "index": { "since_days": 90 }
    }
  ]
}
```

Omitting `index.since_days` means 90 days. Setting it to JSON `null` means all
history. Database paths, model paths, SSH keys, and retrieval constants remain
outside this document.

### BJ-1 — Require explicit local identity

- Decision: `local_node` or `--local-node` identifies the machine exactly.
- Scenario: the three current machines have OS, mDNS, Tailscale, and SSH names
  that need not agree.
- Source/owner: user-requested multi-machine inventory and observed deployment
  aliases.
- Simpler behavior considered: compare the node name to the OS hostname.
- Scope cost: each machine needs one local selection; retire only if CASS gains
  a reliable externally supplied machine identity.

### BJ-2 — Reject unknown fields and versions

- Decision: loaded configuration is strict instead of ignoring unknown input.
- Scenario: a misspelled provider or search field would otherwise silently
  remove histories or nodes from a search.
- Source/owner: the user prioritized reliability and authorized removal of
  questionable behavior.
- Simpler behavior considered: permissive Serde defaults.
- Scope cost: future fields require a versioned contract update; no migration
  machinery is added for version 1.

## Tooling Compatibility

- Implementation language: Rust.
- Native runner: cargo-nextest 0.9.143 for unit and CLI process tests; Cargo is
  used only for doctests where Nextest cannot execute them.
- Veritas producer: the configured `rust-test` discovery producer supports the
  Rust unit and integration declarations used by this change.
- Evidence access: project-bound `vtas` CLI fallback is operational; no
  cross-language bridge or fallback test framework is required.

## Risks / Trade-offs

- A shared file cannot contain a different `local_node` value on each machine.
  Operators must keep machine-local copies, generate that one field during
  deployment, or supply `--local-node`. This is explicit and predictable.
- Removing environment inputs is immediately breaking for existing scripts.
  The fleet deployment must install configuration before relying on default
  federation, and tests must prove environment values have no effect.
- Strict absolute roots reject convenient relative fixtures. Tests and one-off
  use can obtain absolute temporary paths; avoiding working-directory semantics
  is worth the constraint.
- Provider-restricted indexing can be destructive if completeness is applied
  globally. The selected-provider set is therefore part of purge authorization,
  with focused preservation tests before deployment.
- File modification time is a deliberately cheap definition of source
  activity. Copied or touched histories may be admitted even if their messages
  are older, but the complete conversation remains coherent and no old stored
  history is deleted merely because time advances.

## Migration / Rollback

1. Land the parser and CLI wiring while retaining absent-file local defaults.
2. Replace environment-based tests, delete all seven legacy environment
   lookups, and remove discovery for providers outside Claude Code and Codex in
   the same delivery so there is no ambiguous compatibility interval.
3. Before deployment, create and verify a restorable database backup on each
   machine, then create one version-1 file per machine with the same node
   inventory and that machine's `local_node`; validate each through `cass
   status --config ...`.
4. Stage each config atomically, deploy, run read-only status and search smoke
   tests, and only then run configured indexing. Confirm provider roots, local
   identity, horizon, default remote set, local search, and configured federated
   search on all three machines.

Rollback stops new indexing, restores the pre-cutover database backup if any
configured index ran, and redeploys the previous binary. The JSON files are
additive external state and may remain because the older binary ignores them.
Any old environment variables should stay unset so rollback behavior remains
deliberate.
