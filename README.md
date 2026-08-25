# cass

`cass` is a local, JSON-only search CLI for Claude Code and Codex JSONL
histories. It stores normalized conversations in SQLite, uses SQLite FTS5 for
lexical retrieval, and optionally adds local FastEmbed semantic retrieval plus
cross-encoder reranking after explicit model installation.

## Commands

```text
cass index [--full]
cass search <query> [--limit N] [--provider claude-code|codex] [--days N]
cass view <message-id> [--context N]
cass status
cass forget <conversation-id>
cass models install
```

All operational commands emit one JSON value. Bare `cass` prints concise help.
Use `--db PATH` to select the canonical SQLite database and `--models-dir PATH`
to select model assets.

## Discovery

Default history roots are `~/.claude/projects`, `~/.config/claude/projects`,
`~/.codex/sessions`, and `~/.local/share/codex/sessions`. To override them
without expanding the CLI surface, set `CASS_CLAUDE_ROOTS` and
`CASS_CODEX_ROOTS` to platform-separated path lists.

Only Claude Code and Codex JSONL files are parsed. Other application databases,
event logs, provider registries, plugin systems, compatibility shims, and
generalized connector layers are outside the active product boundary.

## Search Behavior

- SQLite is canonical; FTS rows and embeddings are rebuildable derived state.
- Search works immediately in lexical mode.
- `cass models install` is the only command that downloads model assets.
- Once models are installed and `cass index` has built embeddings, search uses
  exact cosine candidates, reciprocal-rank fusion, and bounded reranking.
- Search reports `realized_mode` and `fallback_mode` truthfully in JSON.

There is no TUI, export, daemon, remote sync, watch mode, analytics platform,
provider registry, compatibility layer, or alternate output encoding.

## Verification

```bash
cargo fmt --check
env TMPDIR=/home/james/scratch/cass-tmp \
  CARGO_TARGET_DIR=/home/james/scratch/cass-targets/integrated \
  CARGO_INCREMENTAL=0 cargo nextest run
env TMPDIR=/home/james/scratch/cass-tmp \
  CARGO_TARGET_DIR=/home/james/scratch/cass-targets/integrated \
  CARGO_INCREMENTAL=0 cargo clippy --all-targets -- -D warnings
env TMPDIR=/home/james/scratch/cass-tmp \
  CARGO_TARGET_DIR=/home/james/scratch/cass-targets/integrated \
  CARGO_INCREMENTAL=0 cargo test --doc
```

The real-model integration test is ignored by default and requires
`CASS_TEST_MODELS_DIR` pointing at assets created by `cass models install`.
