## MODIFIED Requirements

### Requirement: Provider boundary

<!-- claim: ingestion/provider-boundary -->
The system SHALL discover and normalize only Claude Code and Codex histories
into CASS-owned canonical conversation and message records. With a loaded
configuration, provider presence in the resolved local node's `providers`
object SHALL enable that provider and its configured roots; absent providers
SHALL not be scanned. Without a loaded configuration, these two concrete
providers SHALL retain their built-in local roots. OpenCode, GitHub Copilot
CLI, Hermes Agent, Pi, and every other provider SHALL have no discovery or
configuration surface.

#### Scenario: Configured providers

<!-- claim: ingestion/configured-provider-roots-index -->
- **WHEN** the resolved local node enables Claude Code and Codex with valid roots
- **THEN** `cass index` scans those roots and does not scan absent providers

#### Scenario: Unsupported provider

<!-- claim: ingestion/unsupported-providers-ignored -->
- **WHEN** a configured supported root contains only another provider's history
- **THEN** no conversation from that provider is indexed or advertised as supported

#### Scenario: Malformed line

<!-- claim: ingestion/malformed-records-do-not-panic -->
- **WHEN** a supported history file contains a malformed record
- **THEN** indexing returns a typed JSON error or bounded diagnostic without panicking

## ADDED Requirements

### Requirement: Explicit index selection

<!-- claim: indexing/cli-provider-selection-is-bounded -->
`cass index` SHALL accept repeatable `--provider PROVIDER` values. Explicit
provider selection SHALL restrict that run to the deduplicated named supported
providers. Unknown providers or an explicitly selected provider without
configured or built-in roots SHALL fail with a typed JSON usage error before
scanning. CASS SHALL NOT expose an ad hoc root override.

#### Scenario: One provider is selected

- **WHEN** both Claude Code and Codex are enabled but indexing is invoked with `--provider codex`
- **THEN** only Codex roots are scanned during that run

#### Scenario: Provider selection is repeated

- **WHEN** `--provider codex --provider codex` is supplied
- **THEN** Codex is scanned once

### Requirement: Partial index safety

<!-- claim: indexing/partial-provider-scan-preserves-others -->
An index run restricted by `--provider` SHALL reconcile missing sources only
for providers actually scanned completely. It SHALL preserve
canonical conversations, FTS rows, embeddings, checkpoints, and tombstones for
all unselected providers.

#### Scenario: Codex-only refresh

- **WHEN** an existing database contains Claude Code and Codex histories and a complete Codex-only refresh succeeds
- **THEN** missing Codex sources may be purged while every Claude Code record remains unchanged

### Requirement: Configured roots are authoritative only when inspectable

<!-- claim: indexing/inaccessible-roots-never-authorize-purge -->
Every configured root for a selected provider SHALL be inspected successfully
before that provider's scan is considered complete. A missing, unreadable, or
disappearing configured root SHALL return a typed JSON error, mark the provider
scan incomplete, and preserve every existing canonical row, derived-search
row, checkpoint, and tombstone for that provider.

#### Scenario: Configured root becomes unavailable

- **WHEN** a selected provider has two configured roots and either root cannot be inspected
- **THEN** indexing fails without reconciling missing sources for that provider

### Requirement: Indexing recency horizon

<!-- claim: indexing/recency-horizon-bounds-admission-not-retention -->
The resolved recency horizon SHALL determine source eligibility using the
source file's modification time relative to one run-start timestamp. A source
whose modification time is on or after the inclusive cutoff SHALL be eligible;
an older source SHALL not be parsed, inserted, updated, or counted as missing.
When a source is eligible, CASS SHALL ingest its complete conversation rather
than truncate messages at the cutoff. With no horizon, every discovered source
SHALL be eligible.

<!-- claim: indexing/recency-exclusion-preserves-stored-state -->
Sources excluded only by the recency horizon SHALL never authorize deletion of
their existing canonical rows, FTS rows, embeddings, checkpoints, or
tombstones. The horizon is an admission/work limit, not an automatic retention
policy.

#### Scenario: Default boundary is inclusive

- **WHEN** a source modification time equals the cutoff computed once at run start
- **THEN** that source is eligible and its complete conversation is ingested

#### Scenario: Old indexed source is preserved

- **WHEN** a source already represented in SQLite is older than the resolved horizon
- **THEN** a later index run skips it without deleting or changing its stored state

#### Scenario: All history is selected

- **WHEN** configuration uses `since_days: null` or the CLI supplies `--all-history`
- **THEN** source age does not restrict discovery or reconciliation
