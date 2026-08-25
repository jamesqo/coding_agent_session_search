use super::*;
use std::path::PathBuf;
use veritas_test_macros as veritas;

fn conversation(content: &str) -> Conversation {
    Conversation {
        id: "session-1".to_owned(),
        provider: "codex",
        source_path: PathBuf::from("/tmp/session-1.jsonl"),
        title: Some("session".to_owned()),
        created_at: Some(1),
        updated_at: Some(2),
        messages: vec![crate::ingestion::NormalizedMessage {
            id: "message-1".to_owned(),
            ordinal: 0,
            role: "user".to_owned(),
            content: content.to_owned(),
            search_projection: None,
            created_at: Some(i64::MAX / 2),
        }],
    }
}

fn conversation_with_messages(messages: &[(&str, &str)]) -> Conversation {
    let mut value = conversation("");
    value.messages = messages
        .iter()
        .enumerate()
        .map(
            |(ordinal, (id, content))| crate::ingestion::NormalizedMessage {
                id: (*id).to_owned(),
                ordinal: i64::try_from(ordinal).expect("test ordinal"),
                role: "user".to_owned(),
                content: (*content).to_owned(),
                search_projection: None,
                created_at: Some(i64::MAX / 2),
            },
        )
        .collect();
    value.created_at = Some(i64::MAX / 2);
    value.updated_at = Some(i64::MAX / 2);
    value
}

#[veritas::claims("storage/full-rebuild-is-idempotent")]
#[test]
fn full_rebuild_is_idempotent() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut storage = Storage::open(&directory.path().join("cass.sqlite3")).expect("open database");
    storage
        .rebuild_derived_search_state()
        .expect("first rebuild");
    storage
        .rebuild_derived_search_state()
        .expect("second rebuild");
    let counts = storage.counts().expect("counts");
    assert_eq!(counts.conversations, 0);
    assert_eq!(counts.messages, 0);
    assert_eq!(counts.embeddings, 0);
}

#[test]
fn writer_checkpoint_survives_a_later_rollback() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&conversation("durable needle"))
        .expect("durable mutation");
    writer
        .record_source_checkpoint("codex", "/tmp/session-1.jsonl", 100, 200)
        .expect("durable source checkpoint");
    writer.checkpoint_writer().expect("checkpoint batch");
    writer
        .replace_conversation(&conversation("rolled back text"))
        .expect("later mutation");
    drop(writer);

    let storage = Storage::open_existing(&path).expect("reopen database");
    assert_eq!(
        storage
            .search("durable", 10, None, None)
            .expect("search committed batch")
            .len(),
        1
    );
    assert!(
        storage
            .source_checkpoint_matches("codex", "/tmp/session-1.jsonl", 100, 200)
            .expect("read checkpoint")
    );
}

#[test]
fn semantic_readiness_changes_atomically_with_canonical_messages() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&conversation("original text"))
        .expect("seed conversation");
    writer
        .replace_embeddings(
            "generation",
            &[EmbeddingWrite {
                message_id: "message-1",
                vector: &[127],
                norm: 127.0,
            }],
        )
        .expect("seed embedding");
    writer
        .mark_semantic_index_ready("generation")
        .expect("mark initial index ready");
    writer.commit_writer().expect("commit ready index");

    let storage = Storage::open_existing(&path).expect("open ready database");
    assert!(
        storage
            .semantic_index_is_ready("generation")
            .expect("read initial readiness")
    );
    drop(storage);

    let mut rolled_back = Storage::open_writer(&path).expect("rollback writer");
    rolled_back
        .replace_conversation(&conversation("rolled back change"))
        .expect("change canonical message");
    assert!(
        !rolled_back
            .semantic_index_is_ready("generation")
            .expect("read transactional invalidation")
    );
    drop(rolled_back);

    let storage = Storage::open_existing(&path).expect("reopen after rollback");
    assert!(
        storage
            .semantic_index_is_ready("generation")
            .expect("read readiness after rollback")
    );
    drop(storage);

    let mut committed = Storage::open_writer(&path).expect("commit writer");
    committed
        .replace_conversation(&conversation("committed change"))
        .expect("change canonical message");
    committed.checkpoint_writer().expect("commit invalidation");
    assert!(
        !committed
            .semantic_index_is_ready("generation")
            .expect("read committed invalidation")
    );
    committed.commit_writer().expect("finish writer");

    let storage = Storage::open_existing(&path).expect("reopen incomplete database");
    assert!(
        !storage
            .semantic_index_is_ready("generation")
            .expect("read incomplete readiness")
    );
}

#[veritas::claims("indexing/canonical-and-fts-are-atomic")]
#[test]
fn incremental_fts_and_canonical_changes_roll_back_together() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&conversation("durable old term"))
        .expect("seed conversation");
    writer.commit_writer().expect("commit seed");
    drop(writer);

    let mut writer = Storage::open_writer(&path).expect("replacement writer");
    writer
        .replace_conversation(&conversation("uncommitted new term"))
        .expect("replace conversation");
    assert_eq!(
        writer
            .finalize_pending_fts_updates(u64::MAX)
            .expect("incremental FTS finalization"),
        FtsRefreshStrategy::Incremental
    );
    assert_eq!(
        writer.search("uncommitted", 10, None, None).unwrap().len(),
        1
    );
    drop(writer);

    let storage = Storage::open_existing(&path).expect("reopen database");
    assert_eq!(storage.search("durable", 10, None, None).unwrap().len(), 1);
    assert!(
        storage
            .search("uncommitted", 10, None, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        storage.view("message-1", 0).unwrap()[0].content,
        "durable old term"
    );
}

