## Why

Replace CASS in place with a completely independent Claude Code, Codex,
OpenCode, GitHub Copilot CLI, and Hermes Agent search application using
Rusqlite, SQLite FTS5, semantic retrieval, fusion, and reranking.

## What Changes

- Replace the existing application with six JSON-first commands: `index`,
  `search`, `view`, `status`, `forget`, and `models install`.
- Ingest current Claude Code and Codex JSONL histories, OpenCode and Hermes
  SQLite histories, and GitHub Copilot CLI JSONL event logs.
- Store canonical conversations and messages in one current Rusqlite schema.
- Combine SQLite FTS5 lexical retrieval with independent semantic retrieval,
  reciprocal-rank fusion, and cross-encoder reranking.
- Remove all other product surfaces, providers, compatibility systems, and the
  complete Dickles/Franken dependency stack.

## Capabilities

### New Capabilities

- `cass-independent-core`: Index and retrieve Claude Code, Codex, OpenCode,
  GitHub Copilot CLI, and Hermes Agent histories through a small, independent,
  machine-readable command-line application.

### Modified Capabilities

None.

## Success Boundary

The retained commands pass their behavioral scenarios; semantic search and
truthful lexical fallback both work; current supported data can be indexed,
viewed, and forgotten; no maintained Dickles/Franken or removed-provider
surface remains; production Rust is at most 70,000 lines and test Rust is at
most 30,000 lines.

## Non-Goals

- Providers other than Claude Code, Codex, OpenCode, GitHub Copilot CLI, and
  Hermes Agent; legacy OpenCode/Hermes file storage and VS Code Copilot Chat
  storage are not part of this change.
- TUI, HTML or transcript export, analytics, remote sync, watch mode, daemons,
  self-update, shell completion, human-oriented rendering, or alternate output
  encodings.
- Compatibility shims, multiple schemas or storage backends, salvage systems,
  CASS-owned ANN/HNSW, plugin registries, or provider abstractions.
- Preserving old CLI commands or presentation contracts.

## Impact

This is an in-place replacement of CASS. Existing source, tests, workflows,
scripts, documentation, assets, and dependencies outside the success boundary
are deleted. Git history remains available as the reference implementation.
