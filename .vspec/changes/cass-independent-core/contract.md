# Reviewed contract: cass-independent-core

Execution mode: direct. Gas City owns no part of this run.
Completion belongs to the normal verification gates for this handoff.

## Required behavior

- Combine SQLite FTS5 lexical retrieval with independent semantic retrieval.
- Ingest Claude Code and Codex JSONL histories through two concrete parsers.
- Remove all other product surfaces, providers, compatibility systems, and the
  legacy dependency stack.
- Expose exactly six JSON-first commands: `index`, `search`, `view`, `status`,
  `forget`, and `models install`.
- Store canonical conversations and messages in one current Rusqlite schema.

## Requirements

- cass-independent-core/size-boundary: The replacement SHALL contain no more than 70,000 lines of production Rust and 30,000 lines of Rust tests.
- cli/removed-commands-are-rejected: The system SHALL expose only `index`, `search`, `view`, `status`, `forget`, and `models install` as operational commands. Operational output SHALL be JSON.
- independence/no-dickles-franken-surface: The maintained project SHALL contain no runtime or build dependency on the legacy dependency ecosystem or removed-provider surface.
- ingestion/malformed-records-do-not-panic: The system SHALL discover and normalize Claude Code and Codex JSONL histories into CASS-owned canonical conversation and message records.
- models/download-is-explicit: The system SHALL support semantic candidates, RRF fusion, and cross-encoder reranking using a mainstream maintained model backend.
- search/lexical-returns-distinctive-match: The system SHALL retrieve lexical candidates with SQLite FTS5 BM25 and apply supported provider and recency filters.
- status/missing-database-recommends-index: The system SHALL report database readiness, indexed counts, model readiness, and realized fallback state as JSON without repair planning machinery.
- storage/forget-removes-conversation: The system SHALL use one Rusqlite database and one current schema as canonical state. Search indexes and embeddings SHALL be rebuildable derived state.
- view/context-clamps-to-conversation: The system SHALL hydrate a stable result identifier and return bounded adjacent messages through `view --context N`.

## Non-goals

- Compatibility shims, multiple schemas or storage backends, salvage systems.
- Preserving old CLI commands or presentation contracts.
- Providers other than Claude Code and Codex.
- TUI, HTML or transcript export, analytics, remote sync, watch mode, daemons.

## Acceptance criteria

- cass-independent-core/size-boundary/scenario/final-accounting: Final accounting: - **WHEN** retained Rust source is counted at final integration - **THEN** both ceilings pass without relocating behavior into generated code or scripts
- cli/removed-commands-are-rejected/scenario/bare-invocation: Bare invocation: - **WHEN** `cass` is invoked without a command - **THEN** it prints concise help without launching an interactive interface
- cli/removed-commands-are-rejected/scenario/removed-command: Removed command: - **WHEN** a removed command such as `export`, `doctor`, `list`, or `sources` is invoked - **THEN** argument parsing fails without compatibility rewriting
- independence/no-dickles-franken-surface/scenario/dependency-scan: Dependency scan: - **WHEN** final manifests, lockfiles, build scripts, source, tests, workflows, and maintained documentation are scanned - **THEN** prohibited dependency and removed-provider surfaces are absent
- ingestion/malformed-records-do-not-panic/scenario/malformed-line: Malformed line: - **WHEN** a supported JSONL file contains a malformed record - **THEN** indexing returns a typed JSON error or bounded diagnostic without panicking
- ingestion/malformed-records-do-not-panic/scenario/supported-histories: Supported histories: - **WHEN** configured roots contain representative Claude Code and Codex JSONL histories - **THEN** `cass index` persists their conversations and messages with stable IDs
- ingestion/malformed-records-do-not-panic/scenario/unsupported-provider: Unsupported provider: - **WHEN** a root contains only another provider's history - **THEN** no conversation from that provider is indexed or advertised as supported
- models/download-is-explicit/scenario/explicit-installation: Explicit installation: - **WHEN** models are absent and no `models install` command is run - **THEN** CASS does not download models implicitly
- models/download-is-explicit/scenario/models-absent: Models absent: - **WHEN** semantic models are not installed - **THEN** search succeeds lexically and truthfully reports lexical fallback
- models/download-is-explicit/scenario/models-installed: Models installed: - **WHEN** compatible embedding and reranking models are installed - **THEN** search reports hybrid realization and returns fused, reranked candidates
- search/lexical-returns-distinctive-match/scenario/matching-query: Matching query: - **WHEN** an indexed message contains a distinctive query term - **THEN** `cass search` returns that message with a stable identifier and lexical score metadata
- status/missing-database-recommends-index/scenario/missing-database: Missing database: - **WHEN** no canonical database exists - **THEN** `status` reports not-ready state and `index` as the direct action
- storage/forget-removes-conversation/scenario/forget: Forget: - **WHEN** `cass forget <id>` succeeds - **THEN** the conversation, its messages, and derived search rows are no longer retrievable
- storage/forget-removes-conversation/scenario/full-rebuild: Full rebuild: - **WHEN** `cass index --full` runs against an existing canonical database - **THEN** derived lexical and semantic state is recreated without duplicating canonical messages
- view/context-clamps-to-conversation/scenario/context-boundary: Context boundary: - **WHEN** requested context extends before the first or after the last message - **THEN** `view` returns only existing messages in canonical order