#[veritas::claims("indexing/canonical-and-fts-are-atomic")]
#[test]
fn bulk_fts_and_canonical_changes_roll_back_together() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&conversation("durable bulk term"))
        .expect("seed conversation");
    writer.commit_writer().expect("commit seed");
    drop(writer);

    let mut writer = Storage::open_writer(&path).expect("replacement writer");
    writer
        .replace_conversation(&conversation("uncommitted bulk replacement"))
        .expect("replace conversation");
    assert_eq!(
        writer
            .finalize_pending_fts_updates(1)
            .expect("bulk FTS finalization"),
        FtsRefreshStrategy::Bulk
    );
    assert_eq!(
        writer.search("replacement", 10, None, None).unwrap().len(),
        1
    );
    drop(writer);

    let storage = Storage::open_existing(&path).expect("reopen database");
    assert_eq!(storage.search("durable", 10, None, None).unwrap().len(), 1);
    assert!(
        storage
            .search("replacement", 10, None, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        storage.view("message-1", 0).unwrap()[0].content,
        "durable bulk term"
    );
}

#[test]
fn fts_strategy_switches_at_the_declared_boundary() {
    let directory = tempfile::tempdir().expect("temporary directory");
    for (changed, expected) in [
        (8_usize, FtsRefreshStrategy::Incremental),
        (9, FtsRefreshStrategy::Bulk),
    ] {
        let path = directory.path().join(format!("changed-{changed}.sqlite3"));
        let original = (0..10)
            .map(|index| (format!("message-{index}"), format!("original {index}")))
            .collect::<Vec<_>>();
        let original_refs = original
            .iter()
            .map(|(id, content)| (id.as_str(), content.as_str()))
            .collect::<Vec<_>>();
        let mut writer = Storage::open_writer(&path).expect("writer");
        writer
            .replace_conversation(&conversation_with_messages(&original_refs))
            .expect("seed messages");
        writer.commit_writer().expect("commit seed");
        drop(writer);

        let replacement = original
            .iter()
            .enumerate()
            .map(|(index, (id, content))| {
                let content = if index < changed {
                    format!("changed {index}")
                } else {
                    content.clone()
                };
                (id.clone(), content)
            })
            .collect::<Vec<_>>();
        let replacement_refs = replacement
            .iter()
            .map(|(id, content)| (id.as_str(), content.as_str()))
            .collect::<Vec<_>>();
        let mut writer = Storage::open_writer(&path).expect("replacement writer");
        writer
            .replace_conversation(&conversation_with_messages(&replacement_refs))
            .expect("replace messages");
        let threshold = writer.measured_fts_bulk_threshold().unwrap();
        assert_eq!(threshold, 9);
        assert_eq!(
            writer.finalize_pending_fts_updates(threshold).unwrap(),
            expected,
            "changed messages: {changed}"
        );
    }

    let deletion_path = directory.path().join("deletion.sqlite3");
    let original = (0..10)
        .map(|index| (format!("message-{index}"), format!("original {index}")))
        .collect::<Vec<_>>();
    let original_refs = original
        .iter()
        .map(|(id, content)| (id.as_str(), content.as_str()))
        .collect::<Vec<_>>();
    let mut writer = Storage::open_writer(&deletion_path).expect("deletion writer");
    writer
        .replace_conversation(&conversation_with_messages(&original_refs))
        .expect("seed deletion corpus");
    writer.commit_writer().expect("commit deletion corpus");
    drop(writer);
    let retained_refs = original_refs[..5].to_vec();
    let mut writer = Storage::open_writer(&deletion_path).expect("deletion writer");
    writer
        .replace_conversation(&conversation_with_messages(&retained_refs))
        .expect("delete half the messages");
    let threshold = writer.measured_fts_bulk_threshold().unwrap();
    assert_eq!(threshold, 9, "deletions use the pre-transaction corpus");
    assert_eq!(
        writer.finalize_pending_fts_updates(threshold).unwrap(),
        FtsRefreshStrategy::Incremental
    );
}

#[test]
fn incremental_and_bulk_fts_produce_equivalent_results() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let incremental_path = directory.path().join("incremental.sqlite3");
    let bulk_path = directory.path().join("bulk.sqlite3");
    for (path, cutoff) in [(&incremental_path, u64::MAX), (&bulk_path, 1)] {
        let mut writer = Storage::open_writer(path).expect("writer");
        writer
            .replace_conversation(&conversation_with_messages(&[
                ("message-1", "alpha shared"),
                ("message-2", "beta shared"),
                ("message-3", "removed sentinel"),
            ]))
            .expect("seed messages");
        writer
            .finalize_pending_fts_updates(cutoff)
            .expect("seed FTS");
        writer
            .replace_conversation(&conversation_with_messages(&[
                ("message-1", "gamma shared"),
                ("message-4", "delta shared"),
            ]))
            .expect("replace messages");
        writer
            .finalize_pending_fts_updates(cutoff)
            .expect("refresh FTS");
        writer.commit_writer().expect("commit corpus");
    }

    let incremental = Storage::open_existing(&incremental_path).unwrap();
    let bulk = Storage::open_existing(&bulk_path).unwrap();
    for (query, limit, provider, days) in [
        ("shared", 10, None, None),
        ("shared", 1, Some("codex"), None),
        ("shared", 10, Some("codex"), Some(1)),
        ("gamma", 10, Some("codex"), None),
        ("removed", 10, None, None),
    ] {
        let ids = |storage: &Storage| {
            storage
                .search(query, limit, provider, days)
                .unwrap()
                .into_iter()
                .map(|hit| hit.id)
                .collect::<Vec<_>>()
        };
        let incremental_ids = ids(&incremental);
        let bulk_ids = ids(&bulk);
        assert_eq!(incremental_ids, bulk_ids, "query {query}");
        if days.is_some() {
            assert!(
                !incremental_ids.is_empty(),
                "recency-filter equivalence must exercise matching rows"
            );
        }
    }
    assert!(
        incremental
            .search("alpha", 10, None, None)
            .unwrap()
            .is_empty()
    );
    assert!(
        incremental
            .search("beta", 10, None, None)
            .unwrap()
            .is_empty()
    );
    assert!(
        incremental
            .search("removed", 10, None, None)
            .unwrap()
            .is_empty()
    );
}

