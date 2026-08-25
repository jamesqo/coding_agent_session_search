use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::{Connection, params};
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

fn run_json_with_roots(arguments: &[&str], claude_root: &Path, codex_root: &Path) -> Value {
    let absent = claude_root.join("providers-not-configured");
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .env("CASS_CLAUDE_ROOTS", claude_root)
        .env("CASS_CODEX_ROOTS", codex_root)
        .env("CASS_OPENCODE_ROOTS", &absent)
        .env("CASS_COPILOT_ROOTS", &absent)
        .env("CASS_HERMES_ROOTS", &absent)
        .env("CASS_PI_ROOTS", &absent)
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

fn run_with_roots(
    arguments: &[&str],
    claude_root: &Path,
    codex_root: &Path,
) -> std::process::Output {
    let absent = claude_root.join("providers-not-configured");
    Command::cargo_bin("cass")
        .expect("cass binary")
        .env("CASS_CLAUDE_ROOTS", claude_root)
        .env("CASS_CODEX_ROOTS", codex_root)
        .env("CASS_OPENCODE_ROOTS", &absent)
        .env("CASS_COPILOT_ROOTS", &absent)
        .env("CASS_HERMES_ROOTS", &absent)
        .env("CASS_PI_ROOTS", &absent)
        .args(arguments)
        .output()
        .expect("run cass")
}

fn seed_search_database(directory: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let claude_root = directory.join("claude");
    let codex_root = directory.join("codex");
    std::fs::create_dir_all(&claude_root).expect("Claude root");
    std::fs::create_dir_all(&codex_root).expect("Codex root");
    std::fs::write(
        claude_root.join("session.jsonl"),
        "{\"type\":\"user\",\"sessionId\":\"federated-local\",\"uuid\":\"local-message\",\"message\":{\"role\":\"user\",\"content\":\"federated needle\"}}\n",
    )
    .expect("Claude fixture");
    let database = directory.join("cass.sqlite3");
    run_json_with_roots(
        &[
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "index",
        ],
        &claude_root,
        &codex_root,
    );
    (database, directory.join("models-empty"))
}

#[cfg(unix)]
fn fake_ssh(directory: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    fake_ssh_with_body(
        directory,
        concat!(
            "cat >/dev/null\n",
            "printf '%s\\n' '{\"protocol\":1,\"kind\":\"search\",\"response\":{\"query\":\"federated\",\"realized_mode\":\"lexical\",\"fallback_mode\":\"lexical\",\"fallback_reason\":null,\"results\":[]}}'\n",
        ),
    )
}

#[cfg(unix)]
fn fake_ssh_with_body(directory: &Path, body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let bin = directory.join("bin");
    std::fs::create_dir_all(&bin).expect("fake bin directory");
    let log = directory.join("ssh.log");
    let ssh = bin.join("ssh");
    std::fs::write(
        &ssh,
        format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$CASS_FAKE_SSH_LOG\"\n{body}"),
    )
    .expect("fake ssh");
    let mut permissions = std::fs::metadata(&ssh)
        .expect("fake ssh metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&ssh, permissions).expect("executable fake ssh");
    (bin, log)
}

fn run_json_with_six_roots(arguments: &[&str], roots: [&Path; 6]) -> Value {
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .env("CASS_CLAUDE_ROOTS", roots[0])
        .env("CASS_CODEX_ROOTS", roots[1])
        .env("CASS_OPENCODE_ROOTS", roots[2])
        .env("CASS_COPILOT_ROOTS", roots[3])
        .env("CASS_HERMES_ROOTS", roots[4])
        .env("CASS_PI_ROOTS", roots[5])
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

fn create_opencode_fixture(path: &Path) {
    Connection::open(path)
        .expect("OpenCode fixture")
        .execute_batch(
            r#"
            CREATE TABLE session (id TEXT PRIMARY KEY, title TEXT,
                time_created INTEGER, time_updated INTEGER);
            CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                time_created INTEGER, data TEXT NOT NULL);
            CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL,
                time_created INTEGER, data TEXT NOT NULL);
            INSERT INTO session VALUES ('open-session', 'OpenCode', 1, 2);
            INSERT INTO message VALUES
                ('open-user', 'open-session', 1, '{"role":"user"}'),
                ('open-assistant', 'open-session', 2, '{"role":"assistant"}');
            INSERT INTO part VALUES
                ('part-user', 'open-user', 1,
                 '{"type":"text","text":"opalescent otter"}'),
                ('part-assistant', 'open-assistant', 2,
                 '{"type":"text","text":"OpenCode answer"}');
            "#,
        )
        .expect("OpenCode schema");
}

fn create_hermes_fixture(path: &Path) {
    Connection::open(path)
        .expect("Hermes fixture")
        .execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY, title TEXT, started_at REAL, ended_at REAL
             );
             CREATE TABLE messages (
                id INTEGER PRIMARY KEY, session_id TEXT, role TEXT, content TEXT,
                reasoning TEXT, timestamp REAL
             );
             INSERT INTO sessions VALUES ('hermes-session', 'Hermes', 1.0, 2.0);
             INSERT INTO messages VALUES
                (1, 'hermes-session', 'user', 'vermilion vicuna', NULL, 1.0),
                (2, 'hermes-session', 'assistant', 'Hermes answer', NULL, 2.0);",
        )
        .expect("Hermes schema");
}

