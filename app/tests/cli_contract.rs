use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::Connection;
use serde_json::Value;
use veritas_test_macros as veritas;

fn run_json(arguments: &[&str]) -> Value {
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .args(arguments)
        .output()
        .expect("run cass");
    assert!(
        output.status.success(),
        "cass failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON response")
}

fn create_valid_model_marker(root: &Path) {
    std::fs::create_dir_all(root).expect("model root");
    std::fs::write(root.join("dummy-model"), [0_u8]).expect("dummy model asset");
    std::fs::write(
        root.join("installed.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "embedding_model": "AllMiniLML6V2Q",
            "reranker_model": "jinaai/jina-reranker-v1-turbo-en",
            "embedding_dimensions": 384,
            "files": [{"path": "dummy-model", "size": 1}]
        }))
        .expect("model marker JSON"),
    )
    .expect("model marker");
}

fn seed_readiness_database(path: &Path, search_projection: Option<&str>) {
    let connection = Connection::open(path).expect("readiness database");
    connection
        .execute_batch(
            "CREATE TABLE conversations (
                id TEXT PRIMARY KEY, provider TEXT NOT NULL, source_path TEXT NOT NULL UNIQUE,
                title TEXT, created_at INTEGER, updated_at INTEGER,
                source_fingerprint TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE messages (
                id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL,
                ordinal INTEGER NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL,
                search_projection TEXT, created_at INTEGER,
                fingerprint TEXT NOT NULL DEFAULT '', UNIQUE(conversation_id, ordinal)
             );
             CREATE TABLE message_embeddings (
                message_id TEXT PRIMARY KEY, generation TEXT NOT NULL,
                dimensions INTEGER NOT NULL, norm REAL NOT NULL, vector BLOB NOT NULL
             );
             CREATE TABLE derived_state (
                singleton INTEGER PRIMARY KEY, search_dirty INTEGER NOT NULL
             );
             INSERT INTO derived_state VALUES (1, 0);
             INSERT INTO conversations(id, provider, source_path)
                VALUES ('session', 'codex', '/tmp/session.jsonl');
             PRAGMA user_version = 8;",
        )
        .expect("readiness schema");
    connection
        .execute(
            "INSERT INTO messages(
                id, conversation_id, ordinal, role, content, search_projection
             ) VALUES ('message', 'session', 0, 'tool', 'canonical tool output', ?1)",
            [search_projection],
        )
        .expect("readiness message");
}

fn current_embedding_generation() -> String {
    blake3::hash(
        concat!(
            "fastembed=6.0.1;model=AllMiniLML6V2Q;",
            "vector=i8-per-vector-symmetric;",
            "cosine=quantized-flat-exact;schema=2"
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string()
}

#[veritas::claims("cli/bare-invocation-prints-help")]
#[test]
fn bare_invocation_prints_help_without_starting_an_interface() {
    Command::cargo_bin("cass")
        .expect("cass binary")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage:"));
}

#[veritas::claims("cli/removed-commands-are-rejected")]
#[test]
fn removed_commands_are_rejected() {
    for command in ["export", "doctor", "list", "sources"] {
        Command::cargo_bin("cass")
            .expect("cass binary")
            .arg(command)
            .assert()
            .failure();
    }
}

#[veritas::claims("semantic/index-requires-models", "models/download-is-explicit")]
#[test]
fn index_without_models_fails_before_creating_the_database() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("cass.sqlite3");
    let models = directory.path().join("models");
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .args([
            "--db",
            database.to_str().unwrap(),
            "--models-dir",
            models.to_str().unwrap(),
            "index",
        ])
        .output()
        .expect("run index");

    assert!(!output.status.success());
    assert_eq!(output.stdout, Vec::<u8>::new());
    let error: Value = serde_json::from_slice(&output.stderr).expect("JSON error");
    assert_eq!(error["error"]["kind"], "model");
    assert_eq!(error["error"]["recommended_action"], "models install");
    assert!(!database.exists());
    assert!(!models.exists());
}

#[veritas::claims("semantic/missing-models-fail-search", "models/download-is-explicit")]
#[test]
fn search_without_models_fails_before_database_access() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let models = directory.path().join("models");
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .args([
            "--db",
            directory.path().join("missing.sqlite3").to_str().unwrap(),
            "--models-dir",
            models.to_str().unwrap(),
            "search",
            "needle",
        ])
        .output()
        .expect("run search");

    assert!(!output.status.success());
    assert_eq!(output.stdout, Vec::<u8>::new());
    let error: Value = serde_json::from_slice(&output.stderr).expect("JSON error");
    assert_eq!(error["error"]["kind"], "model");
    assert_eq!(error["error"]["recommended_action"], "models install");
    assert!(!models.exists());
}

