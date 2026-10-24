//! CI-owned broker TLS native smoke entry point (task 9.1, change
//! `jms-tls-classic`).
//!
//! **Owner: CI.** This module is `#[ignore]`d by default (ADR-0054 `slow test:`
//! classification — a containerized broker plus a built bridge binary are slow
//! prerequisites the per-PR gate cannot cheaply satisfy) and CI runs it
//! explicitly with the `--ignored` flag:
//!
//! ```text
//! cargo test -p camel-test --features integration-tests --test jms_tls_test -- --ignored
//! ```
//!
//! Full CI-owned native broker-TLS matrix:
//!
//! | Row | Expectation |
//! | --- | --- |
//! | private CA pinned via `ca_cert_path` | round-trip OK |
//! | wrong CA pinned via `ca_cert_path` | `TLS_TRUST`, fail-closed (no round-trip) |
//! | DNS-only SAN addressed by IP | `TLS_HOSTNAME` |
//! | DNS-only SAN addressed by IP + `tls.server_name` | OK |
//! | IP SAN addressed by IP | OK |
//! | `needClientAuth` without a client pair | `TLS_CLIENT_AUTH` |
//! | `needClientAuth` with a client pair | OK (mTLS) |
//! | `failover:(ssl://…)` with uniform transport security | OK |
//! | old bridge binary + `tls` table | permanent refusal (`broker-tls-v1`) |
//! | `tcp://` without a `tls` table | regression: plaintext path unchanged |
//!
//! The platform-trust positive (a public-CA broker with no `tls` table) cannot be tested hermetically; CI proves only the negative (a private-CA broker without a pinned CA fails with `TLS_TRUST`).
//!
//! This harness implements two of those rows end-to-end against a real
//! containerized ActiveMQ Classic broker: the private-CA / IP-SAN positive row
//! (`tls_roundtrip_positive`) and the wrong-CA negative row
//! (`tls_wrong_ca_fails_closed`). The remaining rows run in the JVM transport
//! matrix (`bridges/jms` `TlsTransportMatrixTest`) and the Rust config/unit
//! matrices; they are enumerated here so this module is the single native entry
//! point that names them.
//!
//! Requires Docker and a locally built bridge binary
//! (`cargo xtask build-jms-bridge`, or `CAMEL_JMS_BRIDGE_BINARY_PATH`).

#![cfg(feature = "integration-tests")]

mod support;

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use camel_component_jms::config::BrokerTlsConfig;
use camel_component_jms::proto::bridge_service_client::BridgeServiceClient;
use camel_component_jms::proto::{
    BrokerFailureClass, CompletionOutcome, CompletionRequest, HealthRequest, HealthStatus,
    JmsMessage, SendRequest, SubscribeRequest,
};
use support::activemq::{ActiveMqBroker, ActiveMqBrokerOptions};
use support::jms_bridge_procs::{require_jms_bridge_binary, spawn_jms_bridge_tls};
use support::wait::wait_until;

/// Throwaway password for the ephemeral broker keystore. Not a secret: the
/// fixture lives only inside one test run.
const KEYSTORE_PASSWORD: &str = "changeit";

// ─────────────────────────────────────────────────────────────────────────────
// TLS fixture: one containerized Classic broker with an `ssl://` connector.
// rcgen CA + server certificate (support/redis.rs pattern), PKCS#12 broker
// keystore (support/cxf.rs p12-keystore pattern), custom `activemq.xml`.
// ─────────────────────────────────────────────────────────────────────────────

/// Containerized Classic broker with an `ssl://` connector plus the two trust
/// anchors the smoke rows need: the CA that signed the broker certificate
/// (`tls`) and an unrelated CA (`wrong_ca_tls`) for the fail-closed row.
struct TlsBrokerFixture {
    broker: ActiveMqBroker,
    /// `tls` table pinning the signing CA — the positive row.
    tls: BrokerTlsConfig,
    /// Same table shape pinning a CA that did NOT sign the broker
    /// certificate — the negative row.
    wrong_ca_tls: BrokerTlsConfig,
    /// Broker URL the bridge dials: the mapped `ssl://` connector.
    ssl_broker_url: String,
    /// Owns the CA PEM files the `tls` tables point at.
    _dir: tempfile::TempDir,
}

