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

fn error_kind(output: &std::process::Output) -> Value {
    serde_json::from_slice::<Value>(&output.stderr).unwrap_or_else(|error| {
        panic!(
            "typed error: {error}; status={:?}; stdout={}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })["error"]["kind"]
        .clone()
}

fn sorted_lines(path: &Path) -> Vec<String> {
    let mut lines = std::fs::read_to_string(path)
        .expect("text file")
        .lines()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    lines.sort();
    lines
}

#[cfg(unix)]
fn run_search_with_legacy_node_environment(
    path: &std::ffi::OsStr,
    root: &Path,
    log: &Path,
) -> std::process::Output {
    Command::cargo_bin("cass")
        .expect("cass binary")
        .env("PATH", path)
        .envs([("XDG_CONFIG_HOME", root), ("XDG_DATA_HOME", root)])
        .env("CASS_FAKE_SSH_LOG", log)
        .env("CASS_SEARCH_NODES", "dev-macbook")
        .args(["search", "query"])
        .output()
        .expect("local-only search")
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
            "coreml_embedding": cfg!(all(target_os = "macos", target_arch = "aarch64")),
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
                singleton INTEGER PRIMARY KEY, search_dirty INTEGER NOT NULL,
                semantic_ready_generation TEXT
             );
             INSERT INTO derived_state VALUES (1, 0, NULL);
             INSERT INTO conversations(id, provider, source_path)
                VALUES ('session', 'codex', '/tmp/session.jsonl');
             PRAGMA user_version = 10;",
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
    let specification = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        concat!(
            "fastembed=6.0.1;model=AllMiniLML6V2Q;",
            "backend=coreml-fp32;batch=32;sequence=512;",
            "vector=i8-per-vector-symmetric;",
            "cosine=quantized-flat-exact;schema=4"
        )
    } else {
        concat!(
            "fastembed=6.0.1;model=AllMiniLML6V2Q;",
            "batch=8;workers=8;threads=2;",
            "vector=i8-per-vector-symmetric;",
            "cosine=quantized-flat-exact;schema=3"
        )
    };
    blake3::hash(specification.as_bytes()).to_hex().to_string()
}

fn write_config(path: &Path, local_node: &str, claude_root: &Path, codex_root: &Path) {
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 1,
            "local_node": local_node,
            "nodes": [{
                "name": local_node,
                "ssh": local_node,
                "search": true,
                "providers": {
                    "claude-code": {"roots": [claude_root]},
                    "codex": {"roots": [codex_root]}
                },
                "index": {"since_days": 30}
            }]
        }))
        .expect("configuration JSON"),
    )
    .expect("configuration file");
}

fn write_federation_config(path: &Path) {
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 1,
            "local_node": "xenia",
            "nodes": [
                {
                    "name": "xenia",
                    "ssh": "xenia-tail",
                    "search": true,
                    "providers": {}
                },
                {
                    "name": "dev-macbook",
                    "ssh": "dev-tail",
                    "search": true,
                    "providers": {}
                },
                {
                    "name": "personal-macbook",
                    "ssh": "personal-tail",
                    "search": false,
                    "providers": {}
                },
                {
                    "name": "backup-macbook",
                    "ssh": "backup-tail",
                    "search": true,
                    "providers": {}
                }
            ]
        }))
        .expect("federation configuration JSON"),
    )
    .expect("federation configuration");
}

#[veritas::claims("configuration/status-reports-resolved-settings")]
#[test]
fn status_reports_the_exact_resolved_local_configuration() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let config = directory.path().join("config.json");
    let claude = directory.path().join("remote-claude");
    let codex = directory.path().join("remote-codex");
    write_config(&config, "xenia", &claude, &codex);

    let body = run_json(&[
        "--config",
        config.to_str().unwrap(),
        "--db",
        directory.path().join("missing.sqlite3").to_str().unwrap(),
        "--models-dir",
        directory.path().join("missing-models").to_str().unwrap(),
        "status",
    ]);

    assert_eq!(
        body["configuration"],
        serde_json::json!({
            "path": std::fs::canonicalize(&config).expect("canonical config"),
            "loaded": true,
            "local_node": "xenia",
            "providers": {
                "claude-code": {"roots": [claude]},
                "codex": {"roots": [codex]}
            },
            "index": {"since_days": 30}
        })
    );
}

