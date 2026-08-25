## Why

CASS currently treats missing or broken semantic assets as permission to return lexical-only results. Dogfooding showed that this silently downgrades production retrieval quality while still reporting a successful search. Production CASS should instead make its required hybrid retrieval contract explicit and actionable.

## What Changes

- **BREAKING:** `cass search` succeeds only when compatible embedding and reranking models are installed and the current database has complete, current semantic vectors.
- **BREAKING:** semantic model load or inference failure becomes a typed search failure instead of lexical fallback.
- **BREAKING:** `cass index` requires installed semantic models and fails before scanning when they are absent.
- `cass status` distinguishes model installation, embedding readiness, and search readiness, and reports the next required operational command.
- Ordinary refreshes update canonical rows and embeddings only for changed messages. Small transaction batches update matching FTS rows; a deterministic, benchmark-selected cutoff lets a large transaction rebuild FTS before its checkpoint commits. `index --full` retains whole-run bulk rebuilding.
- Full tool-result payloads remain available to `view` but are excluded from FTS and semantic embeddings; mixed prose/tool messages index only their non-tool-result text.
- Officially deployed binaries support the semantic backend only; lexical retrieval remains an internal candidate source within hybrid search rather than a standalone production realization.
- `models install` remains the only command allowed to download model assets.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `cass-independent-core`: Replace lexical fallback and lexical production-build behavior with required hybrid readiness, typed failures, and actionable status reporting.
- `federated-search`: Treat a remote node that is not semantically ready as a partial node failure while preserving successful hybrid results from ready nodes.

## Success Boundary

- A production search without installed models fails with a typed error recommending `cass models install`.
- An index attempt without installed models fails before source discovery and recommends `cass models install`.
- A search with installed models but missing or stale embeddings fails with a typed error recommending `cass index`.
- A model load, embedding, or reranking failure never returns lexical-only results.
- A ready local or federated search reports hybrid/federated realization and retains FTS5 lexical candidates inside hybrid fusion.
- A small ordinary incremental transaction does not rewrite FTS rows or embeddings for unchanged messages; a large transaction may rebuild FTS but does not regenerate current embeddings for unchanged messages.
- A query term present only in tool-result output is not searchable, while the complete currently retained canonical output remains visible in conversation context.
- All three deployed machines install models, build current embeddings, and complete a federated semantic-search smoke test.

## Non-Goals

- Removing FTS5 or lexical candidate generation from hybrid retrieval.
- Automatically downloading models during `search`, `index`, or `status`.
- Adding another model backend, semantic tier, daemon, ANN index, or runtime mode flag.
- General role weighting or ranking heuristics beyond excluding tool-result payloads.
- Changing the existing ingestion safety bound for exceptionally large tool outputs.

## Impact

Users must run `cass models install` and then `cass index` before searching. Existing lexical-only automation becomes an explicit error path. Status JSON, index and search errors, normal-versus-full derived-index maintenance, semantic tests, the optional lexical build realization, CI expectations, deployment smoke checks, and federated node outcomes are affected. A forward schema migration adds an internal search projection and invalidates source checkpoints so existing histories are re-normalized once; no new dependency is required.