async fn start_tls_broker() -> TlsBrokerFixture {
    // rcgen CA + server certificate for IP SAN 127.0.0.1 (redis.rs pattern).
    let ca_key = rcgen::KeyPair::generate().expect("TLS smoke CA key pair");
    let mut ca_params = rcgen::CertificateParams::default();
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "jms-tls-smoke-ca");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params
        .key_usages
        .push(rcgen::KeyUsagePurpose::KeyCertSign);
    ca_params.key_usages.push(rcgen::KeyUsagePurpose::CrlSign);
    let ca_cert = ca_params
        .self_signed(&ca_key)
        .expect("TLS smoke CA certificate");

    // An unrelated CA: the fail-closed row pins THIS one as ca_cert_path.
    let wrong_key = rcgen::KeyPair::generate().expect("TLS smoke wrong-CA key pair");
    let mut wrong_params = rcgen::CertificateParams::default();
    wrong_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "jms-tls-smoke-wrong-ca");
    wrong_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let wrong_cert = wrong_params
        .self_signed(&wrong_key)
        .expect("TLS smoke wrong-CA certificate");

    let server_key = rcgen::KeyPair::generate().expect("TLS smoke server key pair");
    let server_params =
        rcgen::CertificateParams::new(vec![IpAddr::from([127, 0, 0, 1]).to_string()])
            .expect("TLS smoke server certificate params");
    let issuer = rcgen::Issuer::from_params(&ca_params, &ca_key);
    let server_cert = server_params
        .signed_by(&server_key, &issuer)
        .expect("TLS smoke server certificate");

    // PKCS#12 broker keystore (support/cxf.rs pattern): the server key with
    // its certificate and the signing CA as the chain.
    let server_entry =
        p12_keystore::Certificate::from_der(server_cert.der()).expect("broker keystore server DER");
    let ca_entry =
        p12_keystore::Certificate::from_der(ca_cert.der()).expect("broker keystore CA DER");
    let chain = p12_keystore::PrivateKeyChain::new(
        server_key.serialize_der(),
        b"broker",
        [server_entry, ca_entry],
    );
    let mut store = p12_keystore::KeyStore::new();
    store.add_entry(
        "broker",
        p12_keystore::KeyStoreEntry::PrivateKeyChain(chain),
    );
    let keystore = store
        .writer(KEYSTORE_PASSWORD)
        .write()
        .expect("broker PKCS12 keystore encodes");

    let broker = ActiveMqBroker::start_with(ActiveMqBrokerOptions {
        conf_files: vec![
            ("activemq.xml".to_string(), activemq_xml().into_bytes()),
            ("broker.ks".to_string(), keystore),
        ],
        expose_tls_connector: true,
    })
    .await;
    let ssl_broker_url = broker
        .ssl_broker_url
        .clone()
        .expect("TLS fixture exposes the ssl:// connector URL");

    // Bridge-side CA material: PEM files pinned through the `tls` table.
    let dir = tempfile::tempdir().expect("TLS smoke material tempdir");
    let pinned_ca_file = dir.path().join("pinned-ca.pem");
    std::fs::write(&pinned_ca_file, ca_cert.pem()).expect("write pinned CA PEM");
    let wrong_ca_file = dir.path().join("wrong-ca.pem");
    std::fs::write(&wrong_ca_file, wrong_cert.pem()).expect("write wrong CA PEM");

    let tls = BrokerTlsConfig {
        ca_cert_path: Some(path_string(&pinned_ca_file)),
        client_cert_path: None,
        client_key_path: None,
        server_name: None,
    };
    let wrong_ca_tls = BrokerTlsConfig {
        ca_cert_path: Some(path_string(&wrong_ca_file)),
        client_cert_path: None,
        client_key_path: None,
        server_name: None,
    };

    TlsBrokerFixture {
        broker,
        tls,
        wrong_ca_tls,
        ssl_broker_url,
        _dir: dir,
    }
}

fn path_string(path: &std::path::Path) -> String {
    path.to_str()
        .expect("TLS smoke paths are UTF-8")
        .to_string()
}

