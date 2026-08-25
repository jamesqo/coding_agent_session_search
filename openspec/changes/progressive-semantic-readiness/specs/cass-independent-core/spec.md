## MODIFIED Requirements

### Requirement: Incremental indexing lifecycle

An ordinary `cass index` SHALL fingerprint sources and normalized messages,
skip unchanged sources, and update canonical rows and semantic embeddings only
for added, changed, removed, stale, or embedding-missing messages. For each
canonical writer transaction, CASS SHALL apply row-level FTS mutations below a
declared, benchmark-selected changed-row cutoff and MAY rebuild whole-corpus FTS
at or above that cutoff. Either FTS strategy SHALL commit atomically with its
canonical changes. Semantic embeddings are rebuildable derived state and MAY
commit in bounded checkpoints after canonical and FTS state is durable.
`index --full` SHALL retain an explicit whole-run bulk path. A failed or
incomplete provider scan SHALL preserve previously indexed histories.

#### Scenario: Unchanged source

<!-- claim: indexing/unchanged-source-is-skipped -->
- **WHEN** `cass index` sees a source whose fingerprint is unchanged
- **THEN** it reports the source unchanged without parsing it or rewriting its messages, FTS rows, or current embeddings

#### Scenario: Small changed-message transaction

<!-- claim: indexing/only-changed-messages-refresh -->
- **WHEN** a writer transaction contains fewer changed or removed messages than the declared FTS rebuild cutoff and the embedding generation is unchanged
- **THEN** only those messages receive canonical and FTS mutations and only added, changed, stale, or embedding-missing searchable messages receive new embeddings

#### Scenario: Large changed-message transaction

- **WHEN** a writer transaction reaches the declared FTS rebuild cutoff and the embedding generation is unchanged
- **THEN** CASS may rebuild complete FTS state before committing that transaction while generating embeddings only for added, changed, stale, or embedding-missing searchable messages

#### Scenario: Canonical transaction is interrupted

<!-- claim: indexing/canonical-and-fts-are-atomic -->
- **WHEN** a canonical writer transaction fails before its checkpoint commits
- **THEN** neither that transaction's canonical changes nor its row-level or rebuilt FTS state is visible after rollback

#### Scenario: Embedding phase is interrupted

<!-- claim: indexing/partial-embeddings-resume -->
- **WHEN** indexing stops after at least one target-generation embedding checkpoint but before target coverage is complete
- **THEN** committed canonical and FTS state plus completed embedding checkpoints remain durable, covered semantic search remains available, and the next index embeds only target-generation rows still missing vectors

#### Scenario: Incremental and bulk FTS strategies agree

- **WHEN** the same canonical corpus is maintained once through row-level FTS mutations and once through a bulk FTS rebuild
- **THEN** both databases expose the same searchable message identifiers and query results at equal semantic coverage

#### Scenario: Explicit full rebuild

<!-- claim: indexing/full-rebuild-is-explicit -->
- **WHEN** `cass index --full` is invoked
- **THEN** CASS may rebuild whole-corpus FTS and semantic derived state while preserving canonical message identity and any usable serving generation until its replacement can serve

#### Scenario: Source disappeared after complete scan

<!-- claim: indexing/complete-scan-purges-missing-source -->
- **WHEN** a complete successful provider scan no longer discovers a previously indexed source
- **THEN** its conversation, messages, FTS rows, and embeddings are removed from serving and target coverage

#### Scenario: Incomplete scan

<!-- claim: indexing/incomplete-scan-preserves-state -->
- **WHEN** provider discovery or parsing fails before a complete scan is established
- **THEN** previously indexed conversations are not purged as missing

#### Scenario: Forgotten source remains forgotten

<!-- claim: storage/forget-persists-through-indexing -->
- **WHEN** a forgotten source remains present during later indexing
- **THEN** its durable tombstone prevents the conversation from being reinserted

### Requirement: Semantic retrieval and reranking

CASS search SHALL require the concrete semantic backend and compatible installed
embedding and reranking models. Successful search SHALL perform semantic
candidate retrieval, FTS5 candidate retrieval, RRF fusion, and bounded
cross-encoder reranking over only messages covered by one serving embedding
generation. Each stored vector SHALL carry a deterministic identity for its
embedding model and vector schema; vectors outside the serving generation SHALL
never participate in that search.