#[veritas::claims("configuration/status-reports-resolved-settings")]
#[test]
fn legacy_provider_root_environment_variables_are_ignored() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let home = directory.path().join("home");
    let config_home = directory.path().join("config-home");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&config_home).expect("config home");
    let injected = directory.path().join("injected");
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("CASS_CLAUDE_ROOTS", &injected)
        .env("CASS_CODEX_ROOTS", &injected)
        .env("CASS_OPENCODE_ROOTS", &injected)
        .env("CASS_COPILOT_ROOTS", &injected)
        .env("CASS_HERMES_ROOTS", &injected)
        .env("CASS_PI_ROOTS", &injected)
        .args([
            "--db",
            directory.path().join("missing.sqlite3").to_str().unwrap(),
            "status",
        ])
        .output()
        .expect("status");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).expect("status JSON");
    assert_eq!(
        body["configuration"],
        serde_json::json!({
            "path": config_home.join("cass/config.json"),
            "loaded": false,
            "local_node": null,
            "providers": {
                "claude-code": {"roots": [
                    home.join(".claude/projects"),
                    home.join(".config/claude/projects")
                ]},
                "codex": {"roots": [
                    home.join(".codex/sessions"),
                    home.join(".local/share/codex/sessions")
                ]}
            },
            "index": {"since_days": 90}
        })
    );
}

#[veritas::claims(
    "configuration/invalid-loaded-file-fails-before-effects",
    "configuration/errors-are-stable-and-nonretryable"
)]
#[test]
fn every_public_command_rejects_malformed_configuration_before_effects() {
    let command_arguments: &[&[&str]] = &[
        &["index"],
        &["search", "needle", "--node", "remote"],
        &["view", "message", "--node", "remote"],
        &["status"],
        &["forget", "conversation"],
        &["models", "install"],
    ];

    for (arguments, explicit) in command_arguments
        .iter()
        .flat_map(|arguments| [(arguments, true), (arguments, false)])
    {
        let directory = tempfile::tempdir().expect("temporary directory");
        let xdg = directory.path().join("xdg");
        let config = if explicit {
            directory.path().join("config.json")
        } else {
            xdg.join("cass/config.json")
        };
        let database = directory.path().join("cass.sqlite3");
        let models = directory.path().join("models");
        std::fs::create_dir_all(config.parent().expect("config parent")).expect("config parent");
        std::fs::write(&config, "{").expect("malformed configuration");
        let mut command = Command::cargo_bin("cass").expect("cass binary");
        if explicit {
            command.args(["--config", config.to_str().unwrap()]);
        } else {
            command.env("XDG_CONFIG_HOME", &xdg);
        }
        let output = command
            .args(["--db", database.to_str().unwrap()])
            .args(["--models-dir", models.to_str().unwrap()])
            .args(*arguments)
            .output()
            .expect("run public command");

        assert!(
            !output.status.success(),
            "command unexpectedly passed: {arguments:?}"
        );
        assert_eq!(output.stdout, Vec::<u8>::new());
        let error: Value = serde_json::from_slice(&output.stderr).expect("one JSON error");
        assert_eq!(error["error"]["kind"], "configuration");
        assert_eq!(error["error"]["retryable"], false);
        assert!(error["error"].get("recommended_action").is_none());
        assert_eq!(output.status.code(), Some(9));
        assert!(!database.exists());
        assert!(!models.exists());
    }
}

