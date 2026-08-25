## Context

The current default build includes FastEmbed, but Cargo can omit it and the runtime silently falls back to FTS5 whenever models, embeddings, or inference are unavailable. Indexing always defers FTS maintenance, so any changed message marks the derived index dirty and triggers a full FTS rebuild. The storage layer already supports per-message FTS mutation and selects only messages missing the current embedding generation; the CLI currently bypasses both advantages. Normalized canonical content also conflates searchable prose with tool-result payloads, including mixed Claude content blocks, so role filtering alone cannot remove raw output safely.

The three production machines now have canonical lexical indexes containing roughly 24k, 203k, and 291k messages. None has installed model assets or embeddings yet. Federation protocol version 1 accepts lexical and hybrid node responses interchangeably.

## Goals / Non-Goals

**Goals:**

- Make every supported binary and every successful search semantic/hybrid.
- Fail early and actionably when models or embeddings are not ready.
- Make small ordinary index refresh proportional to changed sources and messages while retaining a measured bulk crossover for large FTS transactions.
- Exclude structured tool-result payloads from both derived retrieval paths without losing view context.
- Preserve FTS5 as a hybrid candidate source and preserve the explicit full rebuild path.
- Migrate all three production machines without changing the database schema.

**Non-Goals:**

- Change models, quantization, exact cosine search, RRF, or reranking algorithms.
- Add runtime modes, alternate backends, daemons, ANN indexes, or implicit downloads.
- Solve result-role weighting or other relevance tuning.

## Ownership and Boundaries

- `app/semantic.rs` remains the sole model installation, loading, embedding, and reranking owner. Its production API no longer represents an absent backend as a successful optional value.
- `app/storage.rs` owns the optional search projection, searchable-message counts, current-generation embedding coverage, and per-message FTS/embedding mutation.
- `app/cli.rs` owns readiness precedence, typed recommended actions, index orchestration, and the rule that only hybrid search may return results.
- `app/federation.rs` owns protocol compatibility and node outcomes. It accepts only the new semantic-required protocol and successful hybrid node responses.
- Cargo and CI own one build realization. FastEmbed becomes unconditional and `semantic_disabled.rs` is deleted.

Dependency direction remains CLI orchestration toward concrete storage and semantic modules; neither storage nor semantic code depends on federation or presentation.

## Decisions

### One unconditional semantic build

Make `fastembed` non-optional, remove the `semantic` Cargo feature and all semantic feature gates, and delete the lexical stub module. `--no-default-features` then cannot disable semantic support because there is no semantic feature to disable. This is smaller and harder to misconfigure than maintaining a development-only runtime that production policy forbids.

Alternative considered: retain the lexical feature for tests and reject it only in release profiles. That preserves a second implementation path and allows locally green behavior that production cannot execute, so it is rejected.

### Models are a prerequisite for indexing

`index` loads installed models before opening a writer or discovering sources. Missing or invalid assets therefore fail without touching the database. Once ready, normal ingestion, FTS mutation, and embedding inference complete under the existing writer lifecycle; inference failure returns the model error rather than committing a partially searchable generation.

This ordering also makes status guidance unambiguous: install models first, then index.

### Search readiness is exact coverage

Before inference, search compares canonical searchable-message count with the current embedding-generation count. Missing models produce a model error recommending `models install`; unequal coverage produces a readiness error recommending `index`; model loading or inference errors propagate unchanged. No path calls `Storage::search` as a final response fallback. FTS5 remains called only by `semantic::hybrid_search` as one candidate list.

Typed JSON errors gain an optional `recommended_action` field. Existing errors omit it, preserving their serialized shape.

### Transaction-sized adaptive FTS maintenance is the normal path

Ordinary `index` stops enabling whole-run deferral. The storage writer tracks the number of changed and removed messages in its current transaction. Below a fixed cutoff it applies only the queued row-level FTS deletes and inserts; at or above the cutoff it rebuilds FTS once. FTS maintenance finishes before each bounded writer checkpoint commits, so no committed checkpoint pairs new canonical rows with stale FTS state. `index --full` explicitly retains whole-run deferral and rebuilding. Embedding generation invalidation still makes every searchable message eligible when the model/vector identity changes; otherwise `messages_needing_embeddings` limits inference to added or changed messages even when FTS takes the bulk path.