fn create_copilot_fixture(root: &Path) {
    let session = root.join("copilot-session");
    std::fs::create_dir_all(&session).expect("Copilot session");
    std::fs::write(
        session.join("events.jsonl"),
        concat!(
            "{\"type\":\"user.message\",\"data\":{\"content\":\"cerulean ibis\"}}\n",
            "{\"type\":\"assistant.message\",\"data\":{\"content\":\"Copilot answer\"}}\n",
        ),
    )
    .expect("Copilot fixture");
}

fn create_pi_fixture(root: &Path) {
    std::fs::create_dir_all(root).expect("Pi root");
    std::fs::write(
        root.join("session.jsonl"),
        concat!(
            "{\"type\":\"session\",\"id\":\"pi-session\"}\n",
            "{\"type\":\"message\",\"id\":\"pi-user\",\"message\":{\"role\":\"user\",\"content\":\"chartreuse chinchilla\"}}\n",
            "{\"type\":\"message\",\"id\":\"pi-assistant\",\"message\":{\"role\":\"assistant\",\"content\":\"Pi answer\"}}\n",
        ),
    )
    .expect("Pi fixture");
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
            .failure()
            .stderr(predicate::str::contains("unrecognized subcommand"));
    }
}

#[veritas::claims("cli/operational-command-surface", "ingestion/provider-boundary")]
#[test]
fn removed_provider_roots_and_filters_are_rejected() {
    let removed_root_arg = format!("--{}-db", ["open", "code"].concat());
    Command::cargo_bin("cass")
        .expect("cass binary")
        .args(["index", removed_root_arg.as_str(), "/tmp/removed.sqlite3"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument"));

    let removed_filter = ["cur", "sor"].concat();
    let directory = tempfile::tempdir().expect("temporary directory");
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .args([
            "--db",
            directory.path().join("missing.sqlite3").to_str().unwrap(),
            "search",
            "anything",
            "--provider",
            removed_filter.as_str(),
        ])
        .output()
        .expect("run cass search");
    assert!(!output.status.success());
    let body: Value = serde_json::from_slice(&output.stderr).expect("JSON error");
    assert_eq!(body["error"]["kind"], "usage");
}

#[veritas::claims("status/missing-database-recommends-index")]
#[test]
fn status_without_a_database_recommends_index() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .args([
            "--db",
            directory.path().join("missing.sqlite3").to_str().unwrap(),
            "status",
        ])
        .output()
        .expect("run cass status");

    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).expect("JSON status");
    assert_eq!(body["ready"], false);
    assert_eq!(body["recommended_action"], "index");
}

#[veritas::claims(
    "cli/operational-command-surface",
    "ingestion/supported-jsonl-indexes",
    "search/lexical-returns-distinctive-match",
    "view/context-clamps-to-conversation",
    "storage/forget-removes-conversation"
)]
#[test]
fn index_search_view_and_forget_supported_histories() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let claude_root = directory.path().join("claude");
    let codex_root = directory.path().join("codex");
    std::fs::create_dir_all(&claude_root).expect("Claude root");
    std::fs::create_dir_all(&codex_root).expect("Codex root");
    std::fs::write(
        claude_root.join("session.jsonl"),
        concat!(
            "{\"type\":\"user\",\"sessionId\":\"claude-1\",\"uuid\":\"u1\",\"message\":{\"role\":\"user\",\"content\":\"Where is the phosphorescent defect?\"}}\n",
            "{\"type\":\"assistant\",\"sessionId\":\"claude-1\",\"uuid\":\"a1\",\"message\":{\"role\":\"assistant\",\"content\":\"It is in storage.rs\"}}\n",
        ),
    )
    .expect("Claude fixture");
    std::fs::write(
        codex_root.join("rollout.jsonl"),
        concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-1\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"id\":\"m1\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"check the database\"}]}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"id\":\"m2\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"database checked\"}]}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"message\":\"database checked\"}}\n",
        ),
    )
    .expect("Codex fixture");

    let database = directory.path().join("cass.sqlite3");
    let database = database.to_str().expect("UTF-8 database path");
    let indexed = run_json_with_roots(
        &["--db", database, "index", "--full"],
        &claude_root,
        &codex_root,
    );
    assert_eq!(indexed["indexed_conversations"], 2);
    assert_eq!(indexed["indexed_messages"], 4);

    let found = run_json(&[
        "--db",
        database,
        "search",
        "phosphorescent",
        "--provider",
        "claude-code",
    ]);
    assert_eq!(found["realized_mode"], "lexical");
    let message_id = found["results"][0]["id"]
        .as_str()
        .expect("result message ID");

    let viewed = run_json(&["--db", database, "view", message_id, "--context", "10"]);
    assert_eq!(viewed["messages"].as_array().map(Vec::len), Some(2));

    let forgotten = run_json(&["--db", database, "forget", "claude-1"]);
    assert_eq!(forgotten["forgotten"], true);
    let absent = run_json(&["--db", database, "search", "phosphorescent"]);
    assert_eq!(absent["results"].as_array().map(Vec::len), Some(0));
}

