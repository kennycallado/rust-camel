//! Task 6.5 drill: native live-broker sentinel proof for `jms.connectionIDPrefix`.
//!
//! Included as a focused submodule of `jms_test`, so the prescribed
//! `cargo test -p camel-test --features integration-tests --test jms_test <name>`
//! command selects each case by name.
//!
//! The former `ClientId` observer is NOT a connection-prefix oracle.
//! `ConnectionView.getClientId()` delegates to
//! `TransportConnection.getConnectionId`, which prefers `ConnectionInfo.clientId`
//! when set. This representation holds for the pinned brokers 5.18.3 and
//! 5.19.10, so a client-ID match proves nothing about `connectionIDPrefix`.
//!
//! The real proof instead recovers each producer's connection identifier from
//! the broker-browsed `JMSMessageID` of its stored message (the expert-confirmed
//! right-three-suffix parse of ActiveMQ's `[ID:]conn:session:producer:seq`
//! identity) and then checks the `jms.connectionIDPrefix` sentinel plus two
//! distinct per-process UUID tokens. Identity is never taken from a send
//! response, a factory getter, the input URL, or an MBean object-name key, and
//! neither the bridge nor this test ever sets `JMSMessageID`.

use camel_component_jms::proto::bridge_service_client::BridgeServiceClient;

use super::support::activemq::ActiveMqBroker;
use super::support::jms_bridge_procs::{require_jms_bridge_binary, spawn_jms_bridge};
use super::support::{init_tracing, install_crypto_provider};
use super::{await_broker_connected, send_one, wait_for_enqueue_count};

/// The `jms.connectionIDPrefix` sentinel base the two native processes carry in
/// `BRIDGE_BROKER_URL`. A safe, non-secret known identifier only.
const SENTINEL: &str = "native-uri-sentinel-prefix";

/// Dedicated consumer-free queue: no consumer may remove the evidence before
/// the browse.
const NATIVE_URI_QUEUE: &str = "queue.test-native-uri-option";

/// Extracts the per-process token segment (`jms-bridge-<uuid>`) from a
/// broker-observed connection ID. Returns `None` unless the ID carries a
/// well-formed `jms-bridge-` marker followed by a canonical 8-4-4-4-12 hex
/// UUID; the trailing ActiveMQ transport counter is ignored. The parse is
/// strict so a frozen-token regression and a malformed ID are reported as
/// distinct failures rather than a silent substring match.
fn process_token_segment(id: &str) -> Option<String> {
    const MARKER: &str = "jms-bridge-";
    let start = id.find(MARKER)?;
    let rest = id.get(start + MARKER.len()..)?;
    let uuid = rest.get(..36)?;
    let bytes = uuid.as_bytes();
    if bytes.len() != 36 {
        return None;
    }
    let hyphen_positions = [8usize, 13, 18, 23];
    if !hyphen_positions.iter().all(|&i| bytes[i] == b'-') {
        return None;
    }
    let hex_groups = [(0usize, 8usize), (9, 13), (14, 18), (19, 23), (24, 36)];
    if !hex_groups
        .iter()
        .all(|&(lo, hi)| bytes[lo..hi].iter().all(u8::is_ascii_hexdigit))
    {
        return None;
    }
    Some(format!("{MARKER}{uuid}"))
}

/// True when `component` is a non-empty run of ASCII digits — the numeric shape
/// of a message-id suffix (`sessionId`, `producerId`, `sequenceNumber`).
fn is_numeric_component(component: &str) -> bool {
    !component.is_empty() && component.bytes().all(|b| b.is_ascii_digit())
}

/// Recovers the producer connection identifier from a broker-returned ActiveMQ
/// `JMSMessageID`.
///
/// ActiveMQ builds each message identity as
/// `[ID:]connectionId:sessionId:producerId:sequenceNumber`. The connection
/// identifier itself carries colons (and a trailing `:n` generator counter), so
/// the three numeric suffixes are peeled from the RIGHT; a left-to-right split
/// would tear the connection identifier apart. The standard leading `ID:` marker
/// is stripped only when present. Errors are fixed and value-free (they never
/// echo the input), and no identity is derived from a send response, a factory
/// getter, the input URL, or an MBean object-name key.
fn connection_id_from_broker_message_id(message_id: &str) -> Result<String, String> {
    const MISSING: &str = "malformed broker message identity: missing suffix components";
    const NON_NUMERIC: &str = "malformed broker message identity: non-numeric suffix component";
    const EMPTY: &str = "malformed broker message identity: empty connection identifier";

    // Right-strip the three numeric suffixes (sequence, producer, session).
    let mut parts = message_id.rsplitn(4, ':');
    let sequence = parts.next().unwrap_or("");
    let producer = parts.next().unwrap_or("");
    let session = parts.next().unwrap_or("");
    let connection_part = parts.next().ok_or_else(|| MISSING.to_string())?;

    if !is_numeric_component(sequence)
        || !is_numeric_component(producer)
        || !is_numeric_component(session)
    {
        return Err(NON_NUMERIC.to_string());
    }

    // Strip the standard leading `ID:` marker only when present.
    let connection = connection_part
        .strip_prefix("ID:")
        .unwrap_or(connection_part);
    if connection.is_empty() {
        return Err(EMPTY.to_string());
    }
    Ok(connection.to_string())
}

