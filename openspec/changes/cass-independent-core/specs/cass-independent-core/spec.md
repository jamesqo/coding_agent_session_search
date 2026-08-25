## Purpose

Provide a small, independent JSON CLI that indexes and searches local Claude
Code, Codex, current OpenCode, GitHub Copilot CLI, Hermes Agent, and Pi
conversations with lexical and semantic retrieval.

## ADDED Requirements

### Requirement: Command surface

<!-- claim: cli/operational-command-surface -->
The system SHALL expose only `index`, `search`, `view`, `status`, `forget`, and
`models install` as operational commands. Operational output SHALL be JSON.

#### Scenario: Bare invocation

<!-- claim: cli/bare-invocation-prints-help -->
- **WHEN** `cass` is invoked without a command
- **THEN** it prints concise help without launching an interactive interface

#### Scenario: Removed command

<!-- claim: cli/removed-commands-are-rejected -->
- **WHEN** a removed command such as `export`, `doctor`, `list`, or `sources` is invoked
- **THEN** argument parsing fails without compatibility rewriting

### Requirement: Provider boundary

<!-- claim: ingestion/provider-boundary -->
The system SHALL discover and normalize Claude Code, Codex, current OpenCode,
GitHub Copilot CLI, Hermes Agent, and Pi histories into CASS-owned canonical
conversation and message records.

#### Scenario: Supported histories

<!-- claim: ingestion/supported-jsonl-indexes -->
- **WHEN** configured roots contain representative histories from each of the six supported providers
- **THEN** `cass index` persists their conversations and messages with stable IDs

#### Scenario: Unsupported provider

<!-- claim: ingestion/unsupported-providers-ignored -->
- **WHEN** a root contains only another provider's history
- **THEN** no conversation from that provider is indexed or advertised as supported

#### Scenario: Malformed line

<!-- claim: ingestion/malformed-records-do-not-panic -->
- **WHEN** a supported JSONL file contains a malformed record
- **THEN** indexing returns a typed JSON error or bounded diagnostic without panicking

### Requirement: Canonical storage

The system SHALL use one Rusqlite database and one current schema as canonical
state. It SHALL migrate supported older schema versions forward and reject a
newer unknown schema with a typed JSON error. Search indexes and embeddings
SHALL be rebuildable derived state.

#### Scenario: Full rebuild

<!-- claim: storage/full-rebuild-is-idempotent -->
- **WHEN** `cass index --full` runs against an existing canonical database
- **THEN** derived lexical and semantic state is recreated without duplicating canonical messages

#### Scenario: Forget

<!-- claim: storage/forget-removes-conversation -->
- **WHEN** `cass forget <id>` succeeds
- **THEN** the conversation, its messages, and derived search rows are no longer retrievable and later indexing does not restore it

#### Scenario: Supported schema migration

<!-- claim: storage/supported-schema-migrates -->
- **WHEN** CASS opens a database at a supported older schema version
- **THEN** it applies the required forward migrations once and preserves canonical records

#### Scenario: Unknown newer schema

<!-- claim: storage/newer-schema-is-rejected -->
- **WHEN** CASS opens a database whose schema version is newer than the binary supports
- **THEN** it returns a typed JSON incompatibility error without modifying the database

### Requirement: Incremental indexing lifecycle

The system SHALL fingerprint sources and normalized messages so an ordinary
index refresh skips unchanged sources, updates only added or changed messages,
and removes canonical histories absent from a complete successful provider
scan. A failed or incomplete scan SHALL preserve previously indexed histories.

#### Scenario: Unchanged source

<!-- claim: indexing/unchanged-source-is-skipped -->
- **WHEN** `cass index` sees a source whose fingerprint is unchanged
- **THEN** it reports the source unchanged without rewriting its messages or embeddings

#### Scenario: Changed messages

<!-- claim: indexing/only-changed-messages-refresh -->
- **WHEN** a previously indexed source contains added or changed messages
- **THEN** only those messages require new canonical writes and semantic embeddings

#### Scenario: Source disappeared after complete scan

<!-- claim: indexing/complete-scan-purges-missing-source -->
- **WHEN** a complete successful provider scan no longer discovers a previously indexed source
- **THEN** its conversation, messages, FTS rows, and embeddings are removed

