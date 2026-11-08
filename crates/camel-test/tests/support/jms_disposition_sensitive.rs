use super::*;

const ID: &str = "ID:synthetic";

fn expected_message() -> String {
    format!("{TERMINAL_LOG_PREFIX} {DEFAULT_MAX_DELIVERY_ATTEMPTS} delivery attempts: {ID}")
}

fn json_record(level: &str, logger: &str, message: &str) -> String {
    serde_json::json!({
        "level": level,
        "loggerName": logger,
        "message": message,
    })
    .to_string()
}

fn synthetic(commit_frames: usize, matching: usize) -> ThresholdObservation {
    ThresholdObservation {
        known_id: ID.to_string(),
        commit_frames,
        terminal_check: TerminalLogCheck {
            matching,
            mislabeled: 0,
        },
        ..Default::default()
    }
}

/// Sibling builder for Artemis-shaped observations: a missing bridge
/// `COMMIT` plus explicit broker-side queue counts.
fn synthetic_artemis(source: QueueStat, dlq: QueueStat, expiry: QueueStat) -> ThresholdObservation {
    ThresholdObservation {
        commit_frames: 0,
        artemis_destination: Some(ArtemisDestination {
            source,
            dlq,
            expiry,
        }),
        ..Default::default()
    }
}

fn stat(message_count: u64, messages_added: u64) -> QueueStat {
    QueueStat {
        message_count,
        messages_added,
    }
}

#[test]
fn exact_error_record_is_accepted() {
    let contents = json_record(ERROR_LEVEL, TERMINAL_LOGGER, &expected_message());
    let check = check_terminal_records(&contents, &expected_message());
    assert_eq!(
        check,
        TerminalLogCheck {
            matching: 1,
            mislabeled: 0
        }
    );
}

#[test]
fn warn_level_terminal_record_is_rejected() {
    let contents = json_record("WARN", TERMINAL_LOGGER, &expected_message());
    let check = check_terminal_records(&contents, &expected_message());
    assert_eq!(
        check,
        TerminalLogCheck {
            matching: 0,
            mislabeled: 1
        },
        "a WARN record carrying the terminal text must not pass"
    );
}

#[test]
fn other_logger_terminal_record_is_rejected() {
    let contents = json_record(ERROR_LEVEL, "org.example.OtherLogger", &expected_message());
    let check = check_terminal_records(&contents, &expected_message());
    assert_eq!(
        check,
        TerminalLogCheck {
            matching: 0,
            mislabeled: 1
        },
        "an ERROR from another logger carrying the terminal text must not pass"
    );
}

#[test]
fn wrong_count_terminal_record_is_rejected() {
    let wrong = format!("{TERMINAL_LOG_PREFIX} 5 delivery attempts: {ID}");
    let contents = json_record(ERROR_LEVEL, TERMINAL_LOGGER, &wrong);
    let check = check_terminal_records(&contents, &expected_message());
    assert_eq!(
        check,
        TerminalLogCheck {
            matching: 0,
            mislabeled: 1
        }
    );
}

#[test]
fn wrong_id_terminal_record_is_rejected() {
    let wrong = format!(
        "{TERMINAL_LOG_PREFIX} {DEFAULT_MAX_DELIVERY_ATTEMPTS} delivery attempts: ID:other"
    );
    let contents = json_record(ERROR_LEVEL, TERMINAL_LOGGER, &wrong);
    let check = check_terminal_records(&contents, &expected_message());
    assert_eq!(
        check,
        TerminalLogCheck {
            matching: 0,
            mislabeled: 1
        }
    );
}

#[test]
fn non_json_terminal_text_is_not_a_record() {
    let check = check_terminal_records(&expected_message(), &expected_message());
    assert_eq!(
        check,
        TerminalLogCheck {
            matching: 0,
            mislabeled: 0
        }
    );
}

#[test]
fn bridge_cap_first_when_commit_and_record_match() {
    assert_eq!(
        classify_precedence(&synthetic(1, 1)),
        Precedence::BridgeCapFirst
    );
}

#[test]
fn missing_commit_without_evidence_is_unknown_not_native_first() {
    // No COMMIT and no broker query (Artemis): must be Unknown, never
    // inferred as native-first.
    assert!(matches!(
        classify_precedence(&synthetic(0, 0)),
        Precedence::Unknown { .. }
    ));
}

#[test]
fn missing_commit_with_classic_dlq_evidence_is_native_first() {
    let obs = ThresholdObservation {
        commit_frames: 0,
        classic_destination: Some(ClassicDestination {
            queue_size: 0,
            dlq_enqueue: 1,
        }),
        ..Default::default()
    };
    assert!(matches!(
        classify_precedence(&obs),
        Precedence::NativeLimitFirst { .. }
    ));
}