#[veritas::claims("semantic/missing-embeddings-fail-search")]
#[test]
fn search_with_missing_embeddings_recommends_index_without_loading_models() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let models = directory.path().join("models");
    let database = directory.path().join("cass.sqlite3");
    create_valid_model_marker(&models);
    seed_readiness_database(&database, None);

    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .args([
            "--db",
            database.to_str().unwrap(),
            "--models-dir",
            models.to_str().unwrap(),
            "search",
            "needle",
        ])
        .output()
        .expect("run search");

    assert!(!output.status.success());
    assert_eq!(output.stdout, Vec::<u8>::new());
    let error: Value = serde_json::from_slice(&output.stderr).expect("JSON error");
    assert_eq!(error["error"]["kind"], "search-not-ready");
    assert_eq!(error["error"]["recommended_action"], "index");
}

#[veritas::claims("semantic/inference-failure-fails-search")]
#[test]
fn invalid_installed_assets_fail_without_lexical_results() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let models = directory.path().join("models");
    std::fs::create_dir_all(&models).expect("model root");
    std::fs::write(models.join("installed.json"), "not-json").expect("broken marker");
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .args([
            "--db",
            directory.path().join("cass.sqlite3").to_str().unwrap(),
            "--models-dir",
            models.to_str().unwrap(),
            "search",
            "needle",
        ])
        .output()
        .expect("run search");

    assert!(!output.status.success());
    assert_eq!(output.stdout, Vec::<u8>::new());
    let error: Value = serde_json::from_slice(&output.stderr).expect("JSON error");
    assert_eq!(error["error"]["kind"], "model");
    assert_eq!(error["error"]["recommended_action"], "models install");
}

#[veritas::claims("semantic/inference-failure-fails-search")]
#[test]
fn missing_listed_model_asset_is_a_typed_model_error() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let models = directory.path().join("models");
    create_valid_model_marker(&models);
    std::fs::remove_file(models.join("dummy-model")).expect("remove listed model asset");
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .args([
            "--db",
            directory.path().join("cass.sqlite3").to_str().unwrap(),
            "--models-dir",
            models.to_str().unwrap(),
            "search",
            "needle",
        ])
        .output()
        .expect("run search");

    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).expect("JSON error");
    assert_eq!(error["error"]["kind"], "model");
    assert_eq!(error["error"]["recommended_action"], "models install");
}

#[veritas::claims(
    "status/missing-models-recommends-install",
    "models/download-is-explicit"
)]
#[test]
fn status_without_models_recommends_install_without_downloading() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let models = directory.path().join("models");
    let body = run_json(&[
        "--db",
        directory.path().join("missing.sqlite3").to_str().unwrap(),
        "--models-dir",
        models.to_str().unwrap(),
        "status",
    ]);
    assert_eq!(body["ready"], false);
    assert_eq!(body["recommended_action"], "models install");
    assert!(!models.exists());
}