/// Requires two distinct per-process UUID token segments across `connection_ids`
/// (the two-process live-broker proof supplies exactly two). Every identifier
/// must carry a valid `jms-bridge-<uuid>` token; a shared token reveals an
/// image-build-frozen `IdGenerator` even when the whole identifiers differ.
/// Errors are fixed and value-free.
fn validate_distinct_process_tokens(connection_ids: &[String]) -> Result<(), String> {
    const MISSING_TOKEN: &str = "connection identifier lacks a valid process token";
    const FROZEN: &str = "process tokens are not distinct across connections";

    let mut tokens: Vec<String> = Vec::with_capacity(connection_ids.len());
    for id in connection_ids {
        match process_token_segment(id) {
            Some(token) => tokens.push(token),
            None => return Err(MISSING_TOKEN.to_string()),
        }
    }
    let distinct = tokens.iter().collect::<std::collections::BTreeSet<_>>();
    if distinct.len() < 2 || distinct.len() != tokens.len() {
        return Err(FROZEN.to_string());
    }
    Ok(())
}

/// Native live-broker proof for rc-odr5c: with the class-level
/// `ActiveMQConnectionFactory` reflection registration, the native bridge must
/// apply a Classic `jms.*` broker-URL option instead of leaving it in the URL
/// for the transport factory to reject ("Invalid connect parameters"). Two
/// processes from the SAME fresh 0.8.0 native binary are pointed at one broker
/// with `?jms.connectionIDPrefix=<sentinel>` and each must present a distinct
/// broker-observed connection identity that carries the sentinel.
///
/// Identity comes only from broker-browsed messages: the connection identifier
/// is recovered from the actual `JMSMessageID` of each stored message via the
/// Queue MBean `browse()`. No send response, factory getter, input URL, or
/// MBean object-name key is used as identity, and no message ID is set.
///
/// The per-process token segment must differ between the two processes: a
/// GraalVM-folded token would make both IDs share one segment even though the
/// whole IDs differ (rc-trxge frozen-IdGenerator class).
#[tokio::test(flavor = "multi_thread")]
async fn jms_bridge_native_jms_uri_option_applies_activemq() {
    init_tracing();
    install_crypto_provider();

    let broker = ActiveMqBroker::start().await;
    let binary = require_jms_bridge_binary();
    // Safe, credential-free identifier: the sentinel is only a prefix base. The
    // query stays `jms.connectionIDPrefix` — never a client-prefix substitute.
    let broker_url = format!("{}?jms.connectionIDPrefix={SENTINEL}", broker.broker_url);

    // Two producer-only native processes from the SAME fresh native binary,
    // each with its own private journal root (`spawn_jms_bridge`), both carrying
    // the sentinel query option through BRIDGE_BROKER_URL.
    let (proc1, channel1) = spawn_jms_bridge(&binary, &broker_url).await;
    let (proc2, channel2) = spawn_jms_bridge(&binary, &broker_url).await;
    let mut client1 = BridgeServiceClient::new(channel1);
    let mut client2 = BridgeServiceClient::new(channel2);

    await_broker_connected(&mut client1).await;
    await_broker_connected(&mut client2).await;

    // Send ONCE per process into the dedicated consumer-free queue. The send
    // response is deliberately discarded: identity evidence comes only from the
    // broker-browsed messages below, never from the send acknowledgement.
    send_one(&mut client1, NATIVE_URI_QUEUE, "native-uri-1").await;
    send_one(&mut client2, NATIVE_URI_QUEUE, "native-uri-2").await;

    // Broker-side: both messages landed on the queue (Classic EnqueueCount).
    wait_for_enqueue_count(&broker, NATIVE_URI_QUEUE, 2).await;

    // Browse both stored messages through the Queue MBean (browse does not
    // consume). No consumer may remove the evidence before this point.
    let browsed = broker
        .browse_messages(NATIVE_URI_QUEUE)
        .await
        .expect("browse the dedicated queue");
    assert_eq!(
        browsed.len(),
        2,
        "expected exactly two broker-browsed messages on the dedicated queue"
    );

    // The ONLY identity source: the genuine broker-browsed JMSMessageID field.
    let message_ids: Vec<&str> = browsed
        .iter()
        .map(|m| {
            m.jms_message_id()
                .expect("broker-browsed message must carry a JMSMessageID")
        })
        .collect();

    // Recover each producer connection identifier from the broker message ID.
    let connection_ids: Vec<String> = message_ids
        .iter()
        .map(|id| {
            connection_id_from_broker_message_id(id)
                .expect("broker message ID must yield a producer connection identifier")
        })
        .collect();

    // Surface the actual broker-derived proof values for the report (safe
    // identifiers only; never a message body or header value).
    println!("PROOF 6.5 broker_message_ids={message_ids:?} connection_ids={connection_ids:?}");

    // Each recovered identifier must carry the jms.connectionIDPrefix sentinel.
    for conn in &connection_ids {
        assert!(
            conn.contains(SENTINEL),
            "connection identifier must contain the sentinel {SENTINEL:?}; a \
             missing result means the native bridge did not apply \
             jms.connectionIDPrefix to the live broker connection"
        );
    }

    // The two identifiers must be two distinct live connections.
    assert_ne!(
        connection_ids[0], connection_ids[1],
        "the two producer connection identifiers must differ"
    );

    // GraalVM image-heap safeguard: the two process UUID tokens must differ, or
    // one folded token was shared by both processes.
    let tokens: Vec<String> = connection_ids
        .iter()
        .map(|id| {
            process_token_segment(id)
                .expect("connection identifier must carry a jms-bridge-<uuid> token")
        })
        .collect();
    println!("PROOF 6.5 process_tokens={tokens:?}");
    validate_distinct_process_tokens(&connection_ids)
        .expect("two distinct process UUID tokens required");

    drop(client1);
    drop(client2);
    drop(proc1);
    drop(proc2);
}

