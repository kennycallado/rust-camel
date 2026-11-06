//! CI-owned native durable-subscriber reconnect entry point (task 1.7, change
//! `jms-bridge-tls-durable-topics`).
//!
//! **Owner: CI.** This module is `#[ignore]`d by default (ADR-0054 `slow test:`
//! classification — a containerized broker plus a built bridge binary are slow
//! prerequisites the per-PR gate cannot cheaply satisfy) and CI runs it
//! explicitly with the `--ignored` flag:
//!
//! ```text
//! cargo test -p camel-test --features integration-tests --test jms_durable_test -- --ignored
//! ```
//!
//! Native row matrix:
//!
//! | Row | Expectation |
//! | --- | --- |
//! | `durable_survives_reconnect` | warmup attach + COMMIT, bridge restart (no consumer attached), offline publish, reattach with identical `client_id`/`subscription_name` and a fresh `subscription_id` → the offline payload is delivered |
//!
//! The durable capability-gate rows (`durable-subscribers-v1` fail-closed,
//! runtime capability loss, late attach, per-instance evidence) are covered by
//! the `component.rs`/`bridge_client_test.rs` mock tests
//! (`spawn_durable_gate_bridge`); this file stays the single native entry point
//! that names its matrix and does not duplicate them.
//!
//! Prerequisite: the bridge binary must be built from a bridge that advertises
//! `durable-subscribers-v1` (jms bridge >= 0.9.0, task 1.4). A stale binary
//! fails closed in two shapes:
//!
//! - through the Rust component, the durable gate parks permanently with
//!   `bridge <pinned> lacks durable-subscribers-v1; requires a jms bridge >= 0.9.0 (pinned <pinned>)`;
//! - through this raw gRPC harness, an old bridge ignores the unknown
//!   `client_id`/`subscription_name` proto fields (proto3 unknown-field
//!   tolerance) and silently creates a volatile consumer, so the offline
//!   payload is never delivered and the reattach subscribe times out. This
//!   harness additionally asserts the advertised capability right after attach,
//!   so a stale binary fails fast with the capability diagnostic instead.
//!
//! Requires Docker and a locally built bridge binary
//! (`cargo xtask build-jms-bridge`, or `CAMEL_JMS_BRIDGE_BINARY_PATH`).
//!
//! The broker is a plaintext `tcp://` Classic container: this row proves
//! durability, not TLS (broker TLS has its own entry point, `jms_tls_test.rs`),
//! so the bridge is spawned with the no-TLS `spawn_jms_bridge_configured`
//! helper (it passes `None` for the TLS env contract). `spawn_jms_bridge_tls`
//! is deliberately NOT used: it requires a `&BrokerTlsConfig` and exports the
//! TLS env contract that a plaintext broker must not carry.

#![cfg(feature = "integration-tests")]

mod support;

use std::collections::HashMap;
use std::time::Duration;

use camel_bridge::process::BrokerType;
use camel_component_jms::proto::bridge_service_client::BridgeServiceClient;
use camel_component_jms::proto::{
    CompletionOutcome, CompletionRequest, HealthRequest, JmsMessage, SendRequest, SubscribeRequest,
};
use support::activemq::ActiveMqBroker;
use support::jms_bridge_procs::{require_jms_bridge_binary, spawn_jms_bridge_configured};
use support::wait::wait_until;

/// Durable identity shared by the first attach and the reattach. Both values
/// are route configuration, never per-process generated state.
const CLIENT_ID: &str = "durable-reconnect-app";
const SUBSCRIPTION_NAME: &str = "durable-reconnect-snapshots";
/// Topic destination (durable identity is topic-only).
const TOPIC: &str = "topic:durable-reconnect";
const WARMUP_BODY: &str = "durable-reconnect-warmup";
/// Distinct payload published while no consumer is attached.
const OFFLINE_BODY: &str = "durable-reconnect-offline";

// ─────────────────────────────────────────────────────────────────────────────
// Bounded bridge helpers (same shapes as jms_test.rs / jms_tls_test.rs).
// ─────────────────────────────────────────────────────────────────────────────