#[veritas::claims("configuration/invalid-loaded-file-fails-before-effects")]
#[test]
fn hidden_workers_are_config_blind_and_reject_explicit_config_flags() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let explicit = directory.path().join("config.json");
    std::fs::write(&explicit, "{").expect("malformed explicit config");
    for (worker, request) in [
        (
            ["search", "--federation-request"],
            r#"{"protocol":2,"query":"needle","limit":1}"#,
        ),
        (
            ["view", "--federation-request"],
            r#"{"protocol":2,"id":"message","context":0}"#,
        ),
    ] {
        for explicit_flag in ["config", "local-node"] {
            let mut command = Command::cargo_bin("cass").expect("cass binary");
            if explicit_flag == "config" {
                command.args(["--config", explicit.to_str().unwrap()]);
            } else {
                command.args(["--local-node", "xenia"]);
            }
            let output = command
                .args(worker)
                .write_stdin(request)
                .output()
                .expect("explicit worker invocation");
            let error: Value = serde_json::from_slice(&output.stderr).expect("worker usage error");
            assert_eq!(error["error"]["kind"], "usage");
        }
    }

    let xdg = directory.path().join("xdg");
    let default_config = xdg.join("cass/config.json");
    std::fs::create_dir_all(default_config.parent().expect("config parent"))
        .expect("default config parent");
    std::fs::write(default_config, "{").expect("malformed default config");
    for (worker, request, expected_kind) in [
        (
            ["search", "--federation-request"],
            r#"{"protocol":2,"query":"needle","limit":1}"#,
            "model",
        ),
        (
            ["view", "--federation-request"],
            r#"{"protocol":2,"id":"message","context":0}"#,
            "database-missing",
        ),
    ] {
        let worker_output = Command::cargo_bin("cass")
            .expect("cass binary")
            .env("XDG_CONFIG_HOME", &xdg)
            .args([
                "--db",
                directory.path().join("missing.sqlite3").to_str().unwrap(),
                "--models-dir",
                directory.path().join("missing-models").to_str().unwrap(),
            ])
            .args(worker)
            .write_stdin(request)
            .output()
            .expect("config-blind worker invocation");
        let worker_error: Value =
            serde_json::from_slice(&worker_output.stderr).expect("worker runtime error");
        assert_eq!(worker_error["error"]["kind"], expected_kind);
    }
}