#[veritas::claims(
    "search/tool-results-are-not-searchable",
    "search/mixed-message-excludes-tool-result-text",
    "view/tool-results-remain-visible"
)]
#[test]
fn search_projection_controls_fts_embeddings_and_rerank_text_not_view() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut corpus = conversation_with_messages(&[
        ("tool-only", "private tool payload"),
        ("mixed", "visible request\nprivate mixed payload"),
        ("ordinary", "ordinary searchable text"),
    ]);
    corpus.messages[0].search_projection = Some(String::new());
    corpus.messages[1].search_projection = Some("visible request".to_owned());

    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&corpus)
        .expect("store projected messages");
    let pending = writer
        .messages_needing_embeddings()
        .expect("embedding selection");
    assert_eq!(
        pending
            .iter()
            .map(|message| (message.id.as_str(), message.content.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("mixed", "visible request"),
            ("ordinary", "ordinary searchable text")
        ]
    );
    writer
        .replace_embeddings(
            "generation",
            &[
                EmbeddingWrite {
                    message_id: "tool-only",
                    vector: &[127],
                    norm: 127.0,
                },
                EmbeddingWrite {
                    message_id: "mixed",
                    vector: &[126],
                    norm: 126.0,
                },
                EmbeddingWrite {
                    message_id: "ordinary",
                    vector: &[125],
                    norm: 125.0,
                },
            ],
        )
        .expect("seed semantic vectors");
    writer.commit_writer().expect("commit projected messages");

    let storage = Storage::open_existing(&path).expect("open projected corpus");
    let counts = storage.counts().expect("counts");
    assert_eq!(counts.messages, 3);
    assert_eq!(counts.searchable_messages, 2);
    assert!(
        storage
            .search("private", 10, None, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(storage.search("visible", 10, None, None).unwrap().len(), 1);
    assert_eq!(storage.search("ordinary", 10, None, None).unwrap().len(), 1);

    let vectors = storage
        .semantic_chunks("generation", None, None)
        .expect("semantic vectors");
    assert_eq!(vectors.chunks.len(), 1);
    assert_eq!(vectors.chunks[0].message_rowids, [2, 3]);
    storage
        .connection
        .execute(
            "DELETE FROM message_embeddings WHERE message_id = 'ordinary'",
            [],
        )
        .expect("remove one searchable vector");
    assert_eq!(storage.embedding_count("generation").unwrap(), 2);
    assert_eq!(counts.searchable_messages, 2);
    assert!(
        !storage
            .semantic_coverage_is_complete("generation")
            .expect("exact semantic coverage")
    );
    assert_eq!(
        storage
            .search_documents(&["ordinary", "mixed"])
            .expect("rerank documents"),
        vec![
            "ordinary searchable text".to_owned(),
            "visible request".to_owned()
        ]
    );
    let context = storage.view("mixed", 1).expect("canonical view");
    assert_eq!(context[0].content, "private tool payload");
    assert_eq!(context[1].content, "visible request\nprivate mixed payload");
}

#[test]
fn bulk_fts_refresh_preserves_unchanged_embeddings() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&conversation_with_messages(&[
            ("message-1", "first"),
            ("message-2", "second"),
        ]))
        .expect("seed messages");
    writer
        .replace_embeddings(
            "generation-a",
            &[
                EmbeddingWrite {
                    message_id: "message-1",
                    vector: &[127],
                    norm: 127.0,
                },
                EmbeddingWrite {
                    message_id: "message-2",
                    vector: &[127],
                    norm: 127.0,
                },
            ],
        )
        .expect("seed embeddings");
    writer
        .finalize_pending_fts_updates(1)
        .expect("seed bulk FTS");

    writer
        .replace_conversation(&conversation_with_messages(&[
            ("message-1", "changed first"),
            ("message-2", "second"),
        ]))
        .expect("change one message");
    assert_eq!(
        writer.finalize_pending_fts_updates(1).unwrap(),
        FtsRefreshStrategy::Bulk
    );
    assert_eq!(writer.embedding_count("generation-a").unwrap(), 1);
    assert_eq!(
        writer
            .messages_needing_embeddings()
            .unwrap()
            .into_iter()
            .map(|message| message.id)
            .collect::<Vec<_>>(),
        ["message-1"]
    );
}

