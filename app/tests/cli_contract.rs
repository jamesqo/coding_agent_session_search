use assert_cmd::Command;
use predicates::prelude::*;
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

fn create_opencode_fixture(path: &std::path::Path) {
    let connection = rusqlite::Connection::open(path).expect("OpenCode fixture database");
    connection
        .execute_batch(
            r#"
            CREATE TABLE session (
                id TEXT PRIMARY KEY, project_id TEXT NOT NULL,
                directory TEXT NOT NULL, title TEXT NOT NULL,
                time_created INTEGER, time_updated INTEGER
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                time_created INTEGER, data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY, message_id TEXT NOT NULL,
                session_id TEXT NOT NULL, time_created INTEGER,
                data TEXT NOT NULL
            );
            INSERT INTO session VALUES
                ('open-session', 'project-1', '/work/open', 'OpenCode proof',
                 1700000000000, 1700000002000);
            INSERT INTO message VALUES
                ('open-user', 'open-session', 1700000001000,
                 '{"role":"user"}'),
                ('open-assistant', 'open-session', 1700000002000,
                 '{"role":"assistant"}');
            INSERT INTO part VALUES
                ('part-user', 'open-user', 'open-session', 1700000001000,
                 '{"type":"text","text":"locate the opalescent-otter"}'),
                ('part-assistant', 'open-assistant', 'open-session', 1700000002000,
                 '{"type":"text","text":"the otter is in OpenCode"}'),
                ('part-bad', 'open-assistant', 'open-session', 1700000002001,
                 'not-json');
            "#,
        )
        .expect("OpenCode fixture schema");
}

fn create_copilot_fixture(root: &std::path::Path) {
    let session = root.join("copilot-session");
    std::fs::create_dir_all(&session).expect("Copilot session root");
    std::fs::write(
        session.join("events.jsonl"),
        concat!(
            "{\"type\":\"user.message\",\"data\":{\"content\":\"find the cerulean-ibis\"},\"timestamp\":\"2026-03-01T10:00:01Z\"}\n",
            "not-json\n",
            "{\"type\":\"tool.execution_start\",\"data\":{\"content\":\"ignore me\"}}\n",
            "{\"type\":\"assistant.message\",\"data\":{\"content\":\"the ibis is in Copilot CLI\"},\"timestamp\":\"2026-03-01T10:00:02Z\"}\n",
        ),
    )
    .expect("Copilot fixture");
}

fn create_hermes_fixture(path: &std::path::Path) {
    let connection = rusqlite::Connection::open(path).expect("Hermes fixture database");
    connection
        .execute_batch(
            r"
            CREATE TABLE sessions (
                id TEXT PRIMARY KEY, source TEXT NOT NULL, title TEXT,
                started_at REAL NOT NULL, ended_at REAL
            );
            CREATE TABLE messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL, role TEXT NOT NULL, content TEXT,
                reasoning TEXT, timestamp REAL NOT NULL
            );
            INSERT INTO sessions VALUES
                ('hermes-session', 'cli', 'Hermes proof',
                 1700000000.0, 1700000002.0);
            INSERT INTO messages(session_id, role, content, reasoning, timestamp)
                VALUES
                ('hermes-session', 'session_meta', 'ignore metadata', NULL,
                 1700000000.0),
                ('hermes-session', 'user', 'locate the vermilion-vicuna', NULL,
                 1700000001.0),
                ('hermes-session', 'assistant', 'the vicuna is in Hermes',
                 'checked canonical storage', 1700000002.0);
            ",
        )
        .expect("Hermes fixture schema");
}

