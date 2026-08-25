use std::path::Path;

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
            "INSERT INTO message_embeddings(message_id, generation, dimensions, vector)
             VALUES (?1, ?2, 1, ?3)",
            params![
                message_id,
                blake3::hash(
                    concat!(
                        "fastembed=6.0.1;model=AllMiniLML6V2Q;",
                        "vector=f32-little-endian;cosine=exact;schema=1"
                    )
                    .as_bytes()
                )
                .to_hex()
                .to_string(),
                1.0_f32.to_le_bytes().to_vec()
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