#[ignore = "manual FTS crossover benchmark"]
#[test]
fn benchmark_fts_refresh_crossover() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let benchmark_path = directory.path().join("benchmark.sqlite3");
    if let Some(source) = std::env::var_os("CASS_FTS_BENCH_DB") {
        let source = Connection::open(PathBuf::from(source)).expect("open benchmark source");
        source
            .execute(
                "VACUUM INTO ?1",
                [benchmark_path.to_string_lossy().as_ref()],
            )
            .expect("copy benchmark database");
    } else {
        let mut writer = Storage::open_writer(&benchmark_path).expect("writer");
        let messages = (0..25_000)
            .map(|index| crate::ingestion::NormalizedMessage {
                id: format!("message-{index:05}"),
                ordinal: i64::from(index),
                role: "user".to_owned(),
                content: format!("representative searchable text number {index}"),
                search_projection: None,
                created_at: Some(1),
            })
            .collect();
        let mut corpus = conversation("");
        corpus.messages = messages;
        writer
            .replace_conversation(&corpus)
            .expect("seed benchmark corpus");
        writer.commit_writer().expect("commit benchmark corpus");
    }

    let reader = Storage::open_existing(&benchmark_path).expect("benchmark reader");
    let total = reader.counts().expect("benchmark counts").messages;
    drop(reader);
    let mut deltas = vec![
        1,
        10,
        100,
        1_000,
        10_000,
        total / 10,
        total / 2,
        total.saturating_mul(75) / 100,
        total.saturating_mul(85) / 100,
        total.saturating_mul(90) / 100,
        total.saturating_mul(95) / 100,
        total,
    ];
    deltas.retain(|delta| *delta > 0 && *delta <= total);
    deltas.sort_unstable();
    deltas.dedup();

    eprintln!("fts benchmark corpus_messages={total}");
    for delta in deltas {
        let mut incremental = Vec::new();
        let mut bulk = Vec::new();
        for repetition in 0..3 {
            let order = if repetition % 2 == 0 {
                [(u64::MAX, &mut incremental), (1, &mut bulk)]
            } else {
                [(1, &mut bulk), (u64::MAX, &mut incremental)]
            };
            for (cutoff, timings) in order {
                let mut writer = Storage::open_writer(&benchmark_path).expect("benchmark writer");
                writer
                    .connection
                    .execute(
                        "INSERT INTO pending_fts_messages(message_id)
                         SELECT id FROM messages ORDER BY id LIMIT ?1",
                        [i64::try_from(delta).expect("delta")],
                    )
                    .expect("stage benchmark messages");
                writer
                    .connection
                    .execute(
                        "UPDATE messages SET content = content || ' changed'
                          WHERE id IN (SELECT message_id FROM pending_fts_messages)",
                        [],
                    )
                    .expect("mutate benchmark messages");
                let started = std::time::Instant::now();
                writer
                    .finalize_pending_fts_updates(cutoff)
                    .expect("benchmark FTS finalization");
                timings.push(started.elapsed());
                drop(writer);
            }
        }
        incremental.sort_unstable();
        bulk.sort_unstable();
        eprintln!(
            "delta={delta} incremental_ms={:.3} bulk_ms={:.3}",
            incremental[1].as_secs_f64() * 1_000.0,
            bulk[1].as_secs_f64() * 1_000.0
        );
    }
}

#[test]
fn dirty_search_state_survives_a_batch_and_rebuilds_after_resume() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .defer_search_updates()
        .expect("defer FTS maintenance");
    writer
        .replace_conversation(&conversation("resumable needle"))
        .expect("message batch");
    writer.checkpoint_writer().expect("durable message batch");
    drop(writer);

    let mut resumed = Storage::open_writer(&path).expect("resumed writer");
    assert!(
        resumed
            .derived_search_is_dirty()
            .expect("dirty search marker")
    );
    assert!(
        resumed
            .search("resumable", 10, None, None)
            .expect("stale FTS is readable")
            .is_empty()
    );
    resumed
        .rebuild_derived_search_state()
        .expect("bulk FTS rebuild");
    assert!(
        !resumed
            .derived_search_is_dirty()
            .expect("clean search marker")
    );
    assert_eq!(
        resumed
            .search("resumable", 10, None, None)
            .expect("rebuilt search")
            .len(),
        1
    );
    resumed.commit_writer().expect("commit rebuilt state");
}

#[veritas::claims("indexing/partial-embeddings-resume")]
#[test]
fn committed_embedding_checkpoint_resumes_from_only_missing_rows() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&conversation_with_messages(&[
            ("message-1", "first searchable message"),
            ("message-2", "second searchable message"),
        ]))
        .expect("seed canonical messages");
    writer
        .checkpoint_writer()
        .expect("commit canonical and FTS state");
    writer
        .replace_embeddings(
            "generation",
            &[EmbeddingWrite {
                message_id: "message-1",
                vector: &[127, 0],
                norm: 127.0,
            }],
        )
        .expect("first derived batch");
    writer
        .checkpoint_writer()
        .expect("commit first derived checkpoint");
    writer
        .replace_embeddings(
            "generation",
            &[EmbeddingWrite {
                message_id: "message-2",
                vector: &[0, 127],
                norm: 127.0,
            }],
        )
        .expect("uncommitted second derived batch");
    drop(writer);

    let mut resumed = Storage::open_writer(&path).expect("resumed writer");
    assert_eq!(
        resumed
            .search("searchable", 10, None, None)
            .expect("durable FTS rows")
            .len(),
        2
    );
    assert!(
        !resumed
            .semantic_coverage_is_complete("generation")
            .expect("partial coverage")
    );
    let missing = resumed
        .messages_needing_embeddings()
        .expect("missing embeddings");
    assert_eq!(
        missing
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        ["message-2"]
    );
    resumed
        .replace_embeddings(
            "generation",
            &[EmbeddingWrite {
                message_id: "message-2",
                vector: &[0, 127],
                norm: 127.0,
            }],
        )
        .expect("resumed derived batch");
    assert!(
        resumed
            .semantic_coverage_is_complete("generation")
            .expect("complete coverage")
    );
    resumed.commit_writer().expect("commit resumed embedding");
}

#[test]
fn full_rebuild_defers_per_message_search_writes() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut storage = Storage::open_writer(&path).expect("writer");
    storage
        .defer_search_updates()
        .expect("defer derived writes");
    storage
        .replace_conversation(&conversation("deferred needle"))
        .expect("insert conversation");
    assert_eq!(
        storage
            .connection
            .query_row("SELECT count(*) FROM message_fts", [], |row| row
                .get::<_, i64>(0))
            .expect("FTS count before rebuild"),
        0
    );

    storage
        .rebuild_derived_search_state()
        .expect("bulk rebuild");
    assert_eq!(
        storage
            .search("needle", 10, None, None)
            .expect("search rebuilt FTS")
            .len(),
        1
    );
}