#[test]
fn missing_commit_with_classic_inconclusive_evidence_is_unknown() {
    let obs = ThresholdObservation {
        commit_frames: 0,
        classic_destination: Some(ClassicDestination {
            queue_size: 0,
            dlq_enqueue: 0,
        }),
        ..Default::default()
    };
    assert!(
        matches!(classify_precedence(&obs), Precedence::Unknown { .. }),
        "an empty source queue with an empty DLQ does not confirm native-first"
    );
}

#[test]
fn missing_commit_with_artemis_dlq_evidence_is_native_first() {
    let obs = synthetic_artemis(stat(0, 1), stat(0, 1), stat(0, 0));
    assert!(matches!(
        classify_precedence(&obs),
        Precedence::NativeLimitFirst { .. }
    ));
}

#[test]
fn missing_commit_with_artemis_expiry_evidence_is_native_first() {
    let obs = synthetic_artemis(stat(0, 1), stat(0, 0), stat(0, 1));
    assert!(matches!(
        classify_precedence(&obs),
        Precedence::NativeLimitFirst { .. }
    ));
}

#[test]
fn missing_commit_with_artemis_no_evidence_is_unknown() {
    let obs = synthetic_artemis(stat(0, 1), stat(0, 0), stat(0, 0));
    assert!(
        matches!(classify_precedence(&obs), Precedence::Unknown { .. }),
        "an empty source queue with an empty DLQ and ExpiryQueue does not confirm native-first"
    );
}

#[test]
fn artemis_destination_describe_names_each_outcome() {
    let acknowledged = ArtemisDestination {
        source: stat(0, 1),
        dlq: stat(0, 0),
        expiry: stat(0, 0),
    };
    assert!(
        acknowledged.describe().contains(
            "acknowledged and removed by the bridge terminal policy; no DLQ or expiry delivery"
        ),
        "unexpected description: {}",
        acknowledged.describe()
    );

    let dead_lettered = ArtemisDestination {
        source: stat(0, 1),
        dlq: stat(0, 1),
        expiry: stat(0, 0),
    };
    assert!(
        dead_lettered
            .describe()
            .contains("broker-native dead-letter path (DLQ)"),
        "unexpected description: {}",
        dead_lettered.describe()
    );

    let expired = ArtemisDestination {
        source: stat(0, 1),
        dlq: stat(0, 0),
        expiry: stat(0, 1),
    };
    assert!(
        expired
            .describe()
            .contains("broker-native expiry path (ExpiryQueue)"),
        "unexpected description: {}",
        expired.describe()
    );

    let inconclusive = ArtemisDestination {
        source: stat(1, 1),
        dlq: stat(0, 0),
        expiry: stat(0, 0),
    };
    let description = inconclusive.describe();
    assert!(
        description.starts_with(
            "source MESSAGE_COUNT=1 DLQ MESSAGES_ADDED=0 ExpiryQueue MESSAGES_ADDED=0 ("
        ),
        "unexpected description: {description}"
    );
    assert!(
        description.contains("inconclusive: message still on the source queue"),
        "unexpected description: {description}"
    );
}

#[test]
fn artemis_acknowledged_and_removed_requires_all_three_queues() {
    let dead_lettered = ArtemisDestination {
        source: stat(0, 1),
        dlq: stat(0, 1),
        expiry: stat(0, 0),
    };
    assert!(!dead_lettered.acknowledged_and_removed());

    let expired = ArtemisDestination {
        source: stat(0, 1),
        dlq: stat(0, 0),
        expiry: stat(0, 1),
    };
    assert!(!expired.acknowledged_and_removed());
}

/// The load-bearing property: a missing Artemis broker-side destination is
/// not an unobserved pass — the reporter fails the case. The counters are
/// contract-valid because `assert_threshold_contract` runs before the
/// destination check, so zeroed counters would panic with the wrong message.
#[cfg(unix)]
#[test]
#[should_panic(expected = "returned no evidence")]
fn artemis_missing_destination_fails_the_case() {
    let obs = ThresholdObservation {
        known_id: ID.to_string(),
        reserve_frames: DEFAULT_MAX_DELIVERY_ATTEMPTS,
        rollback_frames: DEFAULT_MAX_DELIVERY_ATTEMPTS,
        commit_frames: 1,
        accepted_events: DEFAULT_MAX_DELIVERY_ATTEMPTS,
        terminal_check: TerminalLogCheck {
            matching: 1,
            mislabeled: 0,
        },
        payload_in_log: false,
        artemis_destination: None,
        ..Default::default()
    };
    record_and_report_disposition(&obs, "artemis");
}