#[veritas::claims("federated-search/node-selection-precedence")]
#[cfg(unix)]
#[test]
fn federated_explicit_nodes_override_environment_and_deduplicate() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (database, models) = seed_search_database(directory.path());
    let (bin, log) = fake_ssh(directory.path());
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("fake PATH");

    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .env("PATH", path)
        .env("CASS_FAKE_SSH_LOG", &log)
        .env("CASS_SEARCH_NODES", "environment-one,environment-two")
        .args([
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "--models-dir",
            models.to_str().expect("UTF-8 models path"),
            "search",
            "federated",
            "--node",
            "explicit-node",
            "--node",
            "explicit-node",
        ])
        .output()
        .expect("federated search");
    assert!(
        output.status.success(),
        "federated search failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = std::fs::read_to_string(&log).expect("fake SSH calls");
    assert_eq!(calls.lines().count(), 1);
    assert!(calls.contains("explicit-node"));
    assert!(!calls.contains("environment-one"));
    assert!(!calls.contains("environment-two"));
}

#[veritas::claims("federated-search/node-selection-precedence")]
#[cfg(unix)]
#[test]
fn federated_environment_nodes_are_used_without_explicit_nodes() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (database, models) = seed_search_database(directory.path());
    let (bin, log) = fake_ssh(directory.path());
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("fake PATH");
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .env("PATH", path)
        .env("CASS_FAKE_SSH_LOG", &log)
        .env("CASS_SEARCH_NODES", "environment-one,environment-one")
        .args([
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "--models-dir",
            models.to_str().expect("UTF-8 models path"),
            "search",
            "federated",
        ])
        .output()
        .expect("federated search");
    assert!(
        output.status.success(),
        "federated search failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = std::fs::read_to_string(&log).expect("fake SSH calls");
    assert_eq!(calls.lines().count(), 1);
    assert!(calls.contains("environment-one"));
}

#[veritas::claims("federated-search/node-validation")]
#[cfg(unix)]
#[test]
fn federated_invalid_or_excess_nodes_fail_before_ssh() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (database, models) = seed_search_database(directory.path());
    let (bin, log) = fake_ssh(directory.path());
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("fake PATH");
    let base = [
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "--models-dir",
        models.to_str().expect("UTF-8 models path"),
        "search",
        "federated",
    ];

    let invalid = Command::cargo_bin("cass")
        .expect("cass binary")
        .env("PATH", &path)
        .env("CASS_FAKE_SSH_LOG", &log)
        .args(base)
        .arg("--node=--option")
        .output()
        .expect("invalid-node search");
    assert!(!invalid.status.success());
    let invalid_stderr = String::from_utf8(invalid.stderr).expect("UTF-8 error");
    assert!(invalid_stderr.contains("invalid SSH node"));

    let mut excessive_arguments = base.iter().map(ToString::to_string).collect::<Vec<_>>();
    for index in 0..17 {
        excessive_arguments.push("--node".to_owned());
        excessive_arguments.push(format!("node-{index}"));
    }
    let excessive = Command::cargo_bin("cass")
        .expect("cass binary")
        .env("PATH", path)
        .env("CASS_FAKE_SSH_LOG", &log)
        .args(excessive_arguments)
        .output()
        .expect("excess-node search");
    assert!(!excessive.status.success());
    let excessive_stderr = String::from_utf8(excessive.stderr).expect("UTF-8 error");
    assert!(excessive_stderr.contains("at most 16 remote nodes"));
    assert!(!log.exists(), "invalid inputs must not invoke SSH");
}

#[veritas::claims("federated-search/node-selection-precedence")]
#[test]
fn local_search_response_omits_federated_fields() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (database, models) = seed_search_database(directory.path());
    let response = run_json(&[
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "--models-dir",
        models.to_str().expect("UTF-8 models path"),
        "search",
        "federated",
    ]);
    assert!(response.get("nodes").is_none());
    assert!(response["results"][0].get("origins").is_none());
    assert!(response["results"][0].get("federated_score").is_none());
}

#[cfg(unix)]
#[test]
fn federation_search_request_is_versioned_and_forces_local_execution() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (database, models) = seed_search_database(directory.path());
    let (bin, log) = fake_ssh(directory.path());
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("fake PATH");
    let mut command = Command::cargo_bin("cass").expect("cass binary");
    let output = command
        .env("PATH", path)
        .env("CASS_FAKE_SSH_LOG", &log)
        .env("CASS_SEARCH_NODES", "must-not-run")
        .args([
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "--models-dir",
            models.to_str().expect("UTF-8 models path"),
            "search",
            "--federation-request",
        ])
        .write_stdin(
            "{\"protocol\":1,\"query\":\"federated\",\"limit\":5,\"provider\":null,\"days\":null}\n",
        )
        .output()
        .expect("federation request");
    assert!(
        output.status.success(),
        "federation endpoint failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).expect("versioned response");
    assert_eq!(response["protocol"], 1);
    assert_eq!(response["kind"], "search");
    assert_eq!(response["response"]["query"], "federated");
    assert_eq!(
        response["response"]["results"].as_array().map(Vec::len),
        Some(1)
    );
    assert!(!log.exists(), "remote request mode must not invoke SSH");
}