#[test]
fn quantized_embedding_blobs_are_validated() {
    let vector = [129_u8, 0, 127];
    assert_eq!(validate_quantized_vector(3, 181.0, &vector), Ok(()));
    assert!(validate_quantized_vector(2, 181.0, &vector).is_err());
    assert!(validate_quantized_vector(3, f32::NAN, &vector).is_err());
}

#[test]
fn malformed_semantic_chunk_metadata_is_rejected() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&conversation("packed vector"))
        .expect("seed message");
    writer
        .replace_embeddings(
            "generation",
            &[EmbeddingWrite {
                message_id: "message-1",
                vector: &[127, 0],
                norm: 127.0,
            }],
        )
        .expect("seed embedding");
    writer
        .mark_semantic_index_ready("generation")
        .expect("publish semantic chunk");
    writer.commit_writer().expect("commit semantic chunk");
    writer
        .connection
        .execute("UPDATE semantic_chunks SET norms = X'00'", [])
        .expect("corrupt norm bytes");

    assert!(writer.semantic_chunks("generation", None, None).is_err());
}

#[test]
fn semantic_chunks_bound_incremental_rewrites_to_affected_rowid_ranges() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&conversation("first chunk"))
        .expect("seed first message");
    writer
        .connection
        .execute(
            "INSERT INTO messages(
                rowid, id, conversation_id, ordinal, role, content,
                search_projection, created_at, fingerprint
             ) VALUES (4097, 'message-4097', 'session-1', 4097, 'assistant',
                       'second chunk', NULL, 123, 'fingerprint')",
            [],
        )
        .expect("seed second chunk message");
    writer
        .replace_embeddings(
            "generation",
            &[
                EmbeddingWrite {
                    message_id: "message-1",
                    vector: &[127, 0],
                    norm: 127.0,
                },
                EmbeddingWrite {
                    message_id: "message-4097",
                    vector: &[0, 127],
                    norm: 127.0,
                },
            ],
        )
        .expect("seed embeddings");
    writer
        .mark_semantic_index_ready("generation")
        .expect("publish semantic chunks");
    writer.commit_writer().expect("commit chunks");

    let chunk_count: i64 = writer
        .connection
        .query_row("SELECT count(*) FROM semantic_chunks", [], |row| row.get(0))
        .expect("count chunks");
    assert_eq!(chunk_count, 2);
    let untouched_before: Vec<u8> = writer
        .connection
        .query_row(
            "SELECT vectors FROM semantic_chunks WHERE chunk_id = 1",
            [],
            |row| row.get(0),
        )
        .expect("second chunk before update");

    let mut writer = Storage::open_writer(&path).expect("update writer");
    writer
        .replace_embeddings(
            "generation",
            &[EmbeddingWrite {
                message_id: "message-1",
                vector: &[0, 126],
                norm: 126.0,
            }],
        )
        .expect("update first chunk embedding");
    writer
        .mark_semantic_index_ready("generation")
        .expect("republish semantic chunks");
    writer
        .commit_writer()
        .expect("commit incremental chunk update");

    let untouched_after: Vec<u8> = writer
        .connection
        .query_row(
            "SELECT vectors FROM semantic_chunks WHERE chunk_id = 1",
            [],
            |row| row.get(0),
        )
        .expect("second chunk after update");
    assert_eq!(untouched_after, untouched_before);
    let updated: Vec<u8> = writer
        .connection
        .query_row(
            "SELECT vectors FROM semantic_chunks WHERE chunk_id = 0",
            [],
            |row| row.get(0),
        )
        .expect("updated first chunk");
    assert_eq!(updated, [0, 126]);
}

#[test]
fn current_embedding_generation_cleanup_preserves_packed_chunks() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&conversation("current generation"))
        .expect("seed message");
    writer
        .replace_embeddings(
            "generation",
            &[EmbeddingWrite {
                message_id: "message-1",
                vector: &[127, 0],
                norm: 127.0,
            }],
        )
        .expect("seed embedding");
    writer.commit_writer().expect("commit semantic chunk");

    let mut writer = Storage::open_writer(&path).expect("no-op generation writer");
    assert_eq!(
        writer
            .invalidate_embedding_generation("generation")
            .expect("retain current generation"),
        0
    );
    writer
        .commit_writer()
        .expect("commit no-op generation check");
    let preserved_chunks: i64 = writer
        .connection
        .query_row("SELECT count(*) FROM semantic_chunks", [], |row| row.get(0))
        .expect("count preserved chunks");
    assert_eq!(preserved_chunks, 1);
}

#[test]
fn explicit_semantic_storage_identifiers_survive_vacuum() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&conversation("stable identifier"))
        .expect("seed message");
    writer
        .replace_embeddings(
            "generation",
            &[EmbeddingWrite {
                message_id: "message-1",
                vector: &[127, 0],
                norm: 127.0,
            }],
        )
        .expect("seed embedding");
    writer
        .mark_semantic_index_ready("generation")
        .expect("publish semantic chunk");
    writer.commit_writer().expect("commit semantic chunk");
    drop(writer);
    let maintenance = Connection::open(&path).expect("maintenance connection");
    maintenance
        .execute_batch("VACUUM")
        .expect("vacuum database");
    drop(maintenance);
    let storage = Storage::open_existing(&path).expect("open vacuumed database");
    let chunks = storage
        .semantic_chunks("generation", None, None)
        .expect("read chunks after vacuum");
    assert_eq!(chunks.chunks[0].message_rowids, [1]);
    assert_eq!(
        storage
            .semantic_chunks("generation", Some("claude-code"), None)
            .expect("apply provider metadata filter")
            .chunks[0]
            .eligible,
        [false]
    );
    assert_eq!(
        storage
            .semantic_chunks("generation", None, Some(90))
            .expect("apply timestamp metadata filter")
            .chunks[0]
            .eligible,
        [true]
    );
    assert_eq!(
        storage
            .search_hits(&[1])
            .expect("hydrate stable storage identifiers")
            .iter()
            .map(|hit| hit.id.as_str())
            .collect::<Vec<_>>(),
        ["message-1"]
    );
}

