## MODIFIED Requirements

### Requirement: Incremental indexing lifecycle

An ordinary `cass index` SHALL fingerprint sources and normalized messages, skip unchanged sources, and update canonical rows and semantic embeddings only for added, changed, or removed messages. For each writer transaction, CASS SHALL apply row-level FTS mutations below a declared, benchmark-selected changed-row cutoff and MAY rebuild whole-corpus FTS at or above that cutoff. Either strategy SHALL finish in the same transaction as its canonical changes. `index --full` SHALL retain an explicit whole-run bulk path. A failed or incomplete scan SHALL preserve previously indexed histories.

#### Scenario: Unchanged source

<!-- claim: indexing/unchanged-source-is-skipped -->
- **WHEN** `cass index` sees a source whose fingerprint is unchanged
- **THEN** it reports the source unchanged without parsing it or rewriting its messages, FTS rows, or embeddings

#### Scenario: Small changed-message transaction

<!-- claim: indexing/only-changed-messages-refresh -->
- **WHEN** a writer transaction contains fewer changed or removed messages than the declared FTS rebuild cutoff and the embedding generation is unchanged
- **THEN** only those messages receive canonical and FTS mutations and only added or changed messages receive new embeddings

#### Scenario: Large changed-message transaction

- **WHEN** a writer transaction reaches the declared FTS rebuild cutoff and the embedding generation is unchanged
- **THEN** CASS may rebuild complete FTS state before committing that transaction while generating embeddings only for added or changed searchable messages

#### Scenario: Transaction is interrupted

<!-- claim: indexing/canonical-and-fts-are-atomic -->
- **WHEN** a writer transaction fails before its checkpoint commits
- **THEN** neither that transaction's canonical changes nor its row-level or rebuilt FTS state is visible after rollback

#### Scenario: Incremental and bulk FTS strategies agree

- **WHEN** the same canonical corpus is maintained once through row-level FTS mutations and once through a bulk FTS rebuild
- **THEN** both databases expose the same searchable message identifiers and query results

#### Scenario: Explicit full rebuild

<!-- claim: indexing/full-rebuild-is-explicit -->
- **WHEN** `cass index --full` is invoked
- **THEN** CASS may rebuild whole-corpus FTS and semantic derived state while preserving canonical message identity

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

### Requirement: Lexical retrieval

SQLite FTS5 lexical retrieval SHALL remain a candidate source inside hybrid search and SHALL apply supported provider and recency filters. CASS SHALL NOT expose lexical-only search as a successful production realization.

#### Scenario: Lexical candidate contributes to ready hybrid search

<!-- claim: search/fts-contributes-to-hybrid -->
- **WHEN** current semantic models and embeddings are ready and an indexed message contains a distinctive query term
- **THEN** `cass search` may retrieve that message through FTS5, includes it in fusion and reranking, and reports hybrid realization

### Requirement: Semantic retrieval and reranking

CASS search SHALL require the concrete semantic backend, compatible installed embedding and reranking models, and current stored embeddings for every searchable message. Successful search SHALL perform semantic candidate retrieval, FTS5 candidate retrieval, RRF fusion, and bounded cross-encoder reranking. Each stored vector SHALL carry a deterministic identity for its embedding model and vector schema; vectors with a different identity SHALL never participate in search.

#### Scenario: Semantic search is ready

<!-- claim: semantic/hybrid-reranks-with-models -->
- **WHEN** compatible models are installed and every searchable message has a current-generation embedding
- **THEN** `cass search` reports hybrid realization and returns fused, reranked candidates

#### Scenario: Models are absent

<!-- claim: semantic/missing-models-fail-search -->
- **WHEN** semantic models are not installed
- **THEN** `cass search` exits unsuccessfully with JSON `error.kind` equal to `model`, `error.recommended_action` equal to `models install`, no results or fallback fields, and no lexical results

#### Scenario: Index starts without models

<!-- claim: semantic/index-requires-models -->
- **WHEN** semantic models are not installed and `cass index` is invoked
- **THEN** indexing exits unsuccessfully with JSON `error.kind` equal to `model` and `error.recommended_action` equal to `models install` before opening or creating the database writer or parsing source content

#### Scenario: Semantic index is incomplete