#[veritas::claims(
    "federated-search/partial-failure",
    "federated-search/deterministic-merge",
    "federated-search/response-provenance"
)]
#[cfg(unix)]
#[test]
fn federated_search_merges_provenance_and_preserves_partial_results() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (database, models) = seed_search_database(directory.path());
    let local = run_json(&[
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "--models-dir",
        models.to_str().expect("UTF-8 models path"),
        "search",
        "federated",
    ]);
    let local_hit = &local["results"][0];
    let duplicate = serde_json::json!({
        "id": local_hit["id"],
        "conversation_id": local_hit["conversation_id"],
        "provider": local_hit["provider"],
        "role": "user",
        "content": "duplicate copy",
        "fusion_score": 0.0
    });
    let response = serde_json::json!({
        "protocol": 1,
        "kind": "search",
        "response": {
            "query": "federated",
            "realized_mode": "lexical",
            "fallback_mode": "lexical",
            "fallback_reason": null,
            "results": [duplicate, {
                "id": "remote-message",
                "conversation_id": "remote-conversation",
                "provider": "codex",
                "role": "assistant",
                "content": "remote only",
                "fusion_score": 0.0
            }]
        }
    });
    let body = format!(
        "cat >/dev/null\ncase \"$*\" in *bad-node*) printf 'remote failed\\n' >&2; exit 7;; esac\nprintf '%s\\n' '{response}'\n"
    );
    let (bin, log) = fake_ssh_with_body(directory.path(), &body);
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("fake PATH");
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .env("PATH", path)
        .env("CASS_FAKE_SSH_LOG", log)
        .args([
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "--models-dir",
            models.to_str().expect("UTF-8 models path"),
            "search",
            "federated",
            "--node",
            "good-node",
            "--node",
            "bad-node",
        ])
        .output()
        .expect("federated search");
    assert!(
        output.status.success(),
        "federated search failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).expect("JSON response");
    assert_eq!(response["realized_mode"], "federated");
    assert_eq!(response["nodes"][0]["node"], "good-node");
    assert_eq!(response["nodes"][0]["status"], "ok");
    assert_eq!(response["nodes"][1]["node"], "bad-node");
    assert_eq!(response["nodes"][1]["status"], "error");
    assert_eq!(response["nodes"][1]["error_kind"], "remote-exit");
    assert_eq!(response["results"].as_array().map(Vec::len), Some(2));
    assert_eq!(response["results"][0]["id"], local_hit["id"]);
    assert_eq!(
        response["results"][0]["origins"],
        serde_json::json!(["local", "good-node"])
    );
    assert_eq!(response["results"][0]["federated_score"], 1.0);
    assert_eq!(
        response["results"][1]["origins"],
        serde_json::json!(["good-node"])
    );
}

#[veritas::claims("federated-search/concurrent-fanout")]
#[cfg(unix)]
#[test]
fn federated_search_fans_out_nodes_concurrently() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (database, models) = seed_search_database(directory.path());
    let (bin, log) = fake_ssh_with_body(
        directory.path(),
        concat!(
            "cat >/dev/null\n",
            "sleep 1\n",
            "printf '%s\\n' '{\"protocol\":1,\"kind\":\"search\",\"response\":{\"query\":\"federated\",\"realized_mode\":\"lexical\",\"fallback_mode\":\"lexical\",\"fallback_reason\":null,\"results\":[]}}'\n",
        ),
    );
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("fake PATH");
    let started = std::time::Instant::now();
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .env("PATH", path)
        .env("CASS_FAKE_SSH_LOG", log)
        .args([
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "--models-dir",
            models.to_str().expect("UTF-8 models path"),
            "search",
            "federated",
            "--node",
            "one",
            "--node",
            "two",
        ])
        .output()
        .expect("federated search");
    assert!(output.status.success());
    assert!(
        started.elapsed() < std::time::Duration::from_millis(1_800),
        "two one-second nodes did not execute concurrently"
    );
}

#[veritas::claims("federated-search/remote-view")]
#[cfg(unix)]
#[test]
fn remote_view_uses_versioned_ssh_protocol_and_returns_context() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (database, _) = seed_search_database(directory.path());
    let (bin, log) = fake_ssh_with_body(
        directory.path(),
        concat!(
            "request=$(cat)\n",
            "case \"$request\" in *'\"protocol\":1'*'\"id\":\"remote-message\"'*) ;; *) exit 9;; esac\n",
            "printf '%s\\n' '{\"protocol\":1,\"kind\":\"view\",\"response\":{\"id\":\"remote-message\",\"messages\":[{\"id\":\"remote-message\",\"ordinal\":3,\"role\":\"assistant\",\"content\":\"remote context\",\"created_at\":null}]}}'\n",
        ),
    );
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("fake PATH");
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .env("PATH", path)
        .env("CASS_FAKE_SSH_LOG", &log)
        .args([
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "view",
            "remote-message",
            "--context",
            "4",
            "--node",
            "dev-macbook",
        ])
        .output()
        .expect("remote view");
    assert!(
        output.status.success(),
        "remote view failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).expect("JSON response");
    assert_eq!(response["id"], "remote-message");
    assert_eq!(response["messages"][0]["content"], "remote context");
    let calls = std::fs::read_to_string(log).expect("SSH call log");
    assert!(calls.contains("dev-macbook"));
    assert!(calls.contains("view --federation-request"));
}

#[cfg(unix)]
#[test]
fn federation_view_request_forces_local_execution() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (database, _) = seed_search_database(directory.path());
    let local = run_json(&[
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "search",
        "federated",
    ]);
    let id = local["results"][0]["id"].as_str().expect("local result ID");
    let mut command = Command::cargo_bin("cass").expect("cass binary");
    let output = command
        .env("CASS_SEARCH_NODES", "must-not-run")
        .args([
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "view",
            "--federation-request",
        ])
        .write_stdin(format!("{{\"protocol\":1,\"id\":{id:?},\"context\":0}}\n"))
        .output()
        .expect("local federation view request");
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).expect("JSON envelope");
    assert_eq!(response["protocol"], 1);
    assert_eq!(response["kind"], "view");
    assert_eq!(response["response"]["id"], id);
}