const VERSION_SEVEN_SCHEMA_FIXTURE: &str = "CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL CHECK (provider IN (
        'claude-code', 'codex', 'opencode', 'github-copilot', 'hermes', 'pi'
    )),
    source_path TEXT NOT NULL UNIQUE, title TEXT, created_at INTEGER, updated_at INTEGER,
    source_fingerprint TEXT NOT NULL DEFAULT ''
 );
 CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL,
    created_at INTEGER, fingerprint TEXT NOT NULL DEFAULT '',
    UNIQUE(conversation_id, ordinal)
 );
 CREATE TABLE message_embeddings (
    message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    generation TEXT NOT NULL DEFAULT '', dimensions INTEGER NOT NULL,
    norm REAL NOT NULL, vector BLOB NOT NULL
 );
 CREATE VIRTUAL TABLE message_fts USING fts5(
    content, message_id UNINDEXED, conversation_id UNINDEXED, tokenize = 'unicode61'
 );
 CREATE TABLE tombstones (
    provider TEXT NOT NULL, conversation_id TEXT NOT NULL,
    forgotten_at INTEGER NOT NULL, PRIMARY KEY(provider, conversation_id)
 );
 CREATE TABLE source_checkpoints (
    provider TEXT NOT NULL, source_path TEXT NOT NULL,
    size_bytes INTEGER NOT NULL, modified_ns INTEGER NOT NULL,
    PRIMARY KEY(provider, source_path)
 );
 CREATE TABLE derived_state (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    search_dirty INTEGER NOT NULL CHECK(search_dirty IN (0, 1))
 );
 INSERT INTO derived_state VALUES (1, 0);
 INSERT INTO conversations(id, provider, source_path, source_fingerprint)
    VALUES ('session-1', 'codex', '/tmp/session-1.jsonl', 'old-source');
 INSERT INTO messages(id, conversation_id, ordinal, role, content, fingerprint)
    VALUES ('message-1', 'session-1', 0, 'user', 'preserved', 'old-message');
 INSERT INTO message_fts(content, message_id, conversation_id)
    VALUES ('preserved', 'message-1', 'session-1');
 INSERT INTO message_embeddings(message_id, generation, dimensions, norm, vector)
    VALUES ('message-1', 'old-generation', 1, 1.0, X'7F');
 INSERT INTO source_checkpoints(provider, source_path, size_bytes, modified_ns)
    VALUES ('codex', '/tmp/session-1.jsonl', 10, 20);
 PRAGMA user_version = 7;";

fn version_eight_provider_fixture() -> String {
    VERSION_SEVEN_SCHEMA_FIXTURE
        .replace(
            "created_at INTEGER, fingerprint TEXT NOT NULL DEFAULT ''",
            "search_projection TEXT, created_at INTEGER, fingerprint TEXT NOT NULL DEFAULT ''",
        )
        .replace(
            "PRAGMA user_version = 7;",
            "INSERT INTO tombstones(provider, conversation_id, forgotten_at)
                VALUES ('codex', 'forgotten-codex', 25);
             INSERT INTO conversations(id, provider, source_path, source_fingerprint)
                VALUES ('unsupported-session', 'opencode', '/tmp/opencode.jsonl', 'source');
             INSERT INTO messages(
                id, conversation_id, ordinal, role, content, search_projection, fingerprint
             ) VALUES (
                'unsupported-message', 'unsupported-session', 0, 'user',
                'unsupported sentinel', NULL, 'message'
             );
             INSERT INTO message_fts(content, message_id, conversation_id)
                VALUES ('unsupported sentinel', 'unsupported-message', 'unsupported-session');
             INSERT INTO message_embeddings(message_id, generation, dimensions, norm, vector)
                VALUES ('unsupported-message', 'generation', 1, 1.0, X'7F');
             INSERT INTO source_checkpoints(provider, source_path, size_bytes, modified_ns)
                VALUES ('opencode', '/tmp/opencode.jsonl', 10, 20);
             INSERT INTO tombstones(provider, conversation_id, forgotten_at)
                VALUES ('opencode', 'forgotten-opencode', 30);
             PRAGMA user_version = 8;",
        )
}

