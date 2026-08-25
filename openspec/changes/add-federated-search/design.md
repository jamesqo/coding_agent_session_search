## Context

CASS is a synchronous JSON-only Rust CLI with local Rusqlite/FTS5 storage and optional local semantic inference. The three deployment targets already have noninteractive SSH connectivity over Tailscale and receive compatible binaries from one workflow. The retained command implementation is concentrated in `app/cli.rs`; search produces a concrete `SearchResponse` containing concrete storage hits.

The federation boundary is opt-in. A process with no selected nodes must not pay a subprocess, threading, schema, or output compatibility cost.

## Goals / Non-Goals

**Goals:**

- Add bounded concurrent SSH fan-out around the existing local search implementation.
- Preserve exact local-only behavior while exposing deterministic origin-aware federated JSON.
- Keep queries and identifiers out of generated remote shell text.
- Make remote failure an observable per-node outcome rather than a global search failure.
- Reuse the deployed CASS binary as both client and remote endpoint without adding a daemon.

**Non-Goals:**

- Synchronization, discovery, background services, shared storage, or automatic indexing.
- A generic transport abstraction, provider registry, remote-execution framework, or async runtime.
- Globally recomputing embeddings or reranking remote text.

## Ownership and Boundaries

- `app/cli.rs` owns public flags, local search/view behavior, and response construction.
- A new private `app/federation.rs` owns node parsing and validation, the versioned structured request/response envelopes, SSH child lifecycle, deadline enforcement, remote outcome classification, and pure deterministic merging.
- `app/storage.rs` continues to own concrete search hits. It gains only serialization fields required to deserialize compatible remote hits and optional federated provenance fields omitted from local output.
- The caller process is the sole aggregator. Remote CASS processes execute hidden local-only request modes and never consult `CASS_SEARCH_NODES`, preventing recursive federation.
- OpenSSH and the remote CASS executable are external boundaries. CASS passes a validated node as an SSH argv element and sends all variable request data through child stdin.

## Decisions

### Use system SSH over Tailscale

The client invokes `ssh` with `BatchMode=yes`, a bounded connect timeout, keepalive limits, the validated host alias, and the constant remote command `~/.local/bin/cass <operation> --federation-request`. This reuses authentication and host verification already maintained outside CASS. An HTTP daemon was rejected because it adds service lifecycle, authorization, port, and deployment machinery. Database replication was rejected because it changes freshness and ownership semantics.

### Use a small versioned stdin protocol

Hidden search and view request modes deserialize a JSON envelope with protocol version `1` from stdin and serialize a versioned response envelope to stdout. These modes force local execution and ignore node defaults. Protocol mismatch, malformed JSON, extra stdout, and nonzero exit become node outcomes. The public query and identifier never appear in the SSH remote command.

### Use bounded synchronous concurrency

The aggregator starts one standard-library worker thread per unique node, capped at sixteen nodes, then performs local search on the caller thread. Each worker drains stdout and stderr concurrently, polls child completion, kills the child at five seconds, and returns one owned result. This prevents pipe deadlock and avoids adding Tokio or a process-timeout dependency for a maximum of sixteen short-lived children.

### Merge final node rankings

Each healthy node returns up to the requested final limit from its existing lexical or hybrid/reranked pipeline. The aggregator adds the local result set, assigns `1 / (rank + 1)` within each origin, and keys candidates by `(provider, conversation_id, message_id)`. Duplicate candidates keep the maximum contribution rather than summing it, so replicated corpora are not artificially boosted. Origins preserve local-first then selected-node order. Final ordering uses contribution descending and the identity tuple as a deterministic tie-breaker.

Raw BM25, cosine, fusion, and reranker scores remain node-local diagnostics because their calibration can differ across corpora and model availability. A global reranker can be evaluated later from measured retrieval quality.

### Preserve local response compatibility

Federated-only fields use conditional serialization and remain absent when no nodes are selected. Federated responses set aggregate mode to `federated`, include node outcomes, and add `origins` plus `federated_score` to hits. Explicit `--node` values replace environment defaults; this makes local-only recovery possible by unsetting the environment without adding another flag.

## Tooling Compatibility

- Implementation language: Rust 2024 edition.
- Native test runner: cargo-nextest 0.9.143; doctests remain under `cargo test --doc`.
- Veritas evidence producer: project-bound `rust-test` discovery, with Nextest executing discovered tests.
- Veritas access: project-bound `vtas` CLI fallback is healthy; no cross-language bridge is required.
- No production dependency is added. Standard-library processes, threads, channels, clocks, and JSON types already present in the manifest cover the design.

## Risks / Trade-offs

- SSH startup adds latency. Fan-out runs concurrently and remains opt-in; repeated-use connection multiplexing stays an SSH configuration concern.
- Five seconds can truncate a genuinely slow remote semantic search. The response exposes timeout explicitly, and local results remain available.
- Full message content can make remote JSON large. Existing message content is already bounded during ingestion; each node returns only the requested limit.
- Remote binaries may differ. The versioned envelope rejects incompatible peers rather than guessing.
- A child may resist normal termination. The worker issues a kill at deadline and waits for cleanup before returning, preventing orphaned SSH processes.
- Rank-only merging loses cross-node score magnitude. It avoids invalid cross-corpus BM25 comparison and is deterministic; retrieval evaluation can justify a later global reranker.

## Migration / Rollback

The change requires no database migration. Existing local invocations and databases remain valid. Deployment must place a compatible binary on each selected node before federation is used. Rolling back the caller removes the new flags; rolling back a remote node produces a typed incompatible-node outcome without affecting local search.