#[veritas::claims("ingestion/provider-boundary", "ingestion/supported-jsonl-indexes")]
#[test]
fn opencode_copilot_hermes_and_pi_histories_are_searchable() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let empty = directory.path().join("empty");
    let opencode = directory.path().join("opencode.db");
    let copilot = directory.path().join("copilot");
    let hermes = directory.path().join("hermes.db");
    let pi = directory.path().join("pi");
    std::fs::create_dir_all(&empty).expect("empty roots");
    create_opencode_fixture(&opencode);
    create_copilot_fixture(&copilot);
    create_hermes_fixture(&hermes);
    create_pi_fixture(&pi);
    let database = directory.path().join("cass.sqlite3");
    let indexed = run_json_with_six_roots(
        &[
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "index",
            "--full",
        ],
        [&empty, &empty, &opencode, &copilot, &hermes, &pi],
    );
    assert_eq!(indexed["indexed_conversations"], 4);
    assert_eq!(indexed["indexed_messages"], 8);
    assert_eq!(indexed["scanned_files"], 4);

    for (query, provider, conversation) in [
        ("opalescent", "opencode", "open-session"),
        ("cerulean", "github-copilot", "copilot-session"),
        ("vermilion", "hermes", "hermes-session"),
        ("chartreuse", "pi", "pi-session"),
    ] {
        let found = run_json(&[
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "search",
            query,
            "--provider",
            provider,
        ]);
        assert_eq!(found["results"].as_array().map(Vec::len), Some(1));
        assert_eq!(found["results"][0]["conversation_id"], conversation);
    }

    let refreshed = run_json_with_six_roots(
        &[
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "index",
        ],
        [&empty, &empty, &opencode, &copilot, &hermes, &pi],
    );
    assert_eq!(refreshed["unchanged_sources"], 4);
    assert_eq!(refreshed["changed_messages"], 0);
}

#[veritas::claims("ingestion/unsupported-providers-ignored")]
#[test]
fn unsupported_records_are_ignored() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let claude_root = directory.path().join("claude");
    let codex_root = directory.path().join("codex");
    std::fs::create_dir_all(&claude_root).expect("Claude root");
    std::fs::create_dir_all(&codex_root).expect("Codex root");
    std::fs::write(
        claude_root.join("unsupported.jsonl"),
        "{\"type\":\"message\",\"role\":\"user\",\"content\":\"not supported\"}\n",
    )
    .expect("unsupported fixture");

    let database = directory.path().join("cass.sqlite3");
    let indexed = run_json_with_roots(
        &[
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "index",
        ],
        &claude_root,
        &codex_root,
    );

    assert_eq!(indexed["scanned_files"], 1);
    assert_eq!(indexed["indexed_conversations"], 0);
    assert_eq!(indexed["indexed_messages"], 0);
}

#[veritas::claims("ingestion/unsupported-providers-ignored")]
#[test]
fn removed_provider_rows_are_purged_from_existing_databases() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("cass.sqlite3");
    let removed_provider = ["cur", "sor"].concat();
    seed_database_with_removed_provider_rows(&database, &removed_provider);

    let status = run_json(&[
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "status",
    ]);
    assert_eq!(status["conversations"], 1);
    assert_eq!(status["messages"], 1);

    let removed = run_json(&[
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "search",
        "legacyneedle",
    ]);
    assert_eq!(removed["results"].as_array().map(Vec::len), Some(0));

    let supported = run_json(&[
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "search",
        "supportedneedle",
    ]);
    assert_eq!(supported["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(supported["results"][0]["provider"], "codex");

    let claude_root = directory.path().join("claude");
    let codex_root = directory.path().join("codex");
    std::fs::create_dir_all(&claude_root).expect("Claude root");
    std::fs::create_dir_all(&codex_root).expect("Codex root");
    let indexed = run_json_with_roots(
        &[
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "index",
            "--full",
        ],
        &claude_root,
        &codex_root,
    );
    assert_eq!(indexed["indexed_conversations"], 0);
    assert_eq!(indexed["indexed_messages"], 0);

    let removed_after_rebuild = run_json(&[
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "search",
        "legacyneedle",
    ]);
    assert_eq!(
        removed_after_rebuild["results"].as_array().map(Vec::len),
        Some(0)
    );
}

#[veritas::claims(
    "ingestion/malformed-records-do-not-panic",
    "ingestion/supported-jsonl-indexes",
    "storage/full-rebuild-is-idempotent"
)]
#[test]
fn malformed_records_are_bounded_and_reindexing_is_idempotent() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let claude_root = directory.path().join("claude");
    let codex_root = directory.path().join("codex");
    std::fs::create_dir_all(&claude_root).expect("Claude root");
    std::fs::create_dir_all(&codex_root).expect("Codex root");
    std::fs::write(
        claude_root.join("session.jsonl"),
        concat!(
            "this is not JSON\n",
            "{\"type\":\"user\",\"sessionId\":\"stable-session\",\"uuid\":\"stable-message\",\"message\":{\"role\":\"user\",\"content\":\"persistent needle\"}}\n",
        ),
    )
    .expect("mixed fixture");

    let database = directory.path().join("cass.sqlite3");
    let database = database.to_str().expect("UTF-8 database path");
    let arguments = ["--db", database, "index", "--full"];
    let first = run_json_with_roots(&arguments, &claude_root, &codex_root);
    let second = run_json_with_roots(&arguments, &claude_root, &codex_root);

    assert_eq!(first["malformed_records"], 1);
    assert_eq!(first["indexed_conversations"], 1);
    assert_eq!(first["indexed_messages"], 1);
    assert_eq!(second["malformed_records"], 1);
    assert_eq!(second["indexed_conversations"], 1);
    assert_eq!(second["indexed_messages"], 1);

    let found = run_json(&["--db", database, "search", "persistent"]);
    assert_eq!(found["results"].as_array().map(Vec::len), Some(1));
}