#[veritas::claims(
    "storage/supported-schema-migrates",
    "storage/tool-search-projection-migrates"
)]
#[test]
fn supported_schema_migrates_once_and_preserves_rows() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let connection = Connection::open(&path).expect("seed database");
    connection
        .execute_batch(VERSION_SEVEN_SCHEMA_FIXTURE)
        .expect("seed older schema");
    drop(connection);

    let storage = Storage::open(&path).expect("migrate database");
    let counts = storage.counts().expect("counts");
    assert_eq!(counts.messages, 1);
    assert_eq!(counts.embeddings, 0);
    assert_eq!(counts.searchable_messages, 1);
    assert!(
        storage
            .derived_search_is_dirty()
            .expect("dirty derived state")
    );
    assert_eq!(
        storage
            .connection
            .query_row("SELECT count(*) FROM message_fts", [], |row| row
                .get::<_, i64>(0))
            .expect("cleared FTS"),
        0
    );
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT search_projection FROM messages WHERE id = 'message-1'",
                [],
                |row| row.get::<_, Option<String>>(0)
            )
            .expect("projection column"),
        None
    );
    assert_eq!(
        storage.view("message-1", 0).expect("canonical view")[0].content,
        "preserved"
    );
    assert_eq!(
        storage
            .connection
            .query_row("SELECT count(*) FROM source_checkpoints", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("cleared checkpoints"),
        0
    );
    assert_eq!(
        storage
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .expect("schema version"),
        SCHEMA_VERSION
    );
    drop(storage);
    let reopened = Storage::open(&path).expect("idempotent second open");
    assert_eq!(reopened.counts().expect("reopened counts").messages, 1);
    assert!(reopened.derived_search_is_dirty().expect("still dirty"));
}

#[veritas::claims("storage/unsupported-provider-data-is-removed")]
#[test]
fn version_eight_migration_removes_every_unsupported_provider_surface() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let connection = Connection::open(&path).expect("seed database");
    let fixture = version_eight_provider_fixture();
    connection
        .execute_batch(&fixture)
        .expect("seed version eight");
    drop(connection);

    let storage = Storage::open(&path).expect("migrate database");
    for table in ["conversations", "source_checkpoints", "tombstones"] {
        let query = format!("SELECT count(*) FROM {table} WHERE provider = 'opencode'");
        assert_eq!(
            storage
                .connection
                .query_row(&query, [], |row| row.get::<_, i64>(0))
                .expect("unsupported provider count"),
            0,
            "unsupported rows remain in {table}"
        );
    }
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT count(*) FROM source_checkpoints WHERE provider = 'codex'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("supported checkpoint count"),
        1
    );
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT count(*) FROM tombstones WHERE provider = 'codex'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("supported tombstone count"),
        1
    );
    assert_eq!(storage.counts().expect("counts").conversations, 1);
    assert_eq!(storage.counts().expect("counts").messages, 1);
    assert_eq!(storage.counts().expect("counts").embeddings, 1);
    assert!(
        storage
            .semantic_index_is_ready("old-generation")
            .expect("backfilled semantic readiness")
    );
    assert_eq!(
        storage
            .connection
            .query_row(
                "SELECT count(*) FROM message_fts WHERE content = 'unsupported sentinel'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("unsupported FTS count"),
        0
    );
    let schema: String = storage
        .connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name = 'conversations'",
            [],
            |row| row.get(0),
        )
        .expect("provider schema");
    assert!(schema.contains("'claude-code'"));
    assert!(schema.contains("'codex'"));
    assert!(!schema.contains("'opencode'"));
    for provider in ["opencode", "github-copilot", "hermes", "pi"] {
        let error = storage
            .connection
            .execute(
                "INSERT INTO conversations(id, provider, source_path)
                 VALUES (?1, ?2, ?3)",
                params![
                    format!("rejected-{provider}"),
                    provider,
                    format!("/tmp/rejected-{provider}.jsonl")
                ],
            )
            .expect_err("unsupported provider rejected");
        assert!(matches!(error, rusqlite::Error::SqliteFailure(_, _)));
    }
    assert_eq!(
        storage
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .expect("schema version"),
        SCHEMA_VERSION
    );
}

#[veritas::claims("storage/newer-schema-is-rejected")]
#[test]
fn newer_schema_is_rejected_without_rewriting_version() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let connection = Connection::open(&path).expect("seed database");
    connection
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .expect("newer version");
    drop(connection);

    let error = Storage::open(&path).err().expect("newer schema rejected");
    assert_eq!(error.error.kind, "schema-incompatible");
    let connection = Connection::open(&path).expect("reopen seed");
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .expect("schema version"),
        SCHEMA_VERSION + 1
    );
}

#[veritas::claims("indexing/concurrent-writer-is-rejected")]
#[test]
fn concurrent_writer_is_rejected_and_first_writer_can_commit() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut first = Storage::open_writer(&path).expect("first writer");
    let error = Storage::open_writer(&path)
        .err()
        .expect("second writer rejected");
    assert_eq!(error.error.kind, "index-busy");
    first.commit_writer().expect("first writer commits");
    Storage::open_writer(&path).expect("writer available after commit");
}

#[veritas::claims(
    "indexing/unchanged-source-is-skipped",
    "indexing/only-changed-messages-refresh"
)]
#[test]
fn conversation_reconciliation_writes_only_changed_messages() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut storage = Storage::open_writer(&path).expect("writer");
    let first = storage
        .replace_conversation(&conversation("first"))
        .expect("initial insert");
    assert_eq!(first.changed_message_ids, ["message-1"]);
    storage
        .replace_embeddings(
            "generation-a",
            &[EmbeddingWrite {
                message_id: "message-1",
                vector: &[127, 0],
                norm: 127.0,
            }],
        )
        .expect("seed embedding");
    let unchanged = storage
        .replace_conversation(&conversation("first"))
        .expect("unchanged refresh");
    assert!(unchanged.unchanged);
    assert_eq!(unchanged.changed_message_ids, Vec::<String>::new());
    assert!(
        storage
            .messages_needing_embeddings()
            .expect("embedding selection")
            .is_empty()
    );
    let changed = storage
        .replace_conversation(&conversation("second"))
        .expect("changed refresh");
    assert_eq!(changed.changed_message_ids, ["message-1"]);
    assert_eq!(
        storage
            .messages_needing_embeddings()
            .expect("embedding selection")
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        ["message-1"]
    );
}