/// Waits until the bridge process reports a live broker connection, so the
/// subsequent single-shot sends do not race lazy broker setup.
async fn await_broker_connected(client: &mut BridgeServiceClient<tonic::transport::Channel>) {
    wait_until(
        "bridge broker connection",
        Duration::from_secs(60),
        Duration::from_millis(500),
        || {
            let mut probe = client.clone();
            async move {
                Ok(probe
                    .health(HealthRequest {})
                    .await
                    .map(|r| {
                        let h = r.into_inner();
                        h.healthy && h.broker_connected
                    })
                    .unwrap_or(false))
            }
        },
    )
    .await
    .expect("bridge did not report a broker connection in time");
}

/// Asserts the launched bridge advertises `durable-subscribers-v1` (bridge >=
/// 0.9.0). A stale binary fails here, before the reconnect sequence, with the
/// capability diagnostic instead of a later offline-delivery timeout.
async fn assert_durable_capability(client: &mut BridgeServiceClient<tonic::transport::Channel>) {
    let health = tokio::time::timeout(Duration::from_secs(15), client.health(HealthRequest {}))
        .await
        .expect("timed out reading bridge health for the durable capability")
        .expect("health RPC for the durable capability")
        .into_inner();
    assert!(
        health
            .capabilities
            .iter()
            .any(|capability| capability == "durable-subscribers-v1"),
        "bridge binary does not advertise durable-subscribers-v1; rebuild it with \
         `cargo xtask build-jms-bridge` from a jms bridge >= 0.9.0 \
         (advertised capabilities: {:?})",
        health.capabilities
    );
}