The cutoff is one declared changed-message ratio selected by a reproducible benchmark over representative database sizes and transaction deltas. The benchmark compares row-level and bulk FTS maintenance at 1, 10, 100, 1,000, 10,000, and corpus-percentage changes; the chosen crossover and measurements are recorded with the change. Tests assert deterministic strategy selection, atomic rollback, affected embeddings, and equivalent query results rather than wall-clock timing. Explicit FTS `optimize` maintenance runs after `index --full`, not after ordinary refreshes, including automatic bulk transactions.

### Canonical content and search projection are separate

Add a nullable `search_projection` column to canonical messages. `NULL` means the complete canonical `content` is searchable without duplicating it; an empty string means the message is context-only; a nonempty value is filtered searchable text for a mixed message. Parsers retain canonical content, including the existing 128-KiB safety bound for exceptionally large explicit tool outputs, while separately classifying structured tool-result blocks. Tool calls/invocation metadata remain searchable because they are not result payloads; general role weighting is outside this change.

FTS insertion and embedding selection consume the effective projection rather than raw canonical content. Search hits and `view` continue returning canonical content. Search readiness compares embeddings against searchable-message count, not total canonical message count.

The schema migration adds the nullable column, clears derived FTS/embedding readiness, and invalidates source checkpoints. Existing source histories are therefore re-parsed once so provider-specific structure can produce correct projections; persisted flattened text alone cannot reconstruct mixed Claude messages. Stable conversation and message identifiers preserve canonical identity.

### Federation protocol version 2 requires hybrid success

Bump the hidden SSH protocol version. Version 2 search envelopes are successful only when their response realization is `hybrid`; older version-1 binaries and any lexical response become classified node compatibility failures. The aggregate still reports `federated`. Local readiness remains mandatory because the local error is the command's primary failure boundary.

### Remove fallback response state

Search and index responses no longer serialize `fallback_mode` or `fallback_reason`. Successful search says `hybrid`; successful index says `hybrid` and reports generated/current embedding counts. Status becomes the stable readiness surface. Protocol versioning prevents mixed deployed binaries from interpreting the changed search envelope as compatible.

## Tooling Compatibility

- Implementation language: Rust 2024 edition.
- Native test runner: cargo-nextest 0.9.143; `cargo test --doc` remains the doctest runner.
- Veritas producer: project-bound `rust-test`, compatible with the Rust unit and CLI integration tests executed by Nextest.
- Veritas access: project-bound `vtas` CLI fallback.
- Cross-language bridge: none.

Removing a Cargo feature changes dependency resolution and requires the full semantic test, strict Clippy, rustfmt, doctest, and deployment build gates.

## Risks / Trade-offs

- Initial embedding generation across the searchable subset of approximately 518k canonical messages on the Macs may be long and CPU-intensive. Mitigation: exclude tool-result payloads first, run machines concurrently, and report per-machine searchable counts and progress.
- FastEmbed inference may exceed federation's five-second deadline on a cold process. Mitigation: preserve explicit timeout outcomes for this version and measure warm/cold behavior during smoke testing; changing the deadline requires a separate observed decision.
- Requiring models before index prevents canonical-only ingestion during model outages. This is intentional: a successful production index must produce searchable state.
- The best per-message-versus-bulk FTS crossover varies with corpus and hardware. A measured fixed corpus ratio keeps behavior deterministic; PH-1 records representative results and chooses the conservative crossover rather than adding adaptive tuning machinery.
- Protocol version 2 causes rolling-deployment nodes to appear temporarily incompatible. Partial-node failure preserves ready results until deployment finishes.

## Migration / Rollback

1. With the current compatible binary, run `cass models install` on Xenia and both Macs so assets are staged without changing search behavior.
2. Take one local backup copy of each canonical database before schema migration.
3. Deploy the strict semantic/schema binary to all machines and verify protocol version 2 installation.
4. Run `cass index --full` on all three machines concurrently. The migration reparses histories, builds tool-filtered FTS state, and creates current embeddings; record duration, canonical/searchable counts, and failures.
5. Confirm `status` reports semantic readiness everywhere, prove a tool-output-only term is absent while `view` retains it, then run local hybrid searches, federated search, and origin-aware view.

Before a database is migrated, rollback is a normal code revert and redeploy. After migration, the older binary correctly rejects the newer schema; rollback therefore restores the pre-migration database backup before redeploying the previous binary. Source histories and staged model assets are unchanged, but the backup is required to preserve CASS-owned tombstones and avoid destructive reconstruction.