#### Scenario: Partial semantic search is ready

<!-- claim: semantic/partial-coverage-reranks-with-models -->
- **WHEN** compatible models are installed and at least one searchable message has a serving-generation embedding
- **THEN** `cass search` reports hybrid realization, reports complete or partial coverage, and returns only fused, reranked covered candidates

#### Scenario: Models are absent

<!-- claim: semantic/missing-models-fail-search -->
- **WHEN** semantic models are not installed
- **THEN** `cass search` exits unsuccessfully with JSON `error.kind` equal to `model`, `error.recommended_action` equal to `models install`, no results or fallback fields, and no lexical results

#### Scenario: Index starts without models

<!-- claim: semantic/index-requires-models -->
- **WHEN** semantic models are not installed and `cass index` is invoked
- **THEN** indexing exits unsuccessfully with JSON `error.kind` equal to `model` and `error.recommended_action` equal to `models install` before opening or creating the database writer or parsing source content

#### Scenario: Semantic index has zero coverage

<!-- claim: semantic/zero-coverage-fails-search -->
- **WHEN** models are installed and searchable messages exist but no usable serving-generation vector is committed
- **THEN** `cass search` exits unsuccessfully with JSON `error.kind` equal to `search-not-ready`, `error.recommended_action` equal to `index`, and no results or fallback fields

#### Scenario: Model load or inference fails

<!-- claim: semantic/inference-failure-fails-search -->
- **WHEN** installed semantic assets cannot load or embedding or reranking inference fails
- **THEN** `cass search` exits unsuccessfully with JSON `error.kind` equal to `model`, an applicable explicit recommended action, no results or fallback fields, and never returns lexical fallback results

#### Scenario: Compatible embedding generation changes

<!-- claim: semantic/compatible-generation-rolls-over -->
- **WHEN** the target vector generation changes without changing its query embedding space and a complete serving generation exists
- **THEN** old vectors remain the exclusive serving set until complete target coverage commits and atomically replaces them

#### Scenario: Explicit installation

<!-- claim: models/download-is-explicit -->
- **WHEN** models are absent and no `models install` command is run
- **THEN** `index`, `search`, and `status` do not download model assets

### Requirement: Status truthfulness

`cass status` SHALL report canonical database counts, model installation,
serving and target embedding coverage, and whether semantic search can serve at
least one covered message. It SHALL recommend exactly the next explicit command
needed to make search available or complete target coverage without downloading
assets or changing storage.

#### Scenario: Models are missing

<!-- claim: status/missing-models-recommends-install -->
- **WHEN** compatible models are not installed, whether or not the canonical database exists
- **THEN** `status` reports search not ready and recommends `models install`

#### Scenario: Coverage is zero

<!-- claim: status/zero-coverage-recommends-index -->
- **WHEN** compatible models are installed and searchable messages exist but no usable serving-generation vector is committed
- **THEN** `status` reports search not ready, reports zero serving coverage, and recommends `index`

#### Scenario: Coverage is partial

<!-- claim: status/partial-coverage-is-ready -->
- **WHEN** compatible models are installed and at least one but fewer than all target vectors are committed without a complete older serving generation
- **THEN** `status` reports search ready in hybrid mode, reports partial serving and target coverage, and recommends `index` to complete coverage

#### Scenario: Replacement generation is building

<!-- claim: status/rollover-distinguishes-generations -->
- **WHEN** a complete compatible serving generation exists while a distinct target generation is incomplete
- **THEN** `status` reports search ready, identifies both generations and their counts, and recommends `index` to complete replacement

#### Scenario: Semantic coverage is complete

<!-- claim: status/semantic-search-ready -->
- **WHEN** compatible models are installed and target-generation coverage equals the searchable message count
- **THEN** `status` reports search ready with hybrid realization, complete coverage, and no recommended action

#### Scenario: Every canonical message is context-only

<!-- claim: status/zero-searchable-messages-can-be-ready -->
- **WHEN** compatible models are installed and the database contains canonical messages but zero searchable messages and zero current embeddings
- **THEN** `status` reports search ready with hybrid realization, complete zero coverage, and no recommended action

#### Scenario: Database is missing

<!-- claim: status/missing-database-recommends-index -->
- **WHEN** compatible models are installed but no canonical database exists
- **THEN** `status` reports search not ready and recommends `index`

