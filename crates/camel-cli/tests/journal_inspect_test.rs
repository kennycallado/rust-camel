//! CLI-level integration tests for `camel journal inspect`.
//!
//! These tests call `RedbRuntimeEventJournal::inspect` directly to verify the
//! inspection logic that `run_inspect` delegates to. The formatting output of
//! `run_inspect` itself (table/JSON rendering, `event_parts` mapping) is not
//! exercised here because it would require capturing stdout — a refactor
//! tracked as a future improvement.

use std::sync::Arc;

use camel_core::{
    JournalDurability, JournalInspectFilter, RedbJournalOptions, RedbRuntimeEventJournal,
    RuntimeEvent, RuntimeEventJournalPort,
};
use tempfile::tempdir;

async fn seed_journal(path: std::path::PathBuf) -> Arc<RedbRuntimeEventJournal> {
    let journal = Arc::new(
        RedbRuntimeEventJournal::new(
            path,
            RedbJournalOptions {
                durability: JournalDurability::Eventual,
                compaction_threshold_events: 10_000,
            },
        )
        .await
        .unwrap(),
    );
    journal
        .append_batch(&[
            RuntimeEvent::RouteRegistered {
                route_id: "r1".to_string(),
            },
            RuntimeEvent::RouteStarted {
                route_id: "r1".to_string(),
            },
            RuntimeEvent::RouteRegistered {
                route_id: "r2".to_string(),
            },
        ])
        .await
        .unwrap();
    journal
}

#[tokio::test]
async fn inspect_shows_all_events_in_descending_order() {
    let dir = tempdir().unwrap();
    let _ = seed_journal(dir.path().join("all.db")).await;

    let entries = RedbRuntimeEventJournal::inspect(
        dir.path().join("all.db"),
        JournalInspectFilter {
            route_id: None,
            limit: 100,
        },
    )
    .await
    .unwrap();

    assert_eq!(entries.len(), 3);
    // Descending order: last appended (r2 Registered) first.
    assert!(matches!(
        &entries[0].event,
        RuntimeEvent::RouteRegistered { route_id } if route_id == "r2"
    ));
    // Sequence numbers must be descending.
    assert!(entries[0].seq > entries[1].seq);
    assert!(entries[1].seq > entries[2].seq);
}

#[tokio::test]
async fn inspect_filters_by_route_id() {
    let dir = tempdir().unwrap();
    let _ = seed_journal(dir.path().join("filter.db")).await;

    let entries = RedbRuntimeEventJournal::inspect(
        dir.path().join("filter.db"),
        JournalInspectFilter {
            route_id: Some("r1".to_string()),
            limit: 100,
        },
    )
    .await
    .unwrap();

    assert_eq!(entries.len(), 2, "only r1 events");
    // All returned entries must be for r1
    for entry in &entries {
        assert!(
            matches!(
                &entry.event,
                RuntimeEvent::RouteRegistered { route_id } | RuntimeEvent::RouteStarted { route_id }
                if route_id == "r1"
            ),
            "unexpected event: {:?}",
            entry.event
        );
    }
}

#[tokio::test]
async fn inspect_respects_limit() {
    let dir = tempdir().unwrap();
    let _ = seed_journal(dir.path().join("limit.db")).await;

    let entries = RedbRuntimeEventJournal::inspect(
        dir.path().join("limit.db"),
        JournalInspectFilter {
            route_id: None,
            limit: 2,
        },
    )
    .await
    .unwrap();

    assert_eq!(entries.len(), 2, "limit=2 must return exactly 2 events");
}

#[tokio::test]
async fn inspect_json_format_is_valid() {
    let dir = tempdir().unwrap();
    let _ = seed_journal(dir.path().join("json.db")).await;

    let entries = RedbRuntimeEventJournal::inspect(
        dir.path().join("json.db"),
        JournalInspectFilter {
            route_id: None,
            limit: 100,
        },
    )
    .await
    .unwrap();

    // Must serialize to valid JSON array.
    let json = serde_json::to_string(&entries).unwrap();
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.len(), 3);
    // Each entry must have seq, timestamp_ms, and event fields.
    for item in &parsed {
        assert!(item.get("seq").is_some());
        assert!(item.get("timestamp_ms").is_some());
        assert!(item.get("event").is_some());
    }
}

#[tokio::test]
async fn inspect_nonexistent_file_returns_clear_error() {
    let result = RedbRuntimeEventJournal::inspect(
        "/nonexistent/path/journal.db",
        JournalInspectFilter {
            route_id: None,
            limit: 100,
        },
    )
    .await;

    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("journal file not found"),
        "error must mention file not found, got: {msg}"
    );
}

#[tokio::test]
async fn inspect_filter_with_no_matches_returns_empty_vec() {
    let dir = tempdir().unwrap();
    let _ = seed_journal(dir.path().join("empty-filter.db")).await;

    let entries = RedbRuntimeEventJournal::inspect(
        dir.path().join("empty-filter.db"),
        JournalInspectFilter {
            route_id: Some("nonexistent-route".to_string()),
            limit: 100,
        },
    )
    .await
    .unwrap();

    assert!(
        entries.is_empty(),
        "filter for nonexistent route must return empty vec, not an error"
    );
}

#[tokio::test]
async fn inspect_limit_zero_returns_empty() {
    let dir = tempdir().unwrap();
    let _ = seed_journal(dir.path().join("limit-zero.db")).await;

    let entries = RedbRuntimeEventJournal::inspect(
        dir.path().join("limit-zero.db"),
        JournalInspectFilter {
            route_id: None,
            limit: 0,
        },
    )
    .await
    .unwrap();

    assert!(
        entries.is_empty(),
        "limit=0 must return empty vec, got {} entries",
        entries.len()
    );
}
