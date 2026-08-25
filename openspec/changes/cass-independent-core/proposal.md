## Why

Replace CASS in place with a completely independent local-history search
application using Rusqlite, SQLite FTS5, semantic retrieval, fusion, and
reranking. Keep indexing durable and incremental so routine refreshes do not
rebuild unchanged work or resurrect deliberately forgotten sessions.

## What Changes

- Keep exactly six JSON-first commands: `index`, `search`, `view`, `status`,
  `forget`, and `models install`.
- Ingest Claude Code, Codex, current OpenCode, GitHub Copilot CLI, Hermes Agent,
  and Pi histories through concrete CASS-owned parsers.
- Store canonical conversations and messages in one current Rusqlite schema.
- Combine SQLite FTS5 lexical retrieval with FastEmbed semantic retrieval,
  reciprocal-rank fusion, and bounded cross-encoder reranking.
- Ship semantic retrieval in default release binaries while preserving a
  supported lexical-only build for fast development.
- Incrementally index changed sources and messages, persist forget tombstones,
  purge histories absent from complete scans, serialize index writers, and
  migrate supported schema versions forward.
- Publish full prebuilt release binaries through CI.
- Remove all other provider surfaces, compatibility systems, and the complete
  legacy dependency stack.

## Capabilities

### New Capabilities

- `cass-independent-core`: Index and retrieve the six supported local history
  formats through a small, independent, machine-readable command-line
  application.

### Modified Capabilities

None.

## Success Boundary

The retained commands pass their behavioral scenarios; incremental refresh,
durable forgetting, complete-scan purging, schema migration, semantic search,
and truthful lexical fallback work; supported histories can be indexed,
viewed, and forgotten; full release binaries are produced by CI; no maintained
legacy dependency or removed-provider surface remains; production Rust is at
most 70,000 lines and test Rust is at most 30,000 lines.

## Non-Goals

- Providers other than Claude Code, Codex, current OpenCode, GitHub Copilot
  CLI, Hermes Agent, and Pi.
- Provider traits, registries, plugins, connector compatibility, or
  legacy/cloud/IDE/Oh My Pi history compatibility.
- TUI, HTML or transcript export, analytics, remote sync, watch mode, daemons,
  self-update, shell completion, human-oriented rendering, or alternate output
  encodings.
- Compatibility shims, multiple storage backends, salvage systems,
  CASS-owned ANN indexes, plugin registries, or provider abstractions.
- Preserving old CLI commands or presentation contracts.

## Impact

This is an in-place replacement of CASS. Existing source, tests, workflows,
scripts, documentation, assets, and dependencies outside the success boundary
are deleted or made inert. Git history and the external reference patch remain
available as implementation references.
