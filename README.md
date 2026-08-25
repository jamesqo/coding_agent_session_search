# cass

`cass` is a local, JSON-only search CLI for Claude Code and Codex histories.
It stores normalized conversations in SQLite, uses SQLite FTS5 for lexical
retrieval, and optionally adds local semantic retrieval plus cross-encoder
reranking after an explicit model installation.

## Build and test

```bash
cargo build --release
cargo nextest run
cargo clippy --all-targets -- -D warnings
cargo test --doc
```

The ordinary test suite never downloads models. To run the real-model smoke
test, first install models into a dedicated directory and pass it explicitly:

```bash
cargo run -- --models-dir /path/to/cass-models models install
CASS_TEST_MODELS_DIR=/path/to/cass-models \
  cargo nextest run --run-ignored ignored-only \
  -E 'test(hybrid_search_with_installed_models)'
```

## Commands

```text
cass index [--full] [--claude-root PATH] [--codex-root PATH]
cass search <query> [--limit N] [--provider claude-code|codex] [--days N]
cass view <message-id> [--context N]
cass status
cass forget <conversation-id>
cass models install
```

All operational commands emit one JSON value. Bare `cass` prints concise help.
Use `--db PATH` to select the canonical SQLite database and `--models-dir PATH`
to select model assets.

Default history roots are `~/.claude/projects` and `~/.codex/sessions`. Only
Claude Code and Codex JSONL files are supported.

## Search behavior

- SQLite is canonical; FTS rows and embeddings are rebuildable derived state.
- Search works immediately in lexical mode.
- `cass models install` is the only command that downloads model assets.
- Once models are installed and `cass index` has built embeddings, search uses
  exact cosine candidates, reciprocal-rank fusion, and bounded reranking.
- Search reports `realized_mode` and `fallback_mode` truthfully in JSON.

There is no TUI, export, daemon, remote sync, watch mode, analytics platform,
provider registry, compatibility layer, or alternate output encoding.

## License

See [LICENSE](LICENSE).