/// Custom `activemq.xml` for the TLS fixture: the stock Classic 5.18.3 broker
/// shape plus an `sslContext` PKCS#12 keystore and an `ssl://` transport
/// connector alongside the plaintext `tcp://` one the container readiness
/// check logs against. `transport.needClientAuth=false` keeps the connector
/// trust-only (the mTLS rows live in the JVM matrix).
fn activemq_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<beans
  xmlns="http://www.springframework.org/schema/beans"
  xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
  xsi:schemaLocation="http://www.springframework.org/schema/beans http://www.springframework.org/schema/beans/spring-beans.xsd
  http://activemq.apache.org/schema/core http://activemq.apache.org/schema/core/activemq-core.xsd">

    <bean class="org.springframework.beans.factory.config.PropertyPlaceholderConfigurer">
        <property name="locations">
            <value>file:${{activemq.conf}}/credentials.properties</value>
        </property>
    </bean>

    <broker xmlns="http://activemq.apache.org/schema/core" brokerName="localhost" dataDirectory="${{activemq.data}}">

        <sslContext>
            <sslContext keyStore="file://${{activemq.conf}}/broker.ks"
                        keyStorePassword="{KEYSTORE_PASSWORD}"
                        keyStoreType="pkcs12"/>
        </sslContext>

        <managementContext>
            <managementContext createConnector="false"/>
        </managementContext>

        <persistenceAdapter>
            <kahaDB directory="${{activemq.data}}/kahadb"/>
        </persistenceAdapter>

        <transportConnectors>
            <transportConnector name="openwire" uri="tcp://0.0.0.0:61616"/>
            <transportConnector name="ssl" uri="ssl://0.0.0.0:61617?transport.needClientAuth=false"/>
        </transportConnectors>

    </broker>

</beans>
"#
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Bounded bridge helpers (same shapes as jms_test.rs).
// ─────────────────────────────────────────────────────────────────────────────