#[test]
fn index_emits_structured_progress_without_polluting_stdout() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let claude_root = directory.path().join("claude");
    let codex_root = directory.path().join("codex");
    std::fs::create_dir_all(&claude_root).expect("Claude root");
    std::fs::create_dir_all(&codex_root).expect("Codex root");
    std::fs::write(
        claude_root.join("session.jsonl"),
        "{\"type\":\"user\",\"sessionId\":\"progress\",\"uuid\":\"m1\",\"message\":{\"role\":\"user\",\"content\":\"show progress\"}}\n",
    )
    .expect("Claude fixture");
    let database = directory.path().join("cass.sqlite3");

    let output = run_with_roots(
        &[
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "index",
        ],
        &claude_root,
        &codex_root,
    );
    assert!(output.status.success());
    serde_json::from_slice::<Value>(&output.stdout).expect("one JSON stdout response");
    let events = String::from_utf8(output.stderr)
        .expect("UTF-8 progress")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("JSON progress line"))
        .collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| event["event"] == "index-progress"
            && event["phase"] == "complete"
            && event["processed_files"] == 1),
        "missing completion progress event: {events:?}"
    );
}

#[test]
fn unchanged_source_uses_a_persisted_file_checkpoint() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let claude_root = directory.path().join("claude");
    let codex_root = directory.path().join("codex");
    std::fs::create_dir_all(&claude_root).expect("Claude root");
    std::fs::create_dir_all(&codex_root).expect("Codex root");
    let source = claude_root.join("session.jsonl");
    std::fs::write(
        &source,
        "{\"type\":\"user\",\"sessionId\":\"checkpoint\",\"uuid\":\"m1\",\"message\":{\"role\":\"user\",\"content\":\"checkpoint me\"}}\n",
    )
    .expect("Claude fixture");
    let database = directory.path().join("cass.sqlite3");
    let arguments = [
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "index",
    ];

    run_json_with_roots(&arguments, &claude_root, &codex_root);
    let connection = Connection::open(&database).expect("open indexed database");
    let checkpoint_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM source_checkpoints WHERE source_path = ?1",
            [source.to_string_lossy().as_ref()],
            |row| row.get(0),
        )
        .expect("persisted source checkpoint");
    assert_eq!(checkpoint_count, 1);

    let second = run_json_with_roots(&arguments, &claude_root, &codex_root);
    assert_eq!(second["checkpoint_skipped_sources"], 1);
    assert_eq!(second["changed_messages"], 0);
}

#[veritas::claims("semantic/missing-models-fall-back", "models/download-is-explicit")]
#[test]
fn search_without_models_falls_back_without_downloading() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let claude_root = directory.path().join("claude");
    let codex_root = directory.path().join("codex");
    let data_home = directory.path().join("data");
    std::fs::create_dir_all(&claude_root).expect("Claude root");
    std::fs::create_dir_all(&codex_root).expect("Codex root");
    std::fs::write(
        claude_root.join("session.jsonl"),
        "{\"type\":\"user\",\"sessionId\":\"fallback\",\"uuid\":\"m1\",\"message\":{\"role\":\"user\",\"content\":\"lexical-only proof\"}}\n",
    )
    .expect("Claude fixture");
    let database = directory.path().join("cass.sqlite3");

    Command::cargo_bin("cass")
        .expect("cass binary")
        .env("XDG_DATA_HOME", &data_home)
        .env("CASS_CLAUDE_ROOTS", &claude_root)
        .env("CASS_CODEX_ROOTS", &codex_root)
        .args([
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "index",
        ])
        .assert()
        .success();
    let output = Command::cargo_bin("cass")
        .expect("cass binary")
        .env("XDG_DATA_HOME", &data_home)
        .args([
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "search",
            "lexical",
        ])
        .output()
        .expect("run search");

    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).expect("JSON search");
    assert_eq!(body["realized_mode"], "lexical");
    assert_eq!(body["fallback_mode"], "lexical");
    assert!(!data_home.exists(), "search must not acquire model assets");
}