/// Synthetic identity-parse unit: only valid producer-derived ActiveMQ message
/// identities are accepted; missing components, an empty connection identifier,
/// and non-numeric suffixes are rejected with value-free errors (the error must
/// never echo the input value).
#[test]
fn broker_message_identity_rejects_invalid_suffixes() {
    // Valid shape: `[ID:]connectionId:sessionId:producerId:sequenceNumber`.
    let valid: [(&str, &str); 2] = [
        (
            "ID:host-jms-bridge-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee:1:10:20:30",
            "host-jms-bridge-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee:1",
        ),
        (
            "host-jms-bridge-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee:2:10:20:30",
            "host-jms-bridge-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee:2",
        ),
    ];
    for (id, expected) in valid {
        let conn = connection_id_from_broker_message_id(id).expect("valid identity accepted");
        assert_eq!(
            conn, expected,
            "a valid identity must recover its producer connection identifier"
        );
    }

    // Malformed identities: missing components, empty connection identifier,
    // non-numeric suffixes. Every error must omit the input value.
    let malformed: [&str; 6] = [
        "conn-jms-bridge-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee:10:20", // missing seq
        "conn:10",                                                    // missing components
        "ID::10:20:30",                                               // empty connection identifier
        ":10:20:30",                                                  // empty connection identifier
        "conn-jms-bridge-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee:10:20:x", // non-numeric seq
        "conn:10:20:30y",                                             // non-numeric seq
    ];
    for id in malformed {
        let err =
            connection_id_from_broker_message_id(id).expect_err("malformed identity rejected");
        assert!(!err.contains(id), "error must omit the input value");
    }
}

/// Synthetic frozen-token unit: two whole message IDs that differ but share one
/// process UUID token must be rejected (whole-ID inequality alone must not
/// pass); two distinct valid process UUIDs pass; missing or invalid UUIDs fail.
#[test]
fn broker_identity_tokens_detect_frozen_process_token() {
    const TOKEN_A: &str = "jms-bridge-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    const TOKEN_B: &str = "jms-bridge-11111111-2222-3333-4444-555555555555";

    // Frozen token: two whole message IDs differ (different connection counters
    // and sequence suffixes) but share ONE process UUID. Whole-ID inequality
    // alone must NOT pass.
    let frozen_ids = [
        format!("ID:alpha-{TOKEN_A}:1:10:20:30"),
        format!("ID:beta-{TOKEN_A}:2:10:20:40"),
    ];
    let frozen_conn: Vec<String> = frozen_ids
        .iter()
        .map(|id| connection_id_from_broker_message_id(id).expect("valid identity"))
        .collect();
    // The recovered identifiers differ as wholes...
    assert_ne!(frozen_conn[0], frozen_conn[1], "whole identifiers differ");
    // ...yet the shared process UUID token is detected as frozen.
    validate_distinct_process_tokens(&frozen_conn)
        .expect_err("shared process UUID token must be rejected as frozen");

    // Distinct tokens: two different valid process UUIDs pass.
    let distinct_ids = [
        format!("ID:alpha-{TOKEN_A}:1:10:20:30"),
        format!("ID:beta-{TOKEN_B}:2:10:20:40"),
    ];
    let distinct_conn: Vec<String> = distinct_ids
        .iter()
        .map(|id| connection_id_from_broker_message_id(id).expect("valid identity"))
        .collect();
    validate_distinct_process_tokens(&distinct_conn)
        .expect("two distinct process UUID tokens accepted");

    // Missing or invalid UUID tokens must fail.
    let missing_token = [
        String::from("ID:alpha-no-token-here:1:10:20:30"),
        format!("ID:beta-{TOKEN_A}:2:10:20:40"),
    ];
    validate_distinct_process_tokens(&missing_token).expect_err("missing process token must fail");

    let invalid_uuid = [
        String::from("ID:alpha-jms-bridge-zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz:1:10:20:30"),
        format!("ID:beta-{TOKEN_A}:2:10:20:40"),
    ];
    validate_distinct_process_tokens(&invalid_uuid)
        .expect_err("invalid process UUID token must fail");
}
