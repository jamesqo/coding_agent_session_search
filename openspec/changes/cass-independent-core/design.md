## Context

CASS has accumulated roughly half a million lines of production Rust around a
small core need: searching local coding-agent histories. There are no external
users or backward-compatibility requirements. The replacement stays in this
repository, retains Claude Code, Codex, current OpenCode, GitHub Copilot CLI,
and Hermes Agent histories, and must be independent of the Dickles/Franken
ecosystem. SQLite is canonical; search artifacts are derived.

## Decisions

- Use five concrete ingestion paths returning CASS-owned `Conversation` and
  `Message` values: Claude Code JSONL, Codex JSONL, current OpenCode SQLite,
  GitHub Copilot CLI JSONL events, and current Hermes SQLite. Reject connector
  traits, registries, copied FAD code, legacy file stores, and VS Code Copilot
  Chat storage.
- Use Rusqlite as the sole database library and one current schema. Reject
  Frankensqlite, dual backends, migration museums, and salvage bridges.
- Use SQLite FTS5 BM25 for lexical retrieval. Reject a second lexical index and
  complex index-publication lifecycle machinery.
- Store one embedding per searchable message/document and start with measured
  exact cosine search. Reject ANN until a benchmark demonstrates a need.
- Fuse lexical and semantic ranks with a small RRF implementation and rerank a
  bounded candidate set with one mainstream non-Dickles model backend.
- Keep the model backend concrete. Select it with a time-boxed portability spike
  covering Linux amd64 and macOS arm64; reject a model registry or daemon.
- Use Tokio only for retained I/O that benefits from async execution. Keep
  parsing, storage, and search synchronous unless a concrete API requires it.
- Operational commands emit JSON; bare `cass` emits concise help. Reject human
  versus robot modes, aliases, corrections, and schema-discovery APIs.
- Rebuild derived search state from SQLite rather than repairing generations.

## Tooling Compatibility

- Implementation language: Rust.
- Native test runner: `cargo nextest 0.9.143` for parallel execution.
- Veritas evidence producer: `rust-test`; it discovers claim annotations while
  nextest executes the compiled Rust tests.
- No cross-language evidence bridge is used.
- Codex and Claude Code are configured with project-scoped Veritas MCP servers.
- Until host reload exposes MCP in existing sessions, project-bound `vtas` CLI
  is the approved fallback.

## Risks

- Current database rows may not map cleanly to the selected schema. Test opening
  a representative current database before cutover and report unsupported data
  explicitly rather than silently discarding it.
- Provider formats evolve. Keep representative fixtures for all five retained
  providers and fail malformed records without panicking. Current OpenCode and
  Hermes schemas plus Copilot CLI event-log assumptions are bounded explicitly
  rather than expanded into legacy compatibility machinery.
- Exact vector search may become slow. Benchmark a representative corpus before
  adding any ANN dependency.
- A mainstream model backend may fail on a deployment target. The portability
  spike must load both models and execute one embedding and one rerank on Linux
  amd64 and macOS arm64 before it is selected.
- Semantic assets may be missing. Search must continue lexically and report the
  realized mode truthfully.
- Deletion can accidentally retain dependency residue. Final gates scan the
  manifest, lockfile, source, build scripts, workflows, tests, and maintained
  documentation for the prohibited dependency and provider surfaces.
