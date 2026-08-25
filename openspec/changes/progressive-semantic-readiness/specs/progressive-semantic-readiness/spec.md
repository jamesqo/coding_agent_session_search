## Purpose

Define when a partially built semantic index may serve search, how coverage is
reported, and how CASS progresses from recent messages to complete coverage
without lexical fallback.

## ADDED Requirements

### Requirement: Partial coverage serves a semantic-only subset

CASS SHALL permit successful hybrid search when compatible models are installed
and at least one searchable message has a committed vector in the serving
generation. Lexical candidates, semantic candidates, fusion, and reranking
SHALL all be restricted to messages having a vector in that one serving
generation. CASS SHALL NOT return an uncovered message or report lexical
fallback.

#### Scenario: First checkpoint enables search

<!-- claim: progressive-readiness/partial-search-is-semantic -->
- **WHEN** a non-empty database has committed serving-generation vectors for some but not all searchable messages
- **THEN** `cass search` succeeds in hybrid mode, reports partial semantic coverage, and every returned message has a serving-generation vector

#### Scenario: No vector is ready

<!-- claim: progressive-readiness/zero-coverage-fails -->
- **WHEN** a non-empty database has no committed vector in a usable serving generation
- **THEN** `cass search` fails with JSON `error.kind` equal to `search-not-ready`, recommends `index`, and returns no lexical results

#### Scenario: Candidate sources share one coverage boundary

<!-- claim: progressive-readiness/results-use-one-generation -->
- **WHEN** an uncovered message is a stronger FTS match than every covered message
- **THEN** the uncovered message does not enter lexical candidates, fusion, reranking, or results for that search

### Requirement: Coverage is explicit and internally consistent

Successful `search` and database-backed `status` responses SHALL contain a
`semantic_coverage` object reporting the serving generation and vector count,
target generation and vector count, total searchable messages, pending target
vectors, and whether target coverage is complete. Counts SHALL describe
committed state visible to that command. A generation that does not exist SHALL
be represented as JSON `null`, not an invented identifier.

#### Scenario: Partial coverage is reported

<!-- claim: progressive-readiness/coverage-is-reported -->
- **WHEN** 60 of 100 searchable messages have target-generation vectors and that target is serving
- **THEN** `status` and successful `search` report 60 serving vectors, 60 target vectors, 100 searchable messages, 40 pending vectors, and incomplete coverage

#### Scenario: Coverage becomes complete

- **WHEN** the final missing target-generation vector commits
- **THEN** subsequent `status` and `search` responses report zero pending vectors and complete coverage

#### Scenario: Empty searchable corpus

- **WHEN** the database has zero searchable messages
- **THEN** `status` reports complete semantic coverage with zero serving, target, and pending vectors

#### Scenario: Federated nodes report independent coverage

<!-- claim: progressive-readiness/federated-coverage-is-node-local -->
- **WHEN** a federated search succeeds across nodes with different committed coverage
- **THEN** the response reports local coverage at the top level and each successful remote node's coverage in that node's outcome without summing the counts

### Requirement: Backfill makes recent history ready first

When several searchable messages need target-generation vectors, `cass index`
SHALL commit vectors in deterministic newest-first message order. Messages with
equal or absent timestamps SHALL use a stable tie-breaker. Exact duplicate text
MAY share one model inference, but every duplicate message SHALL receive its
vector in the checkpoint that first makes that text group visible.

#### Scenario: Index is interrupted after one checkpoint

<!-- claim: progressive-readiness/newest-first -->
- **WHEN** old and recent messages both lack target-generation vectors and indexing stops after its first embedding checkpoint
- **THEN** the committed covered subset contains the most recent deterministic prefix of missing messages, including every member of any committed duplicate-text group

#### Scenario: Index resumes

<!-- claim: progressive-readiness/interruption-resumes -->
- **WHEN** indexing resumes after a partial target-generation checkpoint
- **THEN** it preserves already committed vectors and continues with only the remaining missing messages in deterministic newest-first order

### Requirement: Compatible generation rollover is atomic

When a fully covered serving generation and a different query-compatible target
generation both exist, CASS SHALL continue searching only the serving generation
while target vectors build. It SHALL switch the serving generation atomically
only after the target generation covers every searchable message. Vectors from
different generations SHALL never be mixed in one search.

#### Scenario: Replacement is incomplete

<!-- claim: progressive-readiness/compatible-rollover-keeps-serving -->
- **WHEN** a fully covered compatible serving generation exists and replacement target coverage is incomplete
- **THEN** search continues against only the old serving generation while coverage reports the distinct incomplete target generation

#### Scenario: Replacement completes

<!-- claim: progressive-readiness/compatible-rollover-switches-atomically -->
- **WHEN** the replacement target generation reaches complete coverage
- **THEN** one committed state transition makes it the serving generation for subsequent searches without exposing a mixed-generation result set