#[veritas::claims(
    "status/missing-database-recommends-index",
    "status/missing-embeddings-recommends-index",
    "status/semantic-search-ready",
    "status/zero-searchable-messages-can-be-ready"
)]
#[test]
fn status_uses_model_then_database_then_embedding_readiness() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let models = directory.path().join("models");
    create_valid_model_marker(&models);

    let missing = run_json(&[
        "--db",
        directory.path().join("missing.sqlite3").to_str().unwrap(),
        "--models-dir",
        models.to_str().unwrap(),
        "status",
    ]);
    assert_eq!(missing["ready"], false);
    assert_eq!(missing["recommended_action"], "index");

    let incomplete_database = directory.path().join("incomplete.sqlite3");
    seed_readiness_database(&incomplete_database, None);
    let incomplete = run_json(&[
        "--db",
        incomplete_database.to_str().unwrap(),
        "--models-dir",
        models.to_str().unwrap(),
        "status",
    ]);
    assert_eq!(incomplete["searchable_messages"], 1);
    assert_eq!(incomplete["embeddings"], 0);
    assert_eq!(incomplete["ready"], false);
    assert_eq!(incomplete["recommended_action"], "index");

    let ready_database = directory.path().join("ready.sqlite3");
    seed_readiness_database(&ready_database, None);
    Connection::open(&ready_database)
        .expect("ready database")
        .execute(
            "INSERT INTO message_embeddings(message_id, generation, dimensions, norm, vector)
             VALUES ('message', ?1, 1, 127.0, X'7F')",
            [current_embedding_generation()],
        )
        .expect("current embedding");
    let ready = run_json(&[
        "--db",
        ready_database.to_str().unwrap(),
        "--models-dir",
        models.to_str().unwrap(),
        "status",
    ]);
    assert_eq!(ready["ready"], true);
    assert_eq!(ready["realized_mode"], "hybrid");
    assert!(ready["recommended_action"].is_null());

    let context_only_database = directory.path().join("context-only.sqlite3");
    seed_readiness_database(&context_only_database, Some(""));
    let context_only = run_json(&[
        "--db",
        context_only_database.to_str().unwrap(),
        "--models-dir",
        models.to_str().unwrap(),
        "status",
    ]);
    assert_eq!(context_only["messages"], 1);
    assert_eq!(context_only["searchable_messages"], 0);
    assert_eq!(context_only["embeddings"], 0);
    assert_eq!(context_only["ready"], true);
    assert_eq!(context_only["realized_mode"], "hybrid");
    assert!(context_only["recommended_action"].is_null());
}

#[test]
fn status_reads_an_older_database_without_migrating_it() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let models = directory.path().join("models");
    create_valid_model_marker(&models);
    let database = directory.path().join("old.sqlite3");
    Connection::open(&database)
        .expect("old database")
        .execute_batch(
            "CREATE TABLE conversations (id TEXT PRIMARY KEY);
             CREATE TABLE messages (id TEXT PRIMARY KEY);
             CREATE TABLE message_embeddings (message_id TEXT PRIMARY KEY);
             INSERT INTO conversations VALUES ('conversation');
             INSERT INTO messages VALUES ('message');
             PRAGMA user_version = 7;",
        )
        .expect("seed old database");

    let body = run_json(&[
        "--db",
        database.to_str().unwrap(),
        "--models-dir",
        models.to_str().unwrap(),
        "status",
    ]);
    assert_eq!(body["conversations"], 1);
    assert_eq!(body["messages"], 1);
    assert_eq!(body["ready"], false);
    assert_eq!(body["recommended_action"], "index");

    let connection = Connection::open(&database).expect("reopen old database");
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("schema version");
    let projection_exists: bool = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM pragma_table_info('messages') WHERE name = 'search_projection'
             )",
            [],
            |row| row.get(0),
        )
        .expect("projection check");
    assert_eq!(version, 7);
    assert!(!projection_exists);
}

#[cfg(unix)]
#[veritas::claims("federated-search/local-semantic-readiness-is-required")]
#[test]
fn federated_search_fails_when_local_semantic_search_is_unready() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let ssh = directory.path().join("ssh");
    std::fs::write(
        &ssh,
        concat!(
            "#!/bin/sh\n",
            "cat >/dev/null\n",
            "printf '%s\\n' '{\"protocol\":2,\"kind\":\"search\",",
            "\"response\":{\"query\":\"query\",\"realized_mode\":\"hybrid\",",
            "\"results\":[]}}'\n"
        ),
    )
    .expect("fake ssh");
    let mut permissions = std::fs::metadata(&ssh).expect("ssh metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&ssh, permissions).expect("executable ssh");
    let path = std::env::join_paths(std::iter::once(directory.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("fake ssh path");

    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .env("PATH", path)
        .args([
            "--db",
            directory.path().join("missing.sqlite3").to_str().unwrap(),
            "--models-dir",
            directory.path().join("missing-models").to_str().unwrap(),
            "search",
            "query",
            "--node",
            "remote-node",
        ])
        .output()
        .expect("federated search");
    assert!(!output.status.success());
    assert_eq!(output.stdout, Vec::<u8>::new());
    let error: Value = serde_json::from_slice(&output.stderr).expect("typed local error");
    assert_eq!(error["error"]["kind"], "model");
    assert_eq!(error["error"]["recommended_action"], "models install");
}

#[veritas::claims("view/tool-results-remain-visible")]
#[test]
fn view_returns_context_only_canonical_content_without_models() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("cass.sqlite3");
    seed_readiness_database(&database, Some(""));
    let viewed = run_json(&[
        "--db",
        database.to_str().unwrap(),
        "view",
        "message",
        "--context",
        "0",
    ]);
    assert_eq!(viewed["messages"][0]["content"], "canonical tool output");
}

#[veritas::claims("distribution/every-build-includes-semantic")]
#[test]
fn cargo_and_ci_make_semantic_support_unconditional() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("Cargo manifest");
    let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).expect("CI workflow");
    let release = std::fs::read_to_string(root.join(".github/workflows/release.yml"))
        .expect("release workflow");

    assert!(manifest.contains("fastembed = { version = \"6.0.1\", default-features = false"));
    assert!(!manifest.contains("optional = true"));
    assert!(!manifest.contains("semantic = ["));
    assert!(ci.contains("cargo nextest run --profile ci --no-default-features"));
    assert!(release.contains("cargo build --release --locked"));
    assert!(!root.join("app/semantic_disabled.rs").exists());
}

