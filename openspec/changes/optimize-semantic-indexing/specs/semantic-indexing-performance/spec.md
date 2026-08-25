## Purpose

Define observable throughput, progress, and interruption behavior for CASS's
concrete semantic indexing backend without changing retrieval semantics.

## ADDED Requirements

### Requirement: Embedding work preserves semantic results

CASS SHALL store one current-generation embedding for every searchable message.
The canonical length-aware batching policy SHALL produce deterministic
quantized vectors for the same corpus regardless of message identifier, input
order, or duplicate occurrences. On representative reference-path samples,
each new-policy vector SHALL have at least 0.98 quantized cosine similarity to
its reference vector and SHALL preserve the expected relevant results in the
retrieval fixture.

#### Scenario: Repeated text

<!-- claim: semantic-indexing/repeated-text-reuses-inference -->
- **WHEN** several embedding-missing messages have identical searchable text
- **THEN** one index run stores a current vector for every message while its progress reports fewer model inferences than stored vectors

#### Scenario: Mixed message lengths

<!-- claim: semantic-indexing/batching-preserves-vectors -->
- **WHEN** representative short and long searchable messages are compared between the reference and canonical length-aware policies
- **THEN** each vector has at least 0.98 quantized cosine similarity, repeated canonical runs are byte-deterministic, and the retrieval fixture retains its expected relevant results

### Requirement: Embedding progress is visible

During semantic embedding, `cass index` SHALL periodically emit newline-delimited
JSON progress to standard error. Each embedding progress object SHALL report the
phase, completed stored vectors, total missing vectors at phase start, actual
model inferences, reused duplicate vectors, elapsed milliseconds, and current
stored-vectors-per-second rate. Progress SHALL be monotonic within one run and
SHALL NOT change the single final JSON response on standard output.

#### Scenario: Long cold index

<!-- claim: semantic-indexing/progress-is-monotonic -->
- **WHEN** a cold index requires more searchable vectors than one inference batch
- **THEN** standard error contains monotonic embedding progress through the total and standard output contains exactly one final index response

### Requirement: Cold embedding throughput is measured

The repository SHALL provide a reproducible, explicitly invoked benchmark over
the measured Xenia text-length and duplication shape. The accepted implementation
SHALL record at least four times the 55 stored-vectors-per-second baseline while
meeting the vector-similarity and retrieval-relevance requirements above. The
benchmark SHALL NOT run in the ordinary test or CI gate.

#### Scenario: Candidate batching policy is accepted

<!-- claim: semantic-indexing/cold-throughput-target -->
- **WHEN** the focused benchmark is run on Xenia against the recorded representative corpus shape
- **THEN** it reports at least 220 stored vectors per second, at least 0.98 reference-vector cosine similarity, and retained retrieval-fixture relevance