#[cfg(unix)]
#[veritas::claims("federated-search/remote-worker-is-nonrecursive")]
#[test]
fn hidden_workers_never_start_ssh_from_config_or_environment() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let xdg = directory.path().join("xdg");
    let config = xdg.join("cass/config.json");
    std::fs::create_dir_all(config.parent().unwrap()).expect("config parent");
    write_federation_config(&config);
    let log = directory.path().join("ssh.log");
    let ssh = directory.path().join("ssh");
    std::fs::write(
        &ssh,
        "#!/bin/sh\nprintf started >> \"$CASS_FAKE_SSH_LOG\"\nexit 23\n",
    )
    .expect("fake ssh");
    let mut permissions = std::fs::metadata(&ssh).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&ssh, permissions).expect("executable ssh");
    let path = std::env::join_paths(std::iter::once(directory.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("fake ssh path");

    for (worker, request, expected_kind) in [
        (
            ["search", "--federation-request"],
            r#"{"protocol":2,"query":"needle","limit":1}"#,
            "model",
        ),
        (
            ["view", "--federation-request"],
            r#"{"protocol":2,"id":"message","context":0}"#,
            "database-missing",
        ),
    ] {
        let output = Command::cargo_bin("cass")
            .expect("cass binary")
            .env("PATH", &path)
            .env("XDG_CONFIG_HOME", &xdg)
            .env("CASS_SEARCH_NODES", "dev-macbook")
            .env("CASS_FAKE_SSH_LOG", &log)
            .args([
                "--db",
                directory.path().join("missing.sqlite3").to_str().unwrap(),
                "--models-dir",
                directory.path().join("missing-models").to_str().unwrap(),
            ])
            .args(worker)
            .write_stdin(request)
            .output()
            .expect("hidden worker");
        let error: Value = serde_json::from_slice(&output.stderr).expect("worker JSON");
        assert_eq!(error["error"]["kind"], expected_kind);
        assert!(!log.exists());
    }
}

#[veritas::claims(
    "configuration/cli-values-have-precedence",
    "indexing/cli-provider-selection-is-bounded"
)]
#[test]
fn public_index_flags_validate_before_models_database_or_scanning() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let config = directory.path().join("config.json");
    let codex_root = directory.path().join("codex");
    std::fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "local_node": "test-node",
            "nodes": [{
                "name": "test-node",
                "ssh": "test-node",
                "search": true,
                "providers": {"codex": {"roots": [codex_root]}},
                "index": {"since_days": 90}
            }]
        }))
        .expect("config JSON"),
    )
    .expect("config");
    let database = directory.path().join("cass.sqlite3");
    let models = directory.path().join("models");
    for arguments in [
        vec!["--provider", "opencode"],
        vec!["--provider", "claude-code"],
        vec!["--since-days", "0"],
        vec!["--since-days", "30", "--all-history"],
    ] {
        let output = Command::cargo_bin("cass")
            .expect("cass binary")
            .args(["--config", config.to_str().unwrap()])
            .args(["--db", database.to_str().unwrap()])
            .args(["--models-dir", models.to_str().unwrap()])
            .arg("index")
            .args(arguments)
            .output()
            .expect("invalid index invocation");
        let error: Value = serde_json::from_slice(&output.stderr).expect("usage JSON");
        assert_eq!(error["error"]["kind"], "usage");
        assert!(!database.exists());
        assert!(!models.exists());
    }

    let accepted = Command::cargo_bin("cass")
        .expect("cass binary")
        .args(["--config", config.to_str().unwrap()])
        .args(["--db", database.to_str().unwrap()])
        .args(["--models-dir", models.to_str().unwrap()])
        .args([
            "index",
            "--provider",
            "codex",
            "--provider",
            "codex",
            "--since-days",
            "30",
        ])
        .output()
        .expect("accepted index invocation");
    let error: Value = serde_json::from_slice(&accepted.stderr).expect("model JSON");
    assert_eq!(error["error"]["kind"], "model");
    assert!(!database.exists());
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
    let ready_connection = Connection::open(&ready_database).expect("ready database");
    let generation = current_embedding_generation();
    ready_connection
        .execute(
            "INSERT INTO message_embeddings(message_id, generation, dimensions, norm, vector)
             VALUES ('message', ?1, 1, 127.0, X'7F')",
            [&generation],
        )
        .expect("current embedding");
    ready_connection
        .execute(
            "UPDATE derived_state SET semantic_ready_generation = ?1 WHERE singleton = 1",
            [&generation],
        )
        .expect("semantic readiness");
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
    let config = directory.path().join("config.json");
    write_federation_config(&config);
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
            "--config",
            config.to_str().unwrap(),
            "--db",
            directory.path().join("missing.sqlite3").to_str().unwrap(),
            "--models-dir",
            directory.path().join("missing-models").to_str().unwrap(),
            "search",
            "query",
            "--node",
            "dev-macbook",
        ])
        .output()
        .expect("federated search");
    assert!(!output.status.success());
    assert_eq!(output.stdout, Vec::<u8>::new());
    let error: Value = serde_json::from_slice(&output.stderr).expect("typed local error");
    assert_eq!(error["error"]["kind"], "model");
    assert_eq!(error["error"]["recommended_action"], "models install");
}

