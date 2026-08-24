## Purpose

Provide a small, independent JSON CLI that indexes and searches local Claude
Code and Codex conversations with lexical and semantic retrieval.

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
The system SHALL discover and normalize only Claude Code and Codex JSONL
histories into CASS-owned canonical conversation and message records.

#### Scenario: Supported histories

<!-- claim: ingestion/supported-jsonl-indexes -->
- **WHEN** configured roots contain representative Claude Code and Codex histories
- **THEN** `cass index` persists their conversations and messages with stable IDs

#### Scenario: Unsupported provider

<!-- claim: ingestion/unsupported-providers-ignored -->
- **WHEN** a root contains only an OpenCode or another removed-provider history
- **THEN** no conversation from that provider is indexed or advertised as supported

#### Scenario: Malformed line

<!-- claim: ingestion/malformed-records-do-not-panic -->
- **WHEN** a supported JSONL file contains a malformed record
- **THEN** indexing returns a typed JSON error or bounded diagnostic without panicking

### Requirement: Canonical storage

The system SHALL use one Rusqlite database and one current schema as canonical
state. Search indexes and embeddings SHALL be rebuildable derived state.

#### Scenario: Full rebuild

<!-- claim: storage/full-rebuild-is-idempotent -->
- **WHEN** `cass index --full` runs against an existing canonical database
- **THEN** derived lexical and semantic state is recreated without duplicating canonical messages

#### Scenario: Forget

<!-- claim: storage/forget-removes-conversation -->
- **WHEN** `cass forget <id>` succeeds
- **THEN** the conversation, its messages, and derived search rows are no longer retrievable

### Requirement: Lexical retrieval

The system SHALL retrieve lexical candidates with SQLite FTS5 BM25 and apply
supported provider and recency filters.

#### Scenario: Matching query

<!-- claim: search/lexical-returns-distinctive-match -->
- **WHEN** an indexed message contains a distinctive query term
- **THEN** `cass search` returns that message with a stable identifier and lexical score metadata

### Requirement: Semantic retrieval and reranking

The system SHALL support semantic candidates, RRF fusion, and cross-encoder
reranking using a maintained non-Dickles model backend.

#### Scenario: Models installed

<!-- claim: semantic/hybrid-reranks-with-models -->
- **WHEN** compatible embedding and reranking models are installed
- **THEN** search reports hybrid realization and returns fused, reranked candidates

#### Scenario: Models absent

<!-- claim: semantic/missing-models-fall-back -->
- **WHEN** semantic models are not installed
- **THEN** search succeeds lexically and truthfully reports lexical fallback

#### Scenario: Explicit installation

<!-- claim: models/download-is-explicit -->
- **WHEN** models are absent and no `models install` command is run
- **THEN** CASS does not download models implicitly

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
Dickles/Franken ecosystem.

#### Scenario: Dependency scan

<!-- claim: independence/no-dickles-franken-surface -->
- **WHEN** final manifests, lockfiles, build scripts, source, tests, workflows,
  and maintained documentation are scanned
- **THEN** FAD, Frankensearch, Frankensqlite/fsqlite, Frankentorch, Asupersync,
  FrankenTUI, TOON/tru, the Dickles HNSW fork, and Dickles git pins are absent

### Requirement: Size boundary

The replacement SHALL contain no more than 70,000 lines of production Rust and
30,000 lines of Rust tests.

#### Scenario: Final accounting

- **WHEN** retained Rust source is counted at final integration
- **THEN** both ceilings pass without relocating behavior into generated code or scripts
