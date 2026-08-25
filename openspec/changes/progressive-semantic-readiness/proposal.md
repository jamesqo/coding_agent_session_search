## Why

CASS can ingest gigabytes of JSONL in seconds, but a first semantic backfill can
take minutes. The current all-or-nothing readiness rule makes every search fail
until the last searchable message has a current vector, even though thousands
of valid semantic vectors have already committed. On dev-macbook this turned a
9m19s cold index into a 9m19s search outage while ordinary warm refreshes took
only 3.68s.

## What Changes

- Make semantic search available over the current-generation covered subset as
  soon as at least one searchable message has a committed vector.
- Keep lexical candidates, semantic candidates, fusion, and reranking bounded
  to that same covered subset; partial readiness never becomes lexical fallback.
- Generate missing vectors in deterministic newest-first order and retain the
  existing durable checkpoint/resume behavior.
- Report serving generation, completed vectors, pending vectors, total
  searchable messages, and complete-versus-partial coverage in `status` and
  successful `search` responses.
- Preserve a fully covered, query-compatible serving generation while a new
  compatible generation is built, then switch generations atomically after the
  replacement reaches complete coverage.
- Keep zero-vector non-empty databases unavailable and preserve typed model and
  inference failures.

## Capabilities

### New Capabilities

- `progressive-semantic-readiness`: Partial semantic coverage, newest-first
  backfill, truthful coverage reporting, resumability, and compatible-generation
  rollover.

### Modified Capabilities

- `cass-independent-core`: Semantic search and status readiness no longer
  require current-generation vectors for every searchable message before any
  successful search is possible.

## Success Boundary

- After the first embedding checkpoint, search succeeds semantically and cannot
  return a message without a vector in the reported serving generation.
- Recent messages become searchable before older missing messages, with stable
  ordering and resumability across interruption.
- Coverage counters are internally consistent and reach complete readiness when
  every searchable message has a serving-generation vector.
- A complete compatible old generation remains searchable during replacement
  and switches atomically only after the new generation is complete.
- Existing fully indexed databases remain fully ready after migration.

## Non-Goals

- Lexical-only fallback, partial results containing unembedded messages, or
  weakening model/inference failures.
- A daemon, watch mode, background service, scheduler framework, or new command.
- Changing the embedding model, quantization, exact cosine search, RRF,
  reranking, or relevance policy.
- Retaining or loading multiple incompatible embedding models. An embedding
  model change whose query space is incompatible still requires the current
  model's backfill before that generation can serve.
- Configurable prioritization, coverage thresholds, batch sizes, or tuning
  constants.

## Impact

The change affects the Rusqlite schema and migrations, embedding selection and
generation state, lexical/semantic candidate filtering, `index` sequencing,
and the JSON contracts for `status` and successful `search`. It adds no runtime
dependency and preserves the existing six-command surface and explicit model
installation boundary.
