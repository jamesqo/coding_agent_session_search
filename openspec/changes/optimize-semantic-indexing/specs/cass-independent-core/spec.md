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
- **WHEN** indexing stops after at least one derived embedding checkpoint but before current-generation coverage is complete
- **THEN** committed canonical and FTS state plus completed embedding checkpoints remain durable, search reports not ready, and the next index embeds only rows still missing current-generation vectors

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