/// Sends one text message through `client` (single shot — no retries).
async fn send_one(
    client: &mut BridgeServiceClient<tonic::transport::Channel>,
    topic: &str,
    body: &str,
) -> String {
    let response = tokio::time::timeout(
        Duration::from_secs(30),
        client.send(SendRequest {
            destination: topic.to_string(),
            body: body.as_bytes().to_vec(),
            headers: HashMap::new(),
            content_type: "text/plain".to_string(),
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("timed out sending body {body:?} to {topic}"))
    .unwrap_or_else(|e| panic!("send of body {body:?} to {topic} failed: {e}"));
    response.into_inner().message_id
}

/// Builds a durable Subscribe request: stable `client_id`/`subscription_name`
/// plus a fresh per-stream `subscription_id`.
fn durable_subscribe_request(topic: &str, subscription_id: &str) -> SubscribeRequest {
    SubscribeRequest {
        destination: topic.to_string(),
        subscription_id: subscription_id.to_string(),
        message_selector: String::new(),
        client_id: CLIENT_ID.to_string(),
        subscription_name: SUBSCRIPTION_NAME.to_string(),
    }
}

/// Issues the subscribe RPC with a bounded await and returns the subscription
/// id together with the message stream. The bridge's gRPC server can hold the
/// response headers until the first message arrives, so an unbounded `await` on
/// an empty topic would hang. The durable identity of every request is logged
/// so the reconnect evidence (same identity, fresh stream id) is visible in the
/// test output.
async fn subscribe_bounded(
    client: &mut BridgeServiceClient<tonic::transport::Channel>,
    request: SubscribeRequest,
) -> (String, tonic::Streaming<JmsMessage>) {
    let subscription_id = request.subscription_id.clone();
    eprintln!(
        "durable subscribe: subscription_id={} destination={} client_id={} subscription_name={}",
        request.subscription_id, request.destination, request.client_id, request.subscription_name
    );
    let response = tokio::time::timeout(Duration::from_secs(30), client.subscribe(request))
        .await
        .unwrap_or_else(|_| panic!("timed out subscribing durable consumer to {TOPIC}"))
        .expect("subscribe via the durable bridge process");
    (subscription_id, response.into_inner())
}

/// Receives exactly one message from the subscription stream, bounded.
async fn next_message(stream: &mut tonic::Streaming<JmsMessage>, what: &str) -> JmsMessage {
    tokio::time::timeout(Duration::from_secs(35), stream.message())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
        .unwrap_or_else(|e| panic!("stream error while waiting for {what}: {e}"))
        .unwrap_or_else(|| panic!("subscription stream ended before {what} arrived"))
}

/// Reports `COMMIT` for one delivery through the Completion RPC.
async fn commit_delivery(
    client: &mut BridgeServiceClient<tonic::transport::Channel>,
    subscription_id: &str,
    message: &JmsMessage,
    what: &str,
) {
    let response = tokio::time::timeout(
        Duration::from_secs(30),
        client.complete(CompletionRequest {
            subscription_id: subscription_id.to_string(),
            subscription_generation: message.subscription_generation,
            delivery_token: message.delivery_token.clone(),
            outcome: CompletionOutcome::Commit as i32,
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("timed out committing {what}"))
    .unwrap_or_else(|e| panic!("completion RPC failed for {what}: {e}"));
    assert!(
        response.into_inner().accepted,
        "bridge rejected COMMIT for {what}"
    );
}

/// Receives the next message and commits it before returning. The bridge holds
/// at most one unacknowledged delivery per consumer session.
async fn next_message_committed(
    client: &mut BridgeServiceClient<tonic::transport::Channel>,
    subscription_id: &str,
    stream: &mut tonic::Streaming<JmsMessage>,
    what: &str,
) -> JmsMessage {
    let message = next_message(stream, what).await;
    commit_delivery(client, subscription_id, &message, what).await;
    message
}

/// Drains warmup duplicates that were queued by the publish/subscribe race.
/// The attach publishes warmups until the subscribe RPC returns, so at most a
/// couple can sit behind the first delivery; a short idle window proves the
/// durable subscription is empty before the offline publish.
async fn drain_warmup_duplicates(
    client: &mut BridgeServiceClient<tonic::transport::Channel>,
    subscription_id: &str,
    stream: &mut tonic::Streaming<JmsMessage>,
) {
    loop {
        // allow-test-wait: per-iteration receive is 2s-timeout-bounded; COMMIT is internally bounded
        match tokio::time::timeout(Duration::from_secs(2), stream.message()).await {
            Err(_idle) => return,
            Ok(Err(e)) => panic!("stream error while draining warmup duplicates: {e}"),
            Ok(Ok(None)) => return,
            Ok(Ok(Some(message))) => {
                assert_eq!(
                    String::from_utf8_lossy(&message.body),
                    WARMUP_BODY,
                    "only warmup duplicates may be queued before the offline publish"
                );
                commit_delivery(client, subscription_id, &message, "warmup duplicate").await;
            }
        }
    }
}

/// Waits until the durable client id is present in (or absent from) the
/// broker's live connection list. Presence proves the explicit client id won
/// over prefix autogeneration; absence after the bridge is killed proves no
/// consumer is attached for the offline publish.
async fn wait_for_durable_client(broker: &ActiveMqBroker, expected_present: bool) {
    wait_until(
        if expected_present {
            "durable client id on the broker connection list"
        } else {
            "durable client id to leave the broker connection list"
        },
        Duration::from_secs(60),
        Duration::from_millis(200),
        || async move {
            let ids = broker.client_ids().await?;
            let present = ids.iter().any(|id| id == CLIENT_ID);
            if present == expected_present {
                Ok(true)
            } else {
                Err(format!(
                    "durable client id {CLIENT_ID} presence is {present}, expected \
                     {expected_present} (live client ids: {ids:?})"
                ))
            }
        },
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
}

// ─────────────────────────────────────────────────────────────────────────────
// CI-owned native row (run with `--ignored`).
// ─────────────────────────────────────────────────────────────────────────────

/// Durable topic subscription survives a disconnect: the warmup delivery is
/// committed, the bridge (and with it the consumer connection) is torn down, a
/// distinct message is published while no consumer is attached, and a reattach
/// with the SAME durable identity (fresh stream id) receives it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow test: CI-owned durable reconnect — containerized Classic broker + built jms-bridge (run with --ignored)"]
async fn durable_survives_reconnect() {
    support::init_tracing();
    support::install_crypto_provider();

    // Plaintext Classic broker: durability, not TLS, is under test.
    let broker = ActiveMqBroker::start().await;
    let binary = require_jms_bridge_binary();

    // ── Attach: durable consumer + warmup delivery ──────────────────────────
    let (bridge_attach, channel_attach) = spawn_jms_bridge_configured(
        &binary,
        &broker.broker_url,
        BrokerType::ActiveMq,
        "admin",
        "admin",
    )
    .await;
    let mut client_attach = BridgeServiceClient::new(channel_attach);
    await_broker_connected(&mut client_attach).await;
    assert_durable_capability(&mut client_attach).await;

    let first_request = durable_subscribe_request(TOPIC, "durable-stream-1");
    let first_identity = (
        first_request.client_id.clone(),
        first_request.subscription_name.clone(),
    );

    // The subscribe RPC can withhold its headers until the first delivery, so
    // the warmup publish runs CONCURRENTLY with it: the stream must never block
    // awaiting a message that nobody published. Warmups are republished on a
    // bounded cadence because a publish that races the broker-side durable
    // subscription creation is dropped (the topic has no subscriber yet).
    let mut subscribe_client = client_attach.clone();
    let mut warmup_client = client_attach.clone();
    let mut subscribe_task =
        tokio::spawn(async move { subscribe_bounded(&mut subscribe_client, first_request).await });

    let warmup_deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let (first_subscription_id, mut first_stream) = loop {
        // allow-test-wait: publish is internally bounded, join is 1s-timeout-bounded, overall 60s deadline
        let sent_id = send_one(&mut warmup_client, TOPIC, WARMUP_BODY).await;
        assert!(
            !sent_id.is_empty(),
            "the broker must assign a message id to the warmup publish"
        );
        match tokio::time::timeout(Duration::from_secs(1), &mut subscribe_task).await {
            Ok(joined) => break joined.expect("durable attach subscribe task panicked"),
            Err(_elapsed) if tokio::time::Instant::now() >= warmup_deadline => {
                panic!(
                    "the durable attach never delivered a warmup message within 60s; a stale \
                     bridge binary that ignores the durable identity produces this signature"
                );
            }
            Err(_elapsed) => {}
        }
    };

    let warmup = next_message_committed(
        &mut client_attach,
        &first_subscription_id,
        &mut first_stream,
        "warmup delivery on the durable attach",
    )
    .await;
    assert_eq!(
        String::from_utf8_lossy(&warmup.body),
        WARMUP_BODY,
        "the durable attach must deliver the published warmup body intact"
    );
    drain_warmup_duplicates(
        &mut client_attach,
        &first_subscription_id,
        &mut first_stream,
    )
    .await;

    // The explicit client id must be the broker-side connection identity.
    wait_for_durable_client(&broker, true).await;

    // ── Detach: stream cancelled AND bridge process killed ──────────────────
    // Dropping the stream cancels the subscribe RPC; dropping the process
    // closes every broker connection (no unsubscribe is issued, so the durable
    // subscription stays registered on the broker).
    drop(first_stream);
    drop(warmup_client);
    drop(client_attach);
    drop(bridge_attach);
    wait_for_durable_client(&broker, false).await;

    // ── Offline publish: no consumer attached ───────────────────────────────
    // The process handle is held (not `_`) so the reattach bridge stays alive
    // until the test ends.
    let (_bridge_reattach, channel_reattach) = spawn_jms_bridge_configured(
        &binary,
        &broker.broker_url,
        BrokerType::ActiveMq,
        "admin",
        "admin",
    )
    .await;
    let mut client_reattach = BridgeServiceClient::new(channel_reattach);
    await_broker_connected(&mut client_reattach).await;

    let offline_id = send_one(&mut client_reattach, TOPIC, OFFLINE_BODY).await;
    assert!(
        !offline_id.is_empty(),
        "the broker must assign a message id to the offline publish"
    );

    // ── Reattach: identical durable identity, fresh stream id ───────────────
    let second_request = durable_subscribe_request(TOPIC, "durable-stream-2");
    assert_eq!(
        (
            second_request.client_id.clone(),
            second_request.subscription_name.clone()
        ),
        first_identity,
        "the reattach must present the identical durable identity (client_id + subscription_name)"
    );
    assert_ne!(
        second_request.subscription_id, first_subscription_id,
        "each subscribe stream keeps a fresh subscription_id"
    );

    let (second_subscription_id, mut second_stream) =
        subscribe_bounded(&mut client_reattach, second_request).await;
    let offline = next_message_committed(
        &mut client_reattach,
        &second_subscription_id,
        &mut second_stream,
        "offline delivery after the durable reattach",
    )
    .await;
    assert_eq!(
        String::from_utf8_lossy(&offline.body),
        OFFLINE_BODY,
        "the message published while no consumer was attached must be delivered to the \
         reattached durable subscription"
    );
}