/// Waits until the bridge process reports a live broker connection, so the
/// subsequent single-shot sends do not race lazy broker setup.
async fn await_broker_connected(client: &mut BridgeServiceClient<tonic::transport::Channel>) {
    wait_until(
        "bridge broker connection over TLS",
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
    .expect("bridge did not report a broker connection over TLS in time");
}

/// Sends one text message through `client` (single shot — no retries).
async fn send_one(
    client: &mut BridgeServiceClient<tonic::transport::Channel>,
    queue: &str,
    body: &str,
) -> String {
    let response = tokio::time::timeout(
        Duration::from_secs(30),
        client.send(SendRequest {
            destination: queue.to_string(),
            body: body.as_bytes().to_vec(),
            headers: HashMap::new(),
            content_type: "text/plain".to_string(),
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("timed out sending body {body:?} over TLS"))
    .unwrap_or_else(|e| panic!("send of body {body:?} over TLS failed: {e}"));
    response.into_inner().message_id
}

/// Issues the subscribe RPC with a bounded await and returns the subscription
/// id together with the message stream. The bridge's gRPC server can hold the
/// response headers until the first message arrives on the queue, so tests
/// queue their messages before subscribing and bound this call too.
async fn subscribe_bounded(
    consumer: &mut BridgeServiceClient<tonic::transport::Channel>,
    queue: &str,
    subscription_id: &str,
) -> (String, tonic::Streaming<JmsMessage>) {
    let subscription_id = subscription_id.to_string();
    let response = tokio::time::timeout(
        Duration::from_secs(30),
        consumer.subscribe(SubscribeRequest {
            destination: queue.to_string(),
            subscription_id: subscription_id.clone(),
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("timed out subscribing consumer to {queue}"))
    .expect("subscribe via the TLS bridge process");
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

/// Receives the next message and reports `COMMIT` for it before returning.
/// The bridge holds at most one unacknowledged delivery per consumer session.
async fn next_message_committed(
    client: &mut BridgeServiceClient<tonic::transport::Channel>,
    subscription_id: &str,
    stream: &mut tonic::Streaming<JmsMessage>,
    what: &str,
) -> JmsMessage {
    let message = next_message(stream, what).await;
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
    message
}

// ─────────────────────────────────────────────────────────────────────────────
// CI-owned smoke rows (run with `--ignored`).
// ─────────────────────────────────────────────────────────────────────────────

/// Positive row: private CA pinned via the `tls` table, addressed by IP SAN.
/// Produce → consume round-trip over the `ssl://` connector, health `UP`, and
/// the `broker-tls-v1` capability present.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow test: CI-owned broker TLS smoke — containerized Classic broker + built jms-bridge (run with --ignored)"]
async fn tls_roundtrip_positive() {
    support::init_tracing();
    support::install_crypto_provider();

    let fixture = start_tls_broker().await;
    let binary = require_jms_bridge_binary();
    let queue = "queue.tls-smoke-roundtrip";

    // Spawn the bridge with the `tls` table pinning the signing CA.
    let (_bridge, channel) =
        spawn_jms_bridge_tls(&binary, &fixture.ssl_broker_url, &fixture.tls).await;
    let mut client = BridgeServiceClient::new(channel);

    // The broker connection must come up over the ssl:// connector first.
    await_broker_connected(&mut client).await;

    // Produce one message over TLS, then consume it back over TLS.
    let sent_id = send_one(&mut client, queue, "tls-smoke-roundtrip").await;
    assert!(
        !sent_id.is_empty(),
        "the broker must assign a message id to the TLS-produced message"
    );

    let (subscription_id, mut stream) =
        subscribe_bounded(&mut client, queue, "tls-smoke-sub").await;
    let message =
        next_message_committed(&mut client, &subscription_id, &mut stream, "TLS round-trip").await;
    assert_eq!(
        String::from_utf8_lossy(&message.body),
        "tls-smoke-roundtrip",
        "the TLS round-trip must deliver the produced body intact"
    );

    // Health: UP and the broker-tls-v1 capability.
    let health = tokio::time::timeout(Duration::from_secs(15), client.health(HealthRequest {}))
        .await
        .expect("timed out reading bridge health after the TLS round-trip")
        .expect("health RPC after the TLS round-trip")
        .into_inner();
    assert!(
        health.healthy,
        "bridge must report healthy after the TLS round-trip"
    );
    assert_eq!(
        health.status,
        HealthStatus::Up as i32,
        "health status must be UP after the TLS round-trip"
    );
    assert!(
        health.capabilities.iter().any(|c| c == "broker-tls-v1"),
        "a TLS-capable bridge must report the broker-tls-v1 capability: {:?}",
        health.capabilities
    );
}

/// Negative row: the `tls` table pins a CA that did NOT sign the broker
/// certificate. The trust check fails closed: Health surfaces the `TLS_TRUST`
/// failure class and no message round-trip occurs.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow test: CI-owned broker TLS smoke — containerized Classic broker + built jms-bridge (run with --ignored)"]
async fn tls_wrong_ca_fails_closed() {
    support::init_tracing();
    support::install_crypto_provider();

    let fixture = start_tls_broker().await;
    let binary = require_jms_bridge_binary();
    let queue = "queue.tls-smoke-wrong-ca";

    // Same fixture shape, wrong trust anchor pinned as ca_cert_path.
    let (_bridge, channel) =
        spawn_jms_bridge_tls(&binary, &fixture.ssl_broker_url, &fixture.wrong_ca_tls).await;
    let mut client = BridgeServiceClient::new(channel);

    // The failure surfaces as the TLS_TRUST class on the Health probe.
    wait_until(
        "TLS_TRUST failure class on bridge health",
        Duration::from_secs(60),
        Duration::from_millis(500),
        || {
            let mut probe = client.clone();
            async move {
                let health = probe
                    .health(HealthRequest {})
                    .await
                    .map_err(|e| format!("health RPC failed: {e}"))?
                    .into_inner();
                if health.failure_class == BrokerFailureClass::TlsTrust as i32 {
                    Ok(true)
                } else {
                    Err(format!(
                        "failure class is {} (TLS_TRUST = {}), not observed yet",
                        health.failure_class,
                        BrokerFailureClass::TlsTrust as i32
                    ))
                }
            }
        },
    )
    .await
    .expect("the bridge never surfaced the TLS_TRUST failure class");

    // No message round-trip occurs: a send cannot succeed without a broker
    // connection, and the broker-side queue must stay empty.
    let send = tokio::time::timeout(
        Duration::from_secs(20),
        client.send(SendRequest {
            destination: queue.to_string(),
            body: b"tls-smoke-must-not-arrive".to_vec(),
            headers: HashMap::new(),
            content_type: "text/plain".to_string(),
        }),
    )
    .await;
    match send {
        // A bounded timeout without a broker connection is fail-closed.
        Err(_elapsed) => {}
        // The expected fail-closed outcome: the send RPC errors out.
        Ok(Err(_status)) => {}
        Ok(Ok(_)) => panic!("send succeeded despite the TLS_TRUST trust failure"),
    }

    let enqueued = fixture
        .broker
        .enqueue_count(queue)
        .await
        .expect("broker-side enqueue count after the failed send");
    assert_eq!(
        enqueued, 0,
        "no message may reach the broker while the trust check fails"
    );
}
