use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

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

// Veritas claim: cli/bare-invocation-prints-help
#[test]
fn bare_invocation_prints_help_without_starting_an_interface() {
    Command::cargo_bin("cass")
        .expect("cass binary")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage:"));
}

// Veritas claim: cli/removed-commands-are-rejected
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

// Veritas claim: status/missing-database-recommends-index
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

// Veritas claims: ingestion/supported-jsonl-indexes,
// search/lexical-returns-distinctive-match, view/context-clamps-to-conversation,
// storage/forget-removes-conversation
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
            "{\"type\":\"user\",\"sessionId\":\"claude-1\",\"uuid\":\"u1\",\"message\":{\"role\":\"user\",\"content\":\"Where is the phosphorescent-wombat bug?\"}}\n",
            "{\"type\":\"assistant\",\"sessionId\":\"claude-1\",\"uuid\":\"a1\",\"message\":{\"role\":\"assistant\",\"content\":\"It is in storage.rs\"}}\n",
        ),
    )
    .expect("Claude fixture");
    std::fs::write(
        codex_root.join("rollout.jsonl"),
        concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-1\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"id\":\"m1\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"check the database\"}]}}\n",
        ),
    )
    .expect("Codex fixture");

    let database = directory.path().join("cass.sqlite3");
    let database = database.to_str().expect("UTF-8 database path");
    let claude_root = claude_root.to_str().expect("UTF-8 Claude path");
    let codex_root = codex_root.to_str().expect("UTF-8 Codex path");
    let indexed = run_json(&[
        "--db",
        database,
        "index",
        "--full",
        "--claude-root",
        claude_root,
        "--codex-root",
        codex_root,
    ]);
    assert_eq!(indexed["indexed_conversations"], 2);
    assert_eq!(indexed["indexed_messages"], 3);

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

// Veritas claim: ingestion/unsupported-providers-ignored
#[test]
fn unsupported_provider_records_are_ignored() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let claude_root = directory.path().join("claude");
    let codex_root = directory.path().join("codex");
    std::fs::create_dir_all(&claude_root).expect("Claude root");
    std::fs::create_dir_all(&codex_root).expect("Codex root");
    std::fs::write(
        claude_root.join("opencode.jsonl"),
        "{\"type\":\"message\",\"role\":\"user\",\"content\":\"not supported\"}\n",
    )
    .expect("unsupported fixture");

    let database = directory.path().join("cass.sqlite3");
    let indexed = run_json(&[
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "index",
        "--claude-root",
        claude_root.to_str().expect("UTF-8 Claude path"),
        "--codex-root",
        codex_root.to_str().expect("UTF-8 Codex path"),
    ]);

    assert_eq!(indexed["scanned_files"], 1);
    assert_eq!(indexed["indexed_conversations"], 0);
    assert_eq!(indexed["indexed_messages"], 0);
}

// Veritas claims: ingestion/malformed-records-do-not-panic,
// ingestion/supported-jsonl-indexes, storage/full-rebuild-is-idempotent
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
    let arguments = [
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "index",
        "--full",
        "--claude-root",
        claude_root.to_str().expect("UTF-8 Claude path"),
        "--codex-root",
        codex_root.to_str().expect("UTF-8 Codex path"),
    ];
    let first = run_json(&arguments);
    let second = run_json(&arguments);

    assert_eq!(first["malformed_records"], 1);
    assert_eq!(first["indexed_conversations"], 1);
    assert_eq!(first["indexed_messages"], 1);
    assert_eq!(second["malformed_records"], 1);
    assert_eq!(second["indexed_conversations"], 1);
    assert_eq!(second["indexed_messages"], 1);

    let found = run_json(&[
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "search",
        "persistent",
    ]);
    assert_eq!(found["results"].as_array().map(Vec::len), Some(1));
}

// Veritas claims: semantic/missing-models-fall-back,
// models/download-is-explicit
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
        .args([
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "index",
            "--claude-root",
            claude_root.to_str().expect("UTF-8 Claude path"),
            "--codex-root",
            codex_root.to_str().expect("UTF-8 Codex path"),
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

// Veritas claim: semantic/hybrid-reranks-with-models
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
    let indexed = run_json(&[
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "--models-dir",
        models_dir,
        "index",
        "--full",
        "--claude-root",
        claude_root.to_str().expect("UTF-8 Claude path"),
        "--codex-root",
        codex_root.to_str().expect("UTF-8 Codex path"),
    ]);
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
