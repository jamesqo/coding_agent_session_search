# cass

`cass` is a local, JSON-only search CLI for Claude Code, Codex, current
OpenCode, GitHub Copilot CLI, Hermes Agent, and Pi histories. It stores
normalized conversations in SQLite, uses SQLite FTS5 for
lexical retrieval, and optionally adds local FastEmbed semantic retrieval plus
cross-encoder reranking after explicit model installation.

## Commands

```text
cass index [--full]
cass search <query> [--limit N] [--provider PROVIDER] [--days N]
cass view <message-id> [--context N]
cass status
cass forget <conversation-id>
cass models install
```

All operational commands emit one JSON value. Bare `cass` prints concise help.
Use `--db PATH` to select the canonical SQLite database and `--models-dir PATH`
to select model assets.

## Discovery

Default sources are the current local stores under `~/.claude`, `~/.codex`,
`~/.local/share/opencode`, `~/.copilot`, `~/.hermes`, and `~/.pi`. Override
them with the corresponding `CASS_<PROVIDER>_ROOTS` environment variable,
using a platform-separated path list. OpenCode and Hermes use their current
SQLite schemas; Copilot uses CLI `events.jsonl`; the remaining providers use
their current JSONL stores. Legacy IDE, cloud, and alternate-store formats are
not scanned.

## Search Behavior

- SQLite is canonical; FTS rows and embeddings are rebuildable derived state.
- Search works immediately in lexical mode.
- `cass models install` is the only command that downloads model assets.
- Once models are installed and `cass index` has built embeddings, search uses
  exact cosine candidates, reciprocal-rank fusion, and bounded reranking.
- Search reports `realized_mode` and `fallback_mode` truthfully in JSON.
- Ordinary refreshes fingerprint sources and messages, skip unchanged input,
  and embed only added or changed messages in bounded batches.
- `forget` writes a durable tombstone. Complete scans purge disappeared
  sources; incomplete scans preserve committed state.

Semantic support is enabled in ordinary and release builds. For a faster
lexical-only development build, use `cargo build --no-default-features`; the
same commands remain available and report semantic support as unavailable.

There is no TUI, export, daemon, remote sync, watch mode, analytics platform,
provider registry, compatibility layer, or alternate output encoding.

## Verification

```bash
cargo fmt --check
cargo nextest run
cargo clippy --all-targets -- -D warnings
cargo test --doc
```

The real-model integration test is ignored by default and requires
`CASS_TEST_MODELS_DIR` pointing at assets created by `cass models install`.

## Automatic deployment

Every push to `main` builds the semantic-enabled Linux x86-64 and Apple Silicon
binaries in GitHub Actions, then atomically deploys `cass` to `~/.local/bin` on
Xenia, `dev-macbook`, and `personal-macbook` over Tailscale. The macOS binary is
cross-compiled on the Linux runner with `cargo-zigbuild`; destination machines
need neither this repository nor a Rust toolchain. Cargo downloads and native,
Linux, and Darwin build artifacts are cached by lockfile and toolchain.

Deployment requires `TS_OAUTH_CLIENT_ID`, `TS_OAUTH_SECRET`,
`DEPLOY_SSH_PRIVATE_KEY`, and `DEPLOY_SSH_KNOWN_HOSTS` repository secrets.

## License

See [LICENSE](LICENSE).