#[cfg(unix)]
#[veritas::claims(
    "configuration/environment-inputs-are-ignored",
    "federated-search/node-selection-precedence",
    "federated-search/configured-default-fanout",
    "federated-search/concurrent-fanout",
    "federated-search/remote-view"
)]
#[test]
fn configured_federation_uses_inventory_destinations_and_logical_names() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let config = directory.path().join("config.json");
    let log = directory.path().join("ssh.log");
    write_federation_config(&config);
    let ssh = directory.path().join("ssh");
    std::fs::write(
        &ssh,
        concat!(
            "#!/bin/sh\n",
            "shift 9\n",
            "destination=$1\n",
            "operation=$3\n",
            "if test \"$destination\" = dev-tail || test \"$destination\" = backup-tail; then\n",
            "  touch \"$CASS_FAKE_SSH_SYNC/$destination\"\n",
            "  while ! test -e \"$CASS_FAKE_SSH_SYNC/dev-tail\" || ! test -e \"$CASS_FAKE_SSH_SYNC/backup-tail\"; do sleep 0.01; done\n",
            "fi\n",
            "printf '%s %s\\n' \"$destination\" \"$operation\" >> \"$CASS_FAKE_SSH_LOG\"\n",
            "cat >/dev/null\n",
            "if test \"$operation\" = search; then\n",
            "  printf '%s\\n' '{\"protocol\":2,\"kind\":\"search\",\"response\":{\"query\":\"query\",\"realized_mode\":\"hybrid\",\"results\":[]}}'\n",
            "else\n",
            "  printf '%s\\n' '{\"protocol\":2,\"kind\":\"view\",\"response\":{\"id\":\"message\",\"messages\":[{\"id\":\"message\",\"ordinal\":0,\"role\":\"user\",\"content\":\"remote view\",\"created_at\":null}]}}'\n",
            "fi\n"
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

    let run_search = |nodes: &[&str]| {
        let mut command = Command::cargo_bin("cass").expect("cass binary");
        command
            .env("PATH", &path)
            .env("CASS_FAKE_SSH_LOG", &log)
            .env("CASS_FAKE_SSH_SYNC", directory.path())
            .args(["--config", config.to_str().unwrap()])
            .args([
                "--db",
                directory.path().join("missing.sqlite3").to_str().unwrap(),
                "--models-dir",
                directory.path().join("missing-models").to_str().unwrap(),
                "search",
                "query",
            ]);
        for node in nodes {
            command.args(["--node", node]);
        }
        command.output().expect("configured search")
    };

    let default = run_search(&[]);
    assert_eq!(error_kind(&default), "model");
    assert_eq!(
        sorted_lines(&log),
        ["backup-tail search", "dev-tail search"]
    );
    std::fs::write(&log, "").expect("clear log");
    let explicit = run_search(&["personal-macbook", "personal-macbook"]);
    assert_eq!(error_kind(&explicit), "model");
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "personal-tail search\n"
    );
    for invalid in ["xenia", "unknown"] {
        std::fs::write(&log, "").expect("clear log");
        let output = run_search(&[invalid]);
        assert_eq!(error_kind(&output), "usage");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "");
    }

    std::fs::write(&log, "").expect("clear log");
    let viewed = Command::cargo_bin("cass")
        .expect("cass binary")
        .env("PATH", &path)
        .env("CASS_FAKE_SSH_LOG", &log)
        .args(["--config", config.to_str().unwrap()])
        .args(["view", "message", "--node", "personal-macbook"])
        .output()
        .expect("configured remote view");
    assert!(viewed.status.success());
    let response: Value = serde_json::from_slice(&viewed.stdout).expect("view JSON");
    assert_eq!(response["messages"][0]["content"], "remote view");
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "personal-tail view\n"
    );

    std::fs::write(&log, "").expect("clear log");
    let ignored = run_search_with_legacy_node_environment(&path, directory.path(), &log);
    assert_eq!(error_kind(&ignored), "model");
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "");
}