#### Scenario: Incomplete scan

<!-- claim: indexing/incomplete-scan-preserves-state -->
- **WHEN** provider discovery or parsing fails before a complete scan is established
- **THEN** previously indexed conversations are not purged as missing

#### Scenario: Forgotten source remains forgotten

<!-- claim: storage/forget-persists-through-indexing -->
- **WHEN** a forgotten source remains present during later indexing
- **THEN** its durable tombstone prevents the conversation from being reinserted

### Requirement: Single index writer

The system SHALL permit only one active index writer for a canonical database
while allowing searches to continue against committed state.

#### Scenario: Concurrent index attempt

<!-- claim: indexing/concurrent-writer-is-rejected -->
- **WHEN** a second `cass index` targets a database with an active index writer
- **THEN** it returns a typed JSON busy error without starting a competing refresh

### Requirement: Lexical retrieval

The system SHALL retrieve lexical candidates with SQLite FTS5 BM25 and apply
supported provider and recency filters.

#### Scenario: Matching query

<!-- claim: search/lexical-returns-distinctive-match -->
- **WHEN** an indexed message contains a distinctive query term
- **THEN** `cass search` returns that message with a stable identifier and lexical score metadata

### Requirement: Semantic retrieval and reranking

The system SHALL support semantic candidates, RRF fusion, and cross-encoder
reranking using a mainstream maintained model backend.

#### Scenario: Models installed

<!-- claim: semantic/hybrid-reranks-with-models -->
- **WHEN** compatible embedding and reranking models are installed
- **THEN** search reports hybrid realization and returns fused, reranked candidates

#### Scenario: Models absent

<!-- claim: semantic/missing-models-fall-back -->
- **WHEN** semantic models are not installed
- **THEN** search succeeds lexically and truthfully reports lexical fallback

#### Scenario: Inference failure

<!-- claim: semantic/inference-failure-falls-back -->
- **WHEN** installed semantic assets cannot load or inference fails
- **THEN** search still succeeds through FTS5 and reports the semantic failure and lexical realization

#### Scenario: Explicit installation

<!-- claim: models/download-is-explicit -->
- **WHEN** models are absent and no `models install` command is run
- **THEN** CASS does not download models implicitly

### Requirement: Semantic release and lexical development builds

Official release binaries SHALL include semantic retrieval. The supported
no-default-features development build SHALL compile and run the retained
commands lexically without linking the semantic model backend.

#### Scenario: Official release

<!-- claim: distribution/release-includes-semantic -->
- **WHEN** CI publishes an official CASS binary
- **THEN** the binary includes the concrete FastEmbed semantic backend

#### Scenario: Lexical-only build

<!-- claim: distribution/lexical-only-build-works -->
- **WHEN** CASS is built with `--no-default-features`
- **THEN** indexing and search work through FTS5 and truthfully report semantic support as unavailable

### Requirement: Context view

The system SHALL hydrate a stable result identifier and return bounded adjacent
messages through `view --context N`.

#### Scenario: Context boundary

<!-- claim: view/context-clamps-to-conversation -->
- **WHEN** requested context extends before the first or after the last message
- **THEN** `view` returns only existing messages in canonical order

### Requirement: Status truthfulness

The system SHALL report database readiness, indexed counts, model readiness,
and realized fallback state as JSON without repair planning machinery.

#### Scenario: Missing database

<!-- claim: status/missing-database-recommends-index -->
- **WHEN** no canonical database exists
- **THEN** `status` reports not-ready state and `index` as the direct action

### Requirement: Complete independence

The maintained project SHALL contain no runtime or build dependency on the
legacy dependency ecosystem and no active removed-provider surface.

#### Scenario: Dependency scan

<!-- claim: independence/no-dickles-franken-surface -->
- **WHEN** final manifests, lockfiles, build scripts, source, tests, workflows,
  and maintained documentation are scanned
- **THEN** prohibited legacy dependency surfaces and removed provider surfaces
  are absent

### Requirement: Size boundary

The replacement SHALL contain no more than 70,000 lines of production Rust and
30,000 lines of Rust tests.

#### Scenario: Final accounting

- **WHEN** retained Rust source is counted at final integration
- **THEN** both ceilings pass without relocating behavior into generated code or scripts
