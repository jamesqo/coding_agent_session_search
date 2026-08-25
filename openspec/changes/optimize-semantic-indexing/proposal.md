## Why

Cold semantic indexing is now the dominant CASS cost. On Xenia, ingestion and
FTS completed in under one second while embedding 18,842 searchable messages
took 345.5 seconds, about 55 messages per second. The corpus also demonstrates
avoidable work: arbitrary identifier ordering mixes short and long messages in
the same padded transformer batch, and 29% of searchable rows repeat text that
is embedded again. A stopped run loses every embedding written by that run and
reports no embedding progress.

## What Changes

- Make cold embedding throughput length-aware while preserving one current
  semantic vector per searchable message.
- Reuse one inference result for identical searchable text within a run.
- Persist derived embeddings at bounded checkpoints so interruption can resume
  from committed coverage without exposing search as ready prematurely.
- Emit bounded JSON progress containing completed rows, total rows, inference
  count, reused count, rate, and elapsed time.
- Add a reproducible corpus benchmark for selecting batch size and recording
  cold and warm performance.

## Capabilities

### New Capabilities

- `semantic-indexing-performance`: Owns length-aware inference batching,
  duplicate reuse, resumable derived-state checkpoints, progress reporting, and
  benchmark acceptance.

### Modified Capabilities

- `cass-independent-core`: Semantic readiness continues to require exact
  current-generation coverage even when partial derived embeddings have been
  committed for resume.

## Success Boundary

- A checked representative Xenia corpus benchmark records at least a fourfold
  cold embedding throughput improvement over the measured 55 messages/second
  baseline without changing produced vectors or search results.
- Repeating `cass index` after a completed run remains below two seconds on the
  measured Xenia corpus when only a small active source changed.
- Terminating indexing after a committed embedding checkpoint leaves canonical
  messages and FTS consistent, reports search as not ready, and resumes by
  embedding only missing rows.
- Progress output makes a long embedding run visibly advance and exposes the
  difference between stored message vectors and actual model inferences.

## Non-Goals

- Replacing MiniLM with Word2Vec or another embedding model.
- Changing retrieval ranking, vector dimensions, quantization, or exact cosine
  search.
- Truncating canonical message content, chunking messages, or changing the
  source-file 90-day admission rule in this change.
- Adding GPU, daemon, ANN, async, provider, or runtime tuning frameworks.

## Impact

The change is internal to semantic indexing, SQLite derived embedding writes,
JSON index progress, tests, and focused performance measurement. Existing
databases remain compatible. Interrupted runs may retain partial derived
embedding coverage, but public search remains unavailable until coverage is
complete. No dependency or public command is added.