#[veritas::claims("indexing/only-changed-messages-refresh")]
#[test]
fn conversation_reconciliation_replaces_a_changed_message_id_at_the_same_ordinal() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut storage = Storage::open_writer(&path).expect("writer");
    storage
        .replace_conversation(&conversation("first"))
        .expect("initial insert");

    let mut replacement = conversation("second");
    replacement.messages[0].id = "message-2".to_owned();
    let change = storage
        .replace_conversation(&replacement)
        .expect("replace message identity");

    assert_eq!(change.changed_message_ids, ["message-2"]);
    assert_eq!(change.removed_messages, 1);
    assert_eq!(storage.counts().expect("counts").messages, 1);
    storage.commit_writer().expect("commit replacement");
    assert!(storage.search("first", 10, None, None).unwrap().is_empty());
    assert_eq!(
        storage.search("second", 10, None, None).unwrap()[0].id,
        "message-2"
    );
}

#[test]
fn purging_a_conversation_removes_its_staged_fts_rows() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&conversation("disappearing sentinel"))
        .expect("seed conversation");
    writer.commit_writer().expect("commit seed");
    drop(writer);

    let mut writer = Storage::open_writer(&path).expect("purge writer");
    assert_eq!(
        writer
            .purge_missing_sources(
                "codex",
                &BTreeSet::new(),
                &[std::path::PathBuf::from("/tmp")],
                None,
            )
            .expect("purge missing source"),
        1
    );
    writer.commit_writer().expect("commit purge");
    assert!(
        writer
            .search("disappearing", 10, None, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(writer.counts().unwrap().messages, 0);
}

#[veritas::claims("semantic/stale-embedding-generation-invalidated")]
#[test]
fn stale_embedding_generation_is_excluded_and_replaced() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&conversation("generation proof"))
        .expect("insert conversation");
    writer
        .replace_embeddings(
            "old-generation",
            &[EmbeddingWrite {
                message_id: "message-1",
                vector: &[127, 0],
                norm: 127.0,
            }],
        )
        .expect("old embedding");
    writer.commit_writer().expect("commit old generation");
    drop(writer);

    let storage = Storage::open(&path).expect("reader");
    assert_eq!(storage.embedding_count("new-generation").expect("count"), 0);
    assert_eq!(
        storage
            .semantic_chunks("new-generation", None, None)
            .expect("semantic documents")
            .chunks
            .len(),
        0
    );
    drop(storage);

    let mut writer = Storage::open_writer(&path).expect("replacement writer");
    assert_eq!(
        writer
            .invalidate_embedding_generation("new-generation")
            .expect("invalidate"),
        1
    );
    assert_eq!(
        writer
            .messages_needing_embeddings()
            .expect("re-embedding selection")
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        ["message-1"]
    );
    writer
        .replace_embeddings(
            "new-generation",
            &[EmbeddingWrite {
                message_id: "message-1",
                vector: &[0, 127],
                norm: 127.0,
            }],
        )
        .expect("new embedding");
    writer.commit_writer().expect("commit new generation");
    assert_eq!(writer.embedding_count("new-generation").expect("count"), 1);
    let vectors = writer
        .semantic_chunks("new-generation", None, None)
        .expect("stored quantized vectors");
    assert_eq!(vectors.chunks.len(), 1);
    assert_eq!(vectors.chunks[0].message_rowids, [1]);
    assert_eq!(vectors.chunks[0].values, [0, 127]);
    assert_eq!(vectors.chunks[0].norms, [127.0]);
    assert_eq!(vectors.dimensions, 2);
    assert_eq!(
        writer.search_hits(&[1]).expect("hydrate semantic hit")[0].content,
        "generation proof"
    );
}

#[veritas::claims("storage/forget-persists-through-indexing")]
#[test]
fn tombstone_prevents_reinsertion() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&conversation("remember me"))
        .expect("insert");
    writer.commit_writer().expect("commit");
    drop(writer);

    let mut storage = Storage::open(&path).expect("open database");
    assert!(storage.forget("session-1").expect("forget"));
    drop(storage);

    let mut writer = Storage::open_writer(&path).expect("second writer");
    let change = writer
        .replace_conversation(&conversation("remember me"))
        .expect("tombstone check");
    assert!(change.tombstoned);
    writer.commit_writer().expect("commit");
    assert_eq!(writer.counts().expect("counts").conversations, 0);
}

#[test]
fn changed_searchable_messages_are_transactionally_queued_for_embedding() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&conversation("searchable message"))
        .expect("insert conversation");

    let pending: i64 = writer
        .connection
        .query_row("SELECT count(*) FROM pending_embeddings", [], |row| {
            row.get(0)
        })
        .expect("pending embedding count");
    assert_eq!(pending, 1);

    writer
        .replace_embeddings(
            "generation",
            &[EmbeddingWrite {
                message_id: "message-1",
                vector: &[127, 0],
                norm: 127.0,
            }],
        )
        .expect("store embedding");
    let pending: i64 = writer
        .connection
        .query_row("SELECT count(*) FROM pending_embeddings", [], |row| {
            row.get(0)
        })
        .expect("drained embedding count");
    assert_eq!(pending, 0);
}

#[test]
fn ordinary_writer_commits_leave_wal_checkpointing_to_sqlite() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("cass.sqlite3");
    let wal_path = std::path::PathBuf::from(format!("{}-wal", path.display()));
    let mut writer = Storage::open_writer(&path).expect("writer");
    writer
        .replace_conversation(&conversation("small incremental write"))
        .expect("insert conversation");
    writer.commit_writer().expect("commit writer");

    assert!(
        std::fs::metadata(wal_path).is_ok_and(|metadata| metadata.len() > 0),
        "a small commit must not force a blocking WAL truncation"
    );
}