<!-- claim: semantic/missing-embeddings-fail-search -->
- **WHEN** models are installed but any searchable message lacks a current-generation embedding
- **THEN** `cass search` exits unsuccessfully with JSON `error.kind` equal to `search-not-ready`, `error.recommended_action` equal to `index`, and no results or fallback fields

#### Scenario: Model load or inference fails

<!-- claim: semantic/inference-failure-fails-search -->
- **WHEN** installed semantic assets cannot load or embedding or reranking inference fails
- **THEN** `cass search` exits unsuccessfully with JSON `error.kind` equal to `model`, an applicable explicit recommended action, no results or fallback fields, and never returns lexical fallback results

#### Scenario: Embedding generation changes

<!-- claim: semantic/stale-embedding-generation-invalidated -->
- **WHEN** the configured embedding model or stored-vector schema identity changes
- **THEN** old vectors are excluded immediately and search remains unavailable until `cass index` replaces them with current-generation vectors

#### Scenario: Explicit installation

<!-- claim: models/download-is-explicit -->
- **WHEN** models are absent and no `models install` command is run
- **THEN** `index`, `search`, and `status` do not download model assets

### Requirement: Semantic release and lexical development builds

Every supported CASS binary SHALL include the concrete semantic backend. Cargo feature selection, including `--no-default-features`, SHALL NOT create a supported lexical-only runtime realization.

#### Scenario: Supported binary is built

<!-- claim: distribution/every-build-includes-semantic -->
- **WHEN** CI or a developer builds CASS with any supported Cargo invocation
- **THEN** the resulting binary includes semantic retrieval and enforces semantic readiness before search

### Requirement: Status truthfulness

`cass status` SHALL report canonical database counts, model installation, current embedding coverage, and whether search is ready. It SHALL recommend exactly the next explicit operational command required for semantic search without downloading assets or changing storage.

#### Scenario: Models are missing

<!-- claim: status/missing-models-recommends-install -->
- **WHEN** compatible models are not installed, whether or not the canonical database exists
- **THEN** `status` reports search not ready and recommends `models install`

#### Scenario: Embeddings are missing or stale

<!-- claim: status/missing-embeddings-recommends-index -->
- **WHEN** compatible models are installed but current embedding coverage is incomplete
- **THEN** `status` reports search not ready and recommends `index`

#### Scenario: Semantic search is ready

<!-- claim: status/semantic-search-ready -->
- **WHEN** compatible models are installed and current embedding coverage equals the searchable message count
- **THEN** `status` reports search ready with hybrid realization and no recommended action

#### Scenario: Every canonical message is context-only

<!-- claim: status/zero-searchable-messages-can-be-ready -->
- **WHEN** compatible models are installed and the database contains canonical messages but zero searchable messages and zero current embeddings
- **THEN** `status` reports search ready with hybrid realization and no recommended action

#### Scenario: Database is missing

<!-- claim: status/missing-database-recommends-index -->
- **WHEN** compatible models are installed but no canonical database exists
- **THEN** `status` reports search not ready and recommends `index`

## ADDED Requirements

### Requirement: Tool-result content is context-only

CASS SHALL preserve the complete tool-result content retained under existing ingestion safety bounds in canonical messages for `view` while excluding those payloads from FTS candidates and semantic embeddings. A message containing both ordinary conversational text and tool-result blocks SHALL derive search text only from its non-tool-result content. Tool invocation metadata and ordinary assistant explanations are not tool-result payloads under this rule.

#### Scenario: Pure tool result

<!-- claim: search/tool-results-are-not-searchable -->
- **WHEN** a term occurs only inside a pure tool-result message
- **THEN** `cass search` does not return that message and semantic readiness does not require an embedding for it

#### Scenario: Mixed prose and tool result

<!-- claim: search/mixed-message-excludes-tool-result-text -->
- **WHEN** one canonical message contains conversational text and a structured tool-result block
- **THEN** its conversational text can participate in FTS and embeddings while terms unique to the tool-result block cannot

#### Scenario: View retained output

<!-- claim: view/tool-results-remain-visible -->
- **WHEN** `cass view` returns context containing a tool result excluded from search
- **THEN** the response contains that tool result's complete retained canonical content in conversation order

#### Scenario: Existing database is upgraded

<!-- claim: storage/tool-search-projection-migrates -->
- **WHEN** CASS opens a supported database created before tool-result search exclusion
- **THEN** it preserves canonical messages, invalidates derived search readiness, and requires re-indexing source histories before search can succeed
