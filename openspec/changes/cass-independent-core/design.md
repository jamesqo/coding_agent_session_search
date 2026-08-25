## Context

CASS now serves one narrow need: local search over Claude Code and Codex JSONL
histories. There are no backward-compatibility requirements for older product
surfaces. SQLite is canonical; search artifacts are derived.

## Decisions

- Use two concrete ingestion paths returning CASS-owned `Conversation` and
  `Message` values: Claude Code JSONL and Codex JSONL.
- Configure local discovery through built-in roots plus `CASS_CLAUDE_ROOTS` and
  `CASS_CODEX_ROOTS`; do not add index-time provider root flags.
- Use Rusqlite as the sole database library and one current schema.
- Use SQLite FTS5 BM25 for lexical retrieval.
- Store one embedding per searchable message and use exact cosine search before
  any benchmark-driven ANN discussion.
- Fuse lexical and semantic ranks with a small RRF implementation and rerank a
  bounded candidate set with one mainstream maintained model backend.
- Keep the model backend concrete: FastEmbed MiniLM-class embeddings and a
  FastEmbed cross-encoder reranker.
- Operational commands emit JSON; bare `cass` emits concise help.
- Rebuild derived search state from SQLite rather than repairing generations.

## Tooling Compatibility

- Implementation language: Rust.
- Native test runner: `cargo nextest`.
- Veritas claim attributes remain compiler-checked while normal verification
  uses cargo, nextest, clippy, formatting, and doctests.

## Risks

- Provider formats evolve. Keep representative fixtures for the two retained
  JSONL formats and count malformed records without panicking.
- Exact vector search may become slow. Benchmark a representative corpus before
  adding any ANN dependency.
- Semantic assets may be missing. Search must continue lexically and report the
  realized mode truthfully.
- Deletion can accidentally retain dependency residue. Final gates scan the
  manifest, lockfile, source, build scripts, workflows, tests, and maintained
  documentation for prohibited dependency and removed-provider surfaces.
