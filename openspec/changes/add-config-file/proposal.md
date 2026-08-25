## Why

Federated search and machine-specific provider roots are persistent deployment
facts, but CASS currently spreads them across environment variables, repeated
CLI arguments, SSH configuration, and implicit defaults. That makes a
three-machine installation difficult to inspect and easy to run differently on
each machine. CASS needs one explicit, versioned configuration contract for
node identity, provider roots, and default federation membership.

## What Changes

- Add an optional versioned CASS configuration file and global `--config` path.
- Add explicit local-node selection in configuration with a `--local-node`
  override; CASS does not guess node identity from hostnames.
- Configure each node's SSH destination, default search participation, Claude
  Code and Codex roots, and an indexing recency horizon that defaults to 90
  days.
- Make a search without `--node` include the local corpus plus configured
  search-enabled remote nodes; repeatable `--node` selects an explicit remote
  subset.
- Add repeatable index-time provider selection plus `--since-days` and
  `--all-history` horizon overrides.
- Define precedence as CLI values, then current-node configuration, then
  built-in defaults.
- **BREAKING:** remove every `CASS_*_ROOTS` provider variable and
  `CASS_SEARCH_NODES` as configuration inputs instead of retaining two
  permanent configuration systems.
- Restore the product boundary to the two supported providers, Claude Code and
  Codex; OpenCode, GitHub Copilot CLI, Hermes Agent, and Pi are not configured
  or discovered by this change.

## Capabilities

### New Capabilities

- `configuration`: Versioned configuration discovery, validation, explicit
  local-node resolution, provider roots, precedence, and typed failures.

### Modified Capabilities

- `cass-independent-core`: Indexing obtains enabled providers and roots from
  the resolved local node while retaining concrete provider implementations.
- `federated-search`: Default and explicit remote selection use configured node
  identities, SSH destinations, and bounded timeouts.

## Success Boundary

- The same node inventory can describe Xenia, dev-macbook, and
  personal-macbook while each installation explicitly selects its local entry.
- `cass index` scans only the resolved local node's enabled providers and roots,
  with CLI overrides taking precedence. By default it admits sources active in
  the last 90 days; configuration or CLI flags can select another positive
  horizon or all history.
- Sources excluded only by the recency horizon cannot authorize deletion of
  canonical or derived rows already stored for those sources.
- `cass search` without explicit nodes searches local data plus every other
  search-enabled configured node exactly once.
- Explicit node selection restricts remote search without suppressing local
  hybrid search.
- Missing optional configuration preserves current local-only built-in
  behavior; an explicitly selected missing or invalid file fails with a typed
  JSON configuration error.
- Environment variables no longer alter provider roots or federation targets.

## Non-Goals

- File-size exclusion, watch mode, synchronization, automatic retention, or
  remote indexing orchestration.
- Configurable federation deadlines; the existing bounded timeout remains a
  fixed transport policy.
- Configurable FTS thresholds, batch sizes, RRF, model identities, embedding
  dimensions, ranking constants, or database schemas.
- Hostname probing, fuzzy node matching, SSH discovery, or compatibility
  aliases for the removed environment variables.
- Secrets, SSH keys, database paths, or model paths inside the shared node
  inventory.

## Impact

The CLI, provider-root resolution, federation node selection, SSH invocation,
JSON error surface, tests, documentation, and three-machine deployment
configuration change. The remaining non-product provider discovery paths are
removed. No runtime dependency, provider abstraction, registry, daemon, or
alternate output mode is introduced.