fn create_pi_fixture(root: &std::path::Path) {
    let project = root.join("--work-pi-project--");
    std::fs::create_dir_all(&project).expect("Pi project session root");
    std::fs::write(
        project.join("2026-08-25T01-00-00_pi-session.jsonl"),
        concat!(
            "{\"type\":\"session\",\"version\":3,\"id\":\"pi-session\",\"timestamp\":\"2026-08-25T01:00:00Z\",\"cwd\":\"/work/pi-project\"}\n",
            "{not-json}\n",
            "{\"type\":\"model_change\",\"id\":\"model-1\",\"parentId\":null,\"provider\":\"anthropic\",\"modelId\":\"claude-opus-4-1\"}\n",
            "{\"type\":\"message\",\"id\":\"pi-user\",\"parentId\":\"model-1\",\"timestamp\":\"2026-08-25T01:00:01Z\",\"message\":{\"role\":\"user\",\"content\":\"locate the chartreuse-chinchilla\",\"timestamp\":1787619601000}}\n",
            "{\"type\":\"message\",\"id\":\"pi-assistant\",\"parentId\":\"pi-user\",\"timestamp\":\"2026-08-25T01:00:02Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"thinking\",\"thinking\":\"inspect the current tree\"},{\"type\":\"text\",\"text\":\"the chinchilla is in Pi\"},{\"type\":\"toolCall\",\"id\":\"call-1\",\"name\":\"read\",\"arguments\":{\"path\":\"app/ingestion.rs\"}}],\"timestamp\":1787619602000}}\n",
            "{\"type\":\"message\",\"id\":\"pi-tool\",\"parentId\":\"pi-assistant\",\"timestamp\":\"2026-08-25T01:00:03Z\",\"message\":{\"role\":\"toolResult\",\"toolCallId\":\"call-1\",\"toolName\":\"read\",\"content\":[{\"type\":\"text\",\"text\":\"reader output\"}],\"isError\":false,\"timestamp\":1787619603000}}\n",
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
        "--opencode-db",
        codex_root,
        "--copilot-root",
        codex_root,
        "--hermes-db",
        codex_root,
        "--pi-root",
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

#[veritas::claims(
    "ingestion/provider-boundary",
    "ingestion/supported-jsonl-indexes",
    "ingestion/malformed-records-do-not-panic"
)]
#[test]
fn opencode_and_copilot_cli_histories_are_searchable() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let claude_root = directory.path().join("claude");
    let codex_root = directory.path().join("codex");
    let copilot_root = directory.path().join("session-state");
    std::fs::create_dir_all(&claude_root).expect("Claude root");
    std::fs::create_dir_all(&codex_root).expect("Codex root");

    let opencode_db = directory.path().join("opencode.db");
    create_opencode_fixture(&opencode_db);
    create_copilot_fixture(&copilot_root);

    let database = directory.path().join("cass.sqlite3");
    let database = database.to_str().expect("UTF-8 database path");
    let indexed = run_json(&[
        "--db",
        database,
        "index",
        "--full",
        "--claude-root",
        claude_root.to_str().expect("UTF-8 Claude path"),
        "--codex-root",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--opencode-db",
        opencode_db.to_str().expect("UTF-8 OpenCode path"),
        "--copilot-root",
        copilot_root.to_str().expect("UTF-8 Copilot path"),
        "--hermes-db",
        claude_root.to_str().expect("UTF-8 Claude path"),
        "--pi-root",
        claude_root.to_str().expect("UTF-8 Claude path"),
    ]);
    assert_eq!(indexed["indexed_conversations"], 2);
    assert_eq!(indexed["indexed_messages"], 4);
    assert_eq!(indexed["scanned_files"], 2);
    assert_eq!(indexed["malformed_records"], 2);

    for (query, provider, conversation_id) in [
        ("opalescent", "opencode", "open-session"),
        ("cerulean", "github-copilot", "copilot-session"),
    ] {
        let found = run_json(&["--db", database, "search", query, "--provider", provider]);
        assert_eq!(found["results"].as_array().map(Vec::len), Some(1));
        assert_eq!(found["results"][0]["conversation_id"], conversation_id);
        let message_id = found["results"][0]["id"].as_str().expect("message ID");
        let viewed = run_json(&["--db", database, "view", message_id, "--context", "10"]);
        assert_eq!(viewed["messages"].as_array().map(Vec::len), Some(2));
    }

    let reindexed = run_json(&[
        "--db",
        database,
        "index",
        "--full",
        "--claude-root",
        claude_root.to_str().expect("UTF-8 Claude path"),
        "--codex-root",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--opencode-db",
        opencode_db.to_str().expect("UTF-8 OpenCode path"),
        "--copilot-root",
        copilot_root.to_str().expect("UTF-8 Copilot path"),
        "--hermes-db",
        claude_root.to_str().expect("UTF-8 Claude path"),
        "--pi-root",
        claude_root.to_str().expect("UTF-8 Claude path"),
    ]);
    assert_eq!(reindexed["indexed_conversations"], 2);
    assert_eq!(reindexed["indexed_messages"], 4);
}