#[veritas::claims("independence/no-dickles-franken-surface")]
#[test]
fn maintained_repository_is_independent_and_minimal() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for removed in ["src", "tests", "benches", "fuzz", "scripts"] {
        assert!(!root.join(removed).exists(), "stale surface: {removed}");
    }
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("Cargo manifest");
    for removed in ["franken", "asupersync", "toon", "msgpack"] {
        assert!(!manifest.to_ascii_lowercase().contains(removed));
    }
}

#[veritas::claims(
    "semantic/hybrid-reranks-with-models",
    "search/fts-contributes-to-hybrid",
    "search/tool-results-are-not-searchable",
    "search/mixed-message-excludes-tool-result-text"
)]
#[test]
#[ignore = "requires CASS_TEST_MODELS_DIR containing an explicit model installation"]
fn real_models_index_and_run_hybrid_search() {
    let models = std::env::var_os("CASS_TEST_MODELS_DIR")
        .map(std::path::PathBuf::from)
        .expect("explicit model directory");
    let directory = tempfile::tempdir().expect("temporary directory");
    let claude = directory.path().join("claude");
    let codex = directory.path().join("codex");
    std::fs::create_dir_all(&claude).expect("Claude root");
    std::fs::create_dir_all(&codex).expect("Codex root");
    std::fs::write(
        claude.join("session.jsonl"),
        concat!(
            "{\"type\":\"user\",\"sessionId\":\"hybrid\",\"uuid\":\"m1\",",
            "\"message\":{\"role\":\"user\",\"content\":\"repair authentication credentials\"}}\n",
            "{\"type\":\"assistant\",\"sessionId\":\"hybrid\",\"uuid\":\"m2\",",
            "\"message\":{\"role\":\"assistant\",\"content\":[",
            "{\"type\":\"text\",\"text\":\"refresh the login token\"},",
            "{\"type\":\"tool_result\",\"content\":\"private-output-marker\"}]}}\n"
        ),
    )
    .expect("Claude fixture");
    let database = directory.path().join("cass.sqlite3");
    let model_arg = models.to_str().unwrap();
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .env("CASS_CLAUDE_ROOTS", &claude)
        .env("CASS_CODEX_ROOTS", &codex)
        .args([
            "--db",
            database.to_str().unwrap(),
            "--models-dir",
            model_arg,
            "index",
            "--full",
        ])
        .output()
        .expect("index");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let indexed: Value = serde_json::from_slice(&output.stdout).expect("index JSON");
    assert_eq!(indexed["realized_mode"], "hybrid");
    assert_eq!(indexed["indexed_messages"], 2);
    assert_eq!(indexed["searchable_messages"], 2);
    for timing in [
        "model_load_milliseconds",
        "storage_setup_milliseconds",
        "ingestion_milliseconds",
        "search_refresh_milliseconds",
        "embedding_milliseconds",
        "total_milliseconds",
    ] {
        assert!(indexed[timing].is_u64(), "missing index timing: {timing}");
    }

    let found = run_json(&[
        "--db",
        database.to_str().unwrap(),
        "--models-dir",
        model_arg,
        "search",
        "authentication",
    ]);
    assert_eq!(found["realized_mode"], "hybrid");
    assert!(found.get("fallback_mode").is_none());
    assert!(found.get("fallback_reason").is_none());
    assert!(found["results"][0]["lexical_score"].is_number());
    assert!(found["results"][0]["semantic_score"].is_number());
    assert!(found["results"][0]["rerank_score"].is_number());
}
