# CASS development instructions

## Product boundary

CASS is a small JSON-only Rust CLI for Claude Code and Codex JSONL histories.
Keep the command surface limited to `index`, `search`, `view`, `status`,
`forget`, and `models install`.

- Use Rusqlite as the sole SQLite library.
- Use SQLite FTS5 for lexical search.
- Keep one concrete FastEmbed semantic backend with exact cosine search, RRF,
  and bounded reranking.
- Do not add provider traits, registries, daemons, ANN indexes, compatibility
  shims, alternate output modes, a TUI, export, analytics, sync, or watch mode.
- Keep ingestion to the two concrete CASS-owned JSONL parsers. Do not add
  external application databases, event-log providers, provider traits,
  registries, plugin systems, or compatibility layers.
- Never download models outside the explicit `models install` command.
- Unsafe Rust is forbidden.

## Implementation style

Prefer concrete functions and structs. External input returns typed JSON errors
and never panics. Keep parsing, storage, and search synchronous unless a current
API proves async is necessary. Add a dependency only when it is smaller and
clearer than a local implementation of the required behavior.

Use `apply_patch` for source edits. Preserve unrelated worktree changes. Do not
create compatibility copies or versioned replacement files.

## Verification

Use the persistent Xenia target and temp directories:

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

The real-model integration test is ignored by default and requires an explicit
`CASS_TEST_MODELS_DIR` pointing at assets created by `cass models install`.

## Git

Work on and push `main`; it is the repository's sole development branch.