#[veritas::claims("ingestion/provider-boundary", "ingestion/supported-jsonl-indexes")]
#[test]
fn hermes_histories_are_searchable() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let empty_root = directory.path().join("empty");
    std::fs::create_dir_all(&empty_root).expect("empty provider root");
    let hermes_db = directory.path().join("state.db");
    create_hermes_fixture(&hermes_db);
    let database = directory.path().join("cass.sqlite3");
    let database = database.to_str().expect("UTF-8 database path");
    let empty_root = empty_root.to_str().expect("UTF-8 empty path");
    let hermes_db = hermes_db.to_str().expect("UTF-8 Hermes path");
    let arguments = [
        "--db",
        database,
        "index",
        "--full",
        "--claude-root",
        empty_root,
        "--codex-root",
        empty_root,
        "--opencode-db",
        empty_root,
        "--copilot-root",
        empty_root,
        "--hermes-db",
        hermes_db,
        "--pi-root",
        empty_root,
    ];
    let indexed = run_json(&arguments);
    assert_eq!(indexed["indexed_conversations"], 1);
    assert_eq!(indexed["indexed_messages"], 2);
    assert_eq!(indexed["scanned_files"], 1);

    let found = run_json(&[
        "--db",
        database,
        "search",
        "vermilion",
        "--provider",
        "hermes",
    ]);
    assert_eq!(found["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(found["results"][0]["conversation_id"], "hermes-session");
    let message_id = found["results"][0]["id"].as_str().expect("message ID");
    let viewed = run_json(&["--db", database, "view", message_id, "--context", "10"]);
    assert_eq!(viewed["messages"].as_array().map(Vec::len), Some(2));

    let reindexed = run_json(&arguments);
    assert_eq!(reindexed["indexed_conversations"], 1);
    assert_eq!(reindexed["indexed_messages"], 2);
}

#[veritas::claims(
    "ingestion/provider-boundary",
    "ingestion/supported-jsonl-indexes",
    "ingestion/malformed-records-do-not-panic",
    "storage/full-rebuild-is-idempotent"
)]
#[test]
fn pi_histories_are_searchable() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let empty_root = directory.path().join("empty");
    let pi_root = directory.path().join("pi-sessions");
    std::fs::create_dir_all(&empty_root).expect("empty provider root");
    create_pi_fixture(&pi_root);
    let database = directory.path().join("cass.sqlite3");
    let database = database.to_str().expect("UTF-8 database path");
    let empty_root = empty_root.to_str().expect("UTF-8 empty path");
    let pi_root = pi_root.to_str().expect("UTF-8 Pi path");
    let arguments = [
        "--db",
        database,
        "index",
        "--full",
        "--claude-root",
        empty_root,
        "--codex-root",
        empty_root,
        "--opencode-db",
        empty_root,
        "--copilot-root",
        empty_root,
        "--hermes-db",
        empty_root,
        "--pi-root",
        pi_root,
    ];
    let indexed = run_json(&arguments);
    assert_eq!(indexed["indexed_conversations"], 1);
    assert_eq!(indexed["indexed_messages"], 3);
    assert_eq!(indexed["scanned_files"], 1);
    assert_eq!(indexed["malformed_records"], 1);

    let found = run_json(&["--db", database, "search", "chartreuse", "--provider", "pi"]);
    assert_eq!(found["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(found["results"][0]["conversation_id"], "pi-session");
    let message_id = found["results"][0]["id"].as_str().expect("message ID");
    let viewed = run_json(&["--db", database, "view", message_id, "--context", "10"]);
    assert_eq!(viewed["messages"].as_array().map(Vec::len), Some(3));

    let reindexed = run_json(&arguments);
    assert_eq!(reindexed["indexed_conversations"], 1);
    assert_eq!(reindexed["indexed_messages"], 3);
}

#[veritas::claims("ingestion/unsupported-providers-ignored")]
#[test]
fn unsupported_provider_records_are_ignored() {
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
    let indexed = run_json(&[
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "index",
        "--claude-root",
        claude_root.to_str().expect("UTF-8 Claude path"),
        "--codex-root",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--opencode-db",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--copilot-root",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--hermes-db",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--pi-root",
        codex_root.to_str().expect("UTF-8 Codex path"),
    ]);

    assert_eq!(indexed["scanned_files"], 1);
    assert_eq!(indexed["indexed_conversations"], 0);
    assert_eq!(indexed["indexed_messages"], 0);
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
    let arguments = [
        "--db",
        database.to_str().expect("UTF-8 database path"),
        "index",
        "--full",
        "--claude-root",
        claude_root.to_str().expect("UTF-8 Claude path"),
        "--codex-root",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--opencode-db",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--copilot-root",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--hermes-db",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--pi-root",
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
        .args([
            "--db",
            database.to_str().expect("UTF-8 database path"),
            "index",
            "--claude-root",
            claude_root.to_str().expect("UTF-8 Claude path"),
            "--codex-root",
            codex_root.to_str().expect("UTF-8 Codex path"),
            "--opencode-db",
            codex_root.to_str().expect("UTF-8 Codex path"),
            "--copilot-root",
            codex_root.to_str().expect("UTF-8 Codex path"),
            "--hermes-db",
            codex_root.to_str().expect("UTF-8 Codex path"),
            "--pi-root",
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
        "--opencode-db",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--copilot-root",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--hermes-db",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--pi-root",
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
    run_json(&[
        "--db",
        database,
        "index",
        "--claude-root",
        claude_root.to_str().expect("UTF-8 Claude path"),
        "--codex-root",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--opencode-db",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--copilot-root",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--hermes-db",
        codex_root.to_str().expect("UTF-8 Codex path"),
        "--pi-root",
        codex_root.to_str().expect("UTF-8 Codex path"),
    ]);

    let all = run_json(&["--db", database, "search", "temporal"]);
    assert_eq!(all["results"].as_array().map(Vec::len), Some(2));
    let recent = run_json(&["--db", database, "search", "temporal", "--days", "30"]);
    assert_eq!(recent["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(recent["results"][0]["content"], "temporal new");
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
        concat!("Dickles", "worthstone"),
    ];
    let mut maintained = String::new();
    let mut production_lines = 0;
    let mut test_lines = 0;
    for file in [
        "Cargo.toml",
        "Cargo.lock",
        "README.md",
        "AGENTS.md",
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