#[cfg(unix)]
#[veritas::claims(
    "federated-search/node-selection-precedence",
    "federated-search/node-validation",
    "federated-search/remote-view"
)]
#[test]
fn explicit_remote_nodes_require_configuration_before_local_work_or_ssh() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let log = directory.path().join("ssh.log");
    let ssh = directory.path().join("ssh");
    std::fs::write(
        &ssh,
        "#!/bin/sh\nprintf started >> \"$CASS_FAKE_SSH_LOG\"\nexit 23\n",
    )
    .expect("fake ssh");
    let mut permissions = std::fs::metadata(&ssh).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&ssh, permissions).expect("executable ssh");
    let path = std::env::join_paths(std::iter::once(directory.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("fake ssh path");

    for arguments in [
        ["search", "query", "--node", "dev-macbook"],
        ["view", "message", "--node", "dev-macbook"],
    ] {
        let output = Command::cargo_bin("cass")
            .expect("cass binary")
            .env("PATH", &path)
            .envs([
                ("XDG_CONFIG_HOME", directory.path()),
                ("XDG_DATA_HOME", directory.path()),
            ])
            .env("CASS_FAKE_SSH_LOG", &log)
            .args(arguments)
            .output()
            .expect("explicit node without configuration");
        let error: Value = serde_json::from_slice(&output.stderr).expect("usage JSON");
        assert_eq!(error["error"]["kind"], "usage");
        assert!(!log.exists());
    }
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
    assert!(ci.contains("cargo nextest run --profile ci"));
    assert!(!ci.contains("--no-default-features"));
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
    let readme = std::fs::read_to_string(root.join("README.md")).expect("README");
    for removed in ["OpenCode", "Copilot", "Hermes", "Pi histories"] {
        assert!(!readme.contains(removed), "stale provider docs: {removed}");
    }
}

#[veritas::claims(
    "semantic/hybrid-reranks-with-models",
    "semantic-indexing/batching-preserves-vectors",
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
    let config = directory.path().join("config.json");
    write_config(&config, "test-node", &claude, &codex);
    let model_arg = models.to_str().unwrap();
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .args([
            "--config",
            config.to_str().unwrap(),
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
    assert!(found["results"].as_array().is_some_and(|results| {
        results.iter().any(|result| {
            result["content"]
                .as_str()
                .is_some_and(|content| content.contains("authentication credentials"))
        })
    }));
}

#[veritas::claims("semantic-indexing/cold-throughput-target")]
#[test]
#[ignore = "requires explicit CASS_BENCHMARK_DB, CASS_BENCHMARK_CONFIG, and CASS_TEST_MODELS_DIR"]
fn benchmark_real_corpus_semantic_generation() {
    let source = std::env::var_os("CASS_BENCHMARK_DB")
        .map(std::path::PathBuf::from)
        .expect("explicit benchmark source database");
    let config = std::env::var_os("CASS_BENCHMARK_CONFIG")
        .map(std::path::PathBuf::from)
        .expect("explicit benchmark configuration");
    let models = std::env::var_os("CASS_TEST_MODELS_DIR")
        .map(std::path::PathBuf::from)
        .expect("explicit model directory");
    let directory = tempfile::tempdir().expect("temporary benchmark directory");
    let database = directory.path().join("benchmark.sqlite3");
    let source_connection =
        Connection::open_with_flags(&source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open benchmark source");
    source_connection
        .execute("VACUUM INTO ?1", [database.to_string_lossy().as_ref()])
        .expect("snapshot benchmark database");
    drop(source_connection);
    let connection = Connection::open(&database).expect("open benchmark snapshot");
    connection
        .execute("DELETE FROM message_embeddings", [])
        .expect("clear derived embeddings");
    drop(connection);

    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .args([
            "--config",
            config.to_str().unwrap(),
            "--db",
            database.to_str().unwrap(),
            "--models-dir",
            models.to_str().unwrap(),
            "index",
        ])
        .output()
        .expect("benchmark semantic index");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let indexed: Value = serde_json::from_slice(&output.stdout).expect("benchmark index JSON");
    let stored = indexed["embeddings"].as_u64().expect("stored embeddings");
    let inferred = indexed["model_inferences"]
        .as_u64()
        .expect("model inference count");
    let elapsed = indexed["embedding_milliseconds"]
        .as_u64()
        .expect("embedding elapsed time");
    assert!(inferred <= stored);
    #[allow(clippy::cast_precision_loss)]
    let rate = stored as f64 / (elapsed.max(1) as f64 / 1_000.0);
    eprintln!(
        "semantic benchmark stored={stored} inferred={inferred} elapsed_ms={elapsed} rate={rate:.1}"
    );
    assert!(rate >= 220.0, "semantic generation rate was {rate:.1}/s");
}
