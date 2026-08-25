## Context

CASS now serves one narrow need: local search over Claude Code, Codex, current
OpenCode, GitHub Copilot CLI, Hermes Agent, and Pi histories. There are no
backward-compatibility requirements for older product surfaces. SQLite is
canonical; FTS rows and embeddings are derived.

## Goals / Non-Goals

**Goals:**

- Keep semantic search in official binaries while supporting a fast lexical
  development build.
- Make indexing incremental, bounded, durable across forget operations, and
  safe under concurrent process attempts.
- Migrate the compact schema forward without importing legacy salvage logic.

**Non-Goals:**

- Alternate semantic backends, ANN indexes, background indexing, provider
  abstraction, or compatibility with removed schemas and providers.
- Automatic model acquisition or semantic availability as a prerequisite for
  lexical indexing and search.

## Ownership and Boundaries

- `app/ingestion.rs` owns six concrete discovery/parsing paths and produces
  canonical conversations and messages with stable external identities.
- `app/storage.rs` owns schema versions, migrations, the writer transaction,
  source/message fingerprints, tombstones, canonical rows, FTS5 rows, and
  complete-scan reconciliation.
- `app/semantic.rs` owns one feature-gated FastEmbed backend, exact cosine
  retrieval, bounded embedding batches, RRF, and bounded reranking.
- `app/cli.rs` owns truthful JSON realization and converts storage busy,
  incompatibility, and semantic fallback outcomes into stable response fields.
- GitHub Actions owns full release builds; Cargo feature selection owns the
  lexical-only development path.

## Decisions

- Use six concrete ingestion paths returning CASS-owned `Conversation` and
  `Message` values. Do not introduce a provider trait or registry.
- Configure local discovery through built-in roots and the existing
  provider-specific root environment variables; do not add index-time provider
  root flags.
- Use Rusqlite as the sole database library. Track the schema with
  `PRAGMA user_version`, apply small forward-only migrations transactionally,
  and reject newer unknown versions before writes.
- Serialize indexing with one `BEGIN IMMEDIATE` writer transaction and a zero
  busy timeout so a competing index process receives an immediate structured
  busy result. WAL readers continue against committed state. Do not add a
  second lock-file protocol unless measurements prove inference must move
  outside the writer transaction.
- Store a deterministic source fingerprint and a normalized-content fingerprint
  per message. Skip unchanged sources; upsert and re-embed only new or changed
  messages; delete messages removed from a changed source.
- Reconcile missing sources only after discovery and parsing establish a
  complete successful scan for that provider. Any incomplete provider scan
  suppresses purging for that provider.
- Store forget tombstones by provider plus stable external session identity in
  the canonical database. Forget deletes canonical and derived rows in one
  transaction; indexing checks the tombstone before insertion.
- Use SQLite FTS5 BM25 for lexical retrieval.
- Store one embedding per searchable message and use exact cosine search before
  any benchmark-driven ANN discussion.
- Store a deterministic generation hash with every embedding. The hash covers
  the FastEmbed model identity and vector serialization schema; search filters
  to the current hash and indexing deletes mismatches before selecting messages
  for bounded re-embedding.
- Fuse lexical and semantic ranks with a small RRF implementation and rerank a
  bounded candidate set with one mainstream maintained model backend.
- Keep the model backend concrete: FastEmbed MiniLM-class embeddings and a
  FastEmbed cross-encoder reranker.
- Put the concrete backend behind one default-enabled Cargo feature:
  `default = ["semantic"]` and `semantic = ["dep:fastembed"]`. The
  no-default-features build retains the same commands and emits truthful
  lexical-only status rather than a second product surface.
- Embed added or changed messages in fixed bounded batches. A model load,
  embedding, or reranking failure becomes lexical fallback for that invocation;
  it never rolls back valid canonical/FTS indexing.
- Use thin LTO for ordinary release builds and publish full semantic binaries
  from CI. Do not make local cross-compilation part of the developer loop.
- Operational commands emit JSON; bare `cass` emits concise help.
- Rebuild derived search state from SQLite rather than repairing generations.

## Tooling Compatibility

- Implementation language: Rust.
- Native test runner: `cargo nextest`.
- Veritas claim attributes remain compiler-checked while normal verification
  uses cargo, nextest, clippy, formatting, and doctests.

## Risks

- Provider formats evolve. Keep representative fixtures for the retained JSONL,
  SQLite, and event formats; count malformed records without panicking.
- Exact vector search may become slow. Benchmark a representative corpus before
  adding any ANN dependency.
- Semantic assets may be missing. Search must continue lexically and report the
  realized mode truthfully.
- Holding the writer transaction during inference may delay other writers. The
  product permits one index writer; keep WAL readers available and revisit lock
  choreography only with measurements.
- Provider roots can be temporarily unavailable. Purging is gated on a complete
  successful provider scan so transient discovery or parse failures preserve
  committed histories.
- Deletion can accidentally retain dependency residue. Final gates scan the
  manifest, lockfile, source, build scripts, workflows, tests, and maintained
  documentation for prohibited dependency and removed-provider surfaces.

## Migration / Rollback

- Existing databases at a recognized earlier miniature schema version migrate
  forward in one transaction. Databases from the removed legacy application
  are outside the supported migration boundary and may be rebuilt by `index`.
- Each migration is monotonic and idempotent under `user_version`; no down
  migrations or salvage branches are added.
- Before release, both default-feature and no-default-feature test gates must
  pass. A release can roll back to the prior binary only while its database
  schema remains readable by that binary; otherwise the canonical database is
  rebuilt from source histories.
