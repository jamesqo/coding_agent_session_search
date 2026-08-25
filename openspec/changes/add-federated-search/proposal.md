## Why

CASS histories are distributed across Xenia, dev-macbook, and personal-macbook. Copying those histories or databases into one canonical store adds synchronization, conflict, storage, and freshness problems. CASS should instead query already-indexed remote databases at search time so an agent can retrieve relevant work across the tailnet without centralizing ownership.

## What Changes

- `cass search` accepts repeated `--node <SSH_HOST>` flags and otherwise reads comma-separated defaults from `CASS_SEARCH_NODES`.
- Explicit `--node` values replace environment defaults; with neither source configured, search remains local-only.
- CASS searches the local database and all selected nodes concurrently over SSH, applies a bounded deadline, and returns successful partial results alongside per-node outcomes.
- Federated results identify their origin nodes, deduplicate identical messages, and merge node-local rankings deterministically without comparing database-local BM25 scores.
- `cass view` accepts `--node <SSH_HOST>` to retrieve context from the node that produced a result.
- Remote requests use a fixed CASS command and structured standard input so query text and identifiers never enter a generated shell command.

## Capabilities

### New Capabilities

- `federated-search`: Opt-in SSH/Tailscale fan-out, deterministic result merging, partial-failure reporting, and origin-aware remote viewing.

### Modified Capabilities

- None.

## Success Boundary

- A search with two reachable nodes returns ranked local and remote matches in one JSON response.
- A sleeping, unreachable, malformed, or incompatible node does not suppress results from healthy nodes and is represented by a typed node outcome.
- Repeated explicit nodes are deduplicated, override environment defaults, and cannot be interpreted as SSH options or shell syntax.
- Duplicate messages stored on multiple machines appear once with every origin recorded.
- A returned remote result can be passed to `cass view --node` to retrieve its surrounding context.
- Existing invocations without nodes preserve their current local behavior and response semantics.

## Non-Goals

- Synchronizing histories, SQLite databases, indexes, models, or configuration between machines.
- Automatically indexing local or remote sources during search.
- Adding a daemon, HTTP service, service discovery, shared database, or central coordinator.
- Automatically discovering tailnet devices or modifying Tailscale/SSH configuration.
- Performing a second global semantic inference or reranking pass in the aggregator.
- Adding a general remote-execution or plugin framework.

## Impact

- The `search` and `view` CLI flag surfaces and search JSON response gain additive federated fields.
- CASS invokes the existing system `ssh` executable only when nodes are selected.
- Remote nodes must expose a compatible CASS binary through a fixed command path and permit noninteractive SSH authentication.
- Local-only storage, indexing, semantic search, and reranking remain unchanged.
