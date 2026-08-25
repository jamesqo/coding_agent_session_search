# CASS Independent Core Retarget Plan

Status: implemented
Spec: `specs/cass-independent-core/spec.md`
Delivery: retargeted from canonical `954ec24f`

## Scope

Replace the active CASS surface with a small Rust JSON CLI that indexes exactly
Claude Code and Codex JSONL histories, stores canonical records in Rusqlite,
searches with SQLite FTS5 plus optional FastEmbed semantic retrieval and
reranking, and exposes only `index`, `search`, `view`, `status`, `forget`, and
`models install`.

Provider traits, registries, external application database parsing,
event-log ingestion, compatibility layers, daemons, alternate output modes, and
legacy dependency surfaces are outside the boundary.

## Current Implementation

- `app/cli.rs`: Clap command surface, JSON response boundary, supported-provider
  filtering, status, model installation, view, forget, and search dispatch.
- `app/ingestion.rs`: two concrete JSONL parsers for Claude Code and Codex plus
  built-in roots configurable through `CASS_CLAUDE_ROOTS` and
  `CASS_CODEX_ROOTS`.
- `app/storage.rs`: one Rusqlite schema, FTS5, canonical writes, context view,
  deletion, and embedding persistence.
- `app/semantic.rs`: FastEmbed MiniLM-class embeddings, FastEmbed reranking,
  exact cosine candidate search, RRF, and bounded reranking.
- `app/tests/cli_contract.rs`: black-box CLI behavior, parser, fallback,
  removed-surface, and maintained-boundary tests.

## Retarget Notes

The prior standalone `src/standalone_*` lane was superseded because it kept the
Claude/Codex-only boundary but used a non-mainstream local-vector backend
instead of the required mainstream embedding/reranking backend. It has been
preserved outside the repository at:

```text
/home/james/scratch/cass-reference-patches/adpc-superseded-standalone-lane-20260825.patch
```

The retarget keeps the current canonical FastEmbed implementation and removes
the active provider expansion from CLI, ingestion, storage, tests, README,
AGENTS, and OpenSpec/VSpec text artifacts.

## Verification

Required gates:

```bash
env TMPDIR=/home/james/scratch/cass-tmp \
  CARGO_TARGET_DIR=/home/james/scratch/cass-targets/integrated \
  CARGO_INCREMENTAL=0 cargo nextest run

env TMPDIR=/home/james/scratch/cass-tmp \
  CARGO_TARGET_DIR=/home/james/scratch/cass-targets/integrated \
  CARGO_INCREMENTAL=0 cargo clippy --all-targets -- -D warnings

cargo fmt --check

env TMPDIR=/home/james/scratch/cass-tmp \
  CARGO_TARGET_DIR=/home/james/scratch/cass-targets/integrated \
  CARGO_INCREMENTAL=0 cargo test --doc
```

The real-model integration test remains ignored by default and requires
`CASS_TEST_MODELS_DIR` pointing at assets created by `cass models install`.