#[cfg(feature = "semantic")]
#[veritas::claims("semantic/inference-failure-falls-back")]
#[test]
fn invalid_installed_assets_fall_back_to_lexical_search() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let claude_root = directory.path().join("claude");
    let codex_root = directory.path().join("codex");
    let models_dir = directory.path().join("models");
    std::fs::create_dir_all(&claude_root).expect("Claude root");
    std::fs::create_dir_all(&codex_root).expect("Codex root");
    std::fs::create_dir_all(&models_dir).expect("model root");
    std::fs::write(
        claude_root.join("session.jsonl"),
        "{\"type\":\"user\",\"sessionId\":\"fallback-invalid\",\"uuid\":\"m1\",\"message\":{\"role\":\"user\",\"content\":\"recoverable lexical needle\"}}\n",
    )
    .expect("Claude fixture");
    let database = directory.path().join("cass.sqlite3");
    run_json_with_roots(
        &[
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "index",
        ],
        &claude_root,
        &codex_root,
    );
    let connection = Connection::open(&database).expect("open indexed database");
    let message_id: String = connection
        .query_row("SELECT id FROM messages LIMIT 1", [], |row| row.get(0))
        .expect("message ID");
    connection
        .execute(
            "INSERT INTO message_embeddings(message_id, generation, dimensions, norm, vector)
             VALUES (?1, ?2, 1, 127.0, ?3)",
            params![
                message_id,
                blake3::hash(
                    concat!(
                        "fastembed=6.0.1;model=AllMiniLML6V2Q;",
                        "vector=i8-per-vector-symmetric;",
                        "cosine=quantized-flat-exact;schema=2"
                    )
                    .as_bytes()
                )
                .to_hex()
                .to_string(),
                vec![127_u8]
            ],
        )
        .expect("seed derived vector");
    drop(connection);
    std::fs::write(models_dir.join("installed.json"), "not-json").expect("broken model marker");

    let found = run_json(&[
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "--models-dir",
        models_dir.to_str().expect("UTF-8 model path"),
        "search",
        "needle",
    ]);
    assert_eq!(found["realized_mode"], "lexical");
    assert_eq!(found["fallback_mode"], "lexical");
    assert!(
        found["fallback_reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("invalid model marker"))
    );
    assert_eq!(found["results"].as_array().map(Vec::len), Some(1));
}

#[veritas::claims("semantic/hybrid-reranks-with-models")]
#[test]
#[ignore = "requires CASS_TEST_MODELS_DIR containing an explicit model installation"]
fn hybrid_search_with_installed_models() {
    let models_dir = std::env::var_os("CASS_TEST_MODELS_DIR")
        .map(std::path::PathBuf::from)
        .expect("CASS_TEST_MODELS_DIR for explicitly installed test models");
    let directory = tempfile::tempdir().expect("temporary directory");
    let claude_root = directory.path().join("claude");
    let codex_root = directory.path().join("codex");
    std::fs::create_dir_all(&claude_root).expect("Claude root");
    std::fs::create_dir_all(&codex_root).expect("Codex root");
    std::fs::write(
        claude_root.join("session.jsonl"),
        concat!(
            "{\"type\":\"user\",\"sessionId\":\"hybrid-test\",\"uuid\":\"m1\",\"message\":{\"role\":\"user\",\"content\":\"How do I repair authentication credentials?\"}}\n",
            "{\"type\":\"assistant\",\"sessionId\":\"hybrid-test\",\"uuid\":\"m2\",\"message\":{\"role\":\"assistant\",\"content\":\"Refresh the expired login token and retry.\"}}\n",
            "{\"type\":\"user\",\"sessionId\":\"hybrid-test\",\"uuid\":\"m3\",\"message\":{\"role\":\"user\",\"content\":\"What color is the database icon?\"}}\n",
        ),
    )
    .expect("Claude fixture");
    let database = directory.path().join("cass.sqlite3");
    let models_dir = models_dir.to_str().expect("UTF-8 models path");
    let indexed = run_json_with_roots(
        &[
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "--models-dir",
            models_dir,
            "index",
            "--full",
        ],
        &claude_root,
        &codex_root,
    );
    assert_eq!(indexed["embeddings"], 3);
    assert_eq!(indexed["realized_mode"], "hybrid");

    let found = run_json(&[
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "--models-dir",
        models_dir,
        "search",
        "login credentials",
        "--limit",
        "3",
    ]);
    assert_eq!(found["realized_mode"], "hybrid");
    assert!(found["fallback_mode"].is_null());
    assert_eq!(found["results"].as_array().map(Vec::len), Some(3));
    assert!(found["results"][0]["semantic_score"].is_number());
    assert!(found["results"][0]["rerank_score"].is_number());
    assert!(
        found["results"][0]["content"]
            .as_str()
            .is_some_and(|content| content.contains("credentials"))
    );
}

#[test]
fn lexical_search_applies_recency_filter() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let claude_root = directory.path().join("claude");
    let codex_root = directory.path().join("codex");
    std::fs::create_dir_all(&claude_root).expect("Claude root");
    std::fs::create_dir_all(&codex_root).expect("Codex root");
    std::fs::write(
        claude_root.join("session.jsonl"),
        concat!(
            "{\"type\":\"user\",\"sessionId\":\"recency\",\"uuid\":\"old\",\"timestamp\":\"2000-01-01T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"temporal old\"}}\n",
            "{\"type\":\"assistant\",\"sessionId\":\"recency\",\"uuid\":\"new\",\"timestamp\":\"2099-01-01T00:00:00Z\",\"message\":{\"role\":\"assistant\",\"content\":\"temporal new\"}}\n",
        ),
    )
    .expect("Claude fixture");
    let database = directory.path().join("cass.sqlite3");
    let database = database.to_str().expect("UTF-8 database path");
    run_json_with_roots(&["--db", database, "index"], &claude_root, &codex_root);

    let all = run_json(&["--db", database, "search", "temporal"]);
    assert_eq!(all["results"].as_array().map(Vec::len), Some(2));
    let recent = run_json(&["--db", database, "search", "temporal", "--days", "30"]);
    assert_eq!(recent["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(recent["results"][0]["content"], "temporal new");
}

#[veritas::claims(
    "distribution/lexical-only-build-works",
    "distribution/release-includes-semantic"
)]
#[test]
fn cargo_and_ci_declare_the_two_supported_build_realizations() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("Cargo manifest");
    let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).expect("CI workflow");
    let release = std::fs::read_to_string(root.join(".github/workflows/release.yml"))
        .expect("release workflow");

    assert!(manifest.contains("default = [\"semantic\"]"));
    assert!(manifest.contains("semantic = [\"dep:fastembed\"]"));
    assert!(manifest.contains("fastembed = { version = \"6.0.1\", optional = true"));
    assert!(ci.contains("cargo nextest run --profile ci --no-default-features"));
    assert!(release.contains("cargo build --release --locked"));
    assert!(!release.contains("--no-default-features"));
}

