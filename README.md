# cass

`cass` is a small, JSON-only search CLI for Claude Code and Codex JSONL
histories. SQLite is the canonical store; SQLite FTS5, local FastEmbed
embeddings, reciprocal-rank fusion, and bounded cross-encoder reranking provide
hybrid retrieval. CASS can also federate searches and views across configured
machines over SSH.

## Commands

```text
cass index [--full] [--provider claude-code|codex]... [--since-days N|--all-history]
cass search <query> [--node NAME]... [--limit N] [--provider PROVIDER] [--days N]
cass view <message-id> [--node NAME] [--context N]
cass status
cass forget <conversation-id>
cass models install
```

All operational commands emit one JSON value. Bare `cass` prints help. Global
options are `--config PATH`, `--local-node NAME`, `--db PATH`, and
`--models-dir PATH`.

## Models and indexing

Search is semantic by default and has no production lexical-only mode. Install
models explicitly, then build the local index:

```bash
cass models install
cass index
```

No other command downloads models. Indexing fingerprints sources and messages,
skips unchanged input, updates FTS incrementally for small changes, embeds only
new or changed searchable messages, and excludes raw tool output from FTS and
embeddings while preserving it for `view`. `--full` rebuilds derived search
state. The default source horizon is 90 days; `--since-days N` overrides it and
`--all-history` removes it for one run.

Without a configuration file, CASS indexes these built-in roots when present:

- Claude Code: `~/.claude/projects`, `~/.config/claude/projects`
- Codex: `~/.codex/sessions`, `~/.local/share/codex/sessions`

## Configuration

CASS reads `config.json` from the platform CASS configuration directory. Use
`cass status` to see the resolved path, or pass `--config PATH` explicitly.
The file is strict, versioned JSON. Provider presence enables that provider for
the node; roots must be absolute. `since_days` defaults to 90 and may be `null`
for all history.

```json
{
  "version": 1,
  "local_node": "xenia",
  "nodes": [
    {
      "name": "xenia",
      "ssh": "xenia",
      "search": true,
      "providers": {
        "claude-code": {"roots": ["/home/james/.claude/projects"]},
        "codex": {"roots": ["/home/james/.codex/sessions"]}
      },
      "index": {"since_days": 90}
    },
    {
      "name": "dev-macbook",
      "ssh": "dev-macbook",
      "search": true,
      "providers": {
        "claude-code": {"roots": ["/Users/jko/.claude/projects"]},
        "codex": {"roots": ["/Users/jko/.codex/sessions"]}
      }
    }
  ]
}
```

The configured `local_node` selects this machine's roots and indexing horizon;
`--local-node` overrides that identity. CLI indexing flags override the local
node settings. A search without `--node` contacts every other node with
`"search": true`; repeated `--node NAME` values replace that default set and
may explicitly select a node whose `search` value is false. Logical node names
are reported in results while `ssh` supplies the transport destination.

There are no provider or federation environment-variable overrides. There is
also no TUI, export, daemon, remote sync, watch mode, analytics platform,
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

Every push to `main` builds the Linux x86-64 and Apple Silicon binaries in
GitHub Actions, then atomically deploys `cass` to `~/.local/bin` on Xenia,
dev-macbook, and personal-macbook over Tailscale. The macOS binary is
cross-compiled on the Linux runner with `cargo-zigbuild`; destination machines
need neither this repository nor a Rust toolchain.

Deployment requires `TS_OAUTH_CLIENT_ID`, `TS_OAUTH_SECRET`,
`DEPLOY_SSH_PRIVATE_KEY`, and `DEPLOY_SSH_KNOWN_HOSTS` repository secrets.

## License

See [LICENSE](LICENSE).