#[veritas::claims("independence/no-dickles-franken-surface")]
#[test]
fn maintained_repository_is_independent_and_minimal() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for removed in [
        "src",
        "tests",
        "benches",
        "fuzz",
        "scripts",
        "docs",
        "packaging",
        "build.rs",
    ] {
        let path = root.join(removed);
        let absent = if path.is_dir() {
            std::fs::read_dir(&path)
                .expect("read removed directory")
                .next()
                .is_none()
        } else {
            !path.exists()
        };
        assert!(absent, "removed legacy path still has files: {removed}");
    }

    let forbidden = [
        concat!("franken", "sqlite"),
        concat!("franken", "search"),
        concat!("franken", "torch"),
        concat!("franken", "tui"),
        concat!("franken_", "agent_detection"),
        concat!("asu", "persync"),
        concat!("Dick", "les", "worthstone"),
    ];
    let mut maintained = String::new();
    let mut production_lines = 0;
    let mut test_lines = 0;
    for file in [
        "Cargo.toml",
        "Cargo.lock",
        "README.md",
        "AGENTS.md",
        "openspec/config.yaml",
        "openspec/changes/cass-independent-core/proposal.md",
        "openspec/changes/cass-independent-core/design.md",
        "openspec/changes/cass-independent-core/plan.md",
        "openspec/changes/cass-independent-core/specs/cass-independent-core/spec.md",
        ".vspec/changes/cass-independent-core/contract.md",
        ".github/workflows/ci.yml",
    ] {
        maintained.push_str(&std::fs::read_to_string(root.join(file)).expect("maintained file"));
    }
    for entry in walkdir::WalkDir::new(root.join("app")) {
        let entry = entry.expect("walk app source");
        if entry.file_type().is_file()
            && entry.path().extension().and_then(|value| value.to_str()) == Some("rs")
        {
            let source =
                std::fs::read_to_string(entry.path()).expect("UTF-8 maintained Rust source");
            if entry.path().starts_with(root.join("app/tests")) {
                test_lines += source.lines().count();
            } else {
                production_lines += source.lines().count();
            }
            maintained.push_str(&source);
        }
    }
    assert!(production_lines <= 70_000, "production Rust LOC exceeded");
    assert!(test_lines <= 30_000, "test Rust LOC exceeded");
    let maintained = maintained.to_ascii_lowercase();
    for name in forbidden {
        assert!(
            !maintained.contains(&name.to_ascii_lowercase()),
            "prohibited dependency surface remains: {name}"
        );
    }
}

fn seed_database_with_removed_provider_rows(path: &Path, removed_provider: &str) {
    let connection = Connection::open(path).expect("open seed database");
    let schema = format!(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE conversations (
             id TEXT PRIMARY KEY,
             provider TEXT NOT NULL CHECK (
                 provider IN ('claude-code', 'codex', '{removed_provider}')
             ),
             source_path TEXT NOT NULL UNIQUE,
             title TEXT,
             created_at INTEGER,
             updated_at INTEGER
         );
         CREATE TABLE messages (
             id TEXT PRIMARY KEY,
             conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
             ordinal INTEGER NOT NULL,
             role TEXT NOT NULL,
             content TEXT NOT NULL,
             created_at INTEGER,
             UNIQUE (conversation_id, ordinal)
         );
         CREATE TABLE message_embeddings (
             message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
             dimensions INTEGER NOT NULL,
             vector BLOB NOT NULL
         );
         CREATE VIRTUAL TABLE message_fts USING fts5(
             content,
             message_id UNINDEXED,
             conversation_id UNINDEXED,
             tokenize = 'unicode61'
         );"
    );
    connection.execute_batch(&schema).expect("seed schema");
    for (conversation_id, provider, source_path, message_id, content) in [
        (
            "legacy-conversation",
            removed_provider,
            "/tmp/legacy.jsonl",
            "legacy-message",
            "legacyneedle",
        ),
        (
            "codex-conversation",
            "codex",
            "/tmp/codex.jsonl",
            "codex-message",
            "supportedneedle",
        ),
    ] {
        connection
            .execute(
                "INSERT INTO conversations(id, provider, source_path)
                 VALUES (?1, ?2, ?3)",
                params![conversation_id, provider, source_path],
            )
            .expect("seed conversation");
        connection
            .execute(
                "INSERT INTO messages(id, conversation_id, ordinal, role, content)
                 VALUES (?1, ?2, 0, 'user', ?3)",
                params![message_id, conversation_id, content],
            )
            .expect("seed message");
        connection
            .execute(
                "INSERT INTO message_fts(content, message_id, conversation_id)
                 VALUES (?1, ?2, ?3)",
                params![content, message_id, conversation_id],
            )
            .expect("seed FTS");
    }
}
