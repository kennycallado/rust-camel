//! CI-owned Artemis rows for the invalid-property policy (bd rc-ooczk). Requires Docker
//! and the jms-bridge binary; run via `cargo test -p camel-test --features
//! integration-tests --test jms_test <name>`.
//!
//! The scenario is the spec's "An illegal name on Artemis dead-letters with a
//! name-only error". The JMS identifier rule (message-selector identifiers) excludes
//! `-`, and the Artemis client enforces it while ActiveMQ Classic tolerates it. The
//! bridge does not encode that legality set: it accumulates the property writes the
//! provider rejects and fails the send with a name-only typed error, which the service
//! maps to gRPC `INVALID_ARGUMENT`. This module pins both halves against the shared
//! Artemis testcontainers fixture:
//!
//! * a hyphenated, allowlisted header fails the send, the route dead-letters, and the
//!   dead-letter error names the property and carries no value (nothing is delivered to
//!   a consumer on the destination); and
//! * a legal identifier forwards byte-identically and round-trips through a consumer.
//!
//! The queue names are distinct from every other row sharing the bridge.

use std::time::Duration;

use camel_api::Value;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_test::CamelTestContext;

use super::support::jms::shared_jms_artemis;
use super::support::wait::wait_until;
use super::support::{init_tracing, install_crypto_provider};

/// A hyphenated name: legal JMS-looking but not a valid identifier, so the Artemis
/// client rejects the property write.
const ILLEGAL_PROPERTY: &str = "x-crash-proof";

/// The value the rejected property carried. The dead-letter error must never show it.
const ILLEGAL_VALUE: &str = "proof-value";

/// The body of the send that the broker must reject.
const ILLEGAL_BODY: &str = "invalid-prop-check";

/// A legal identifier on every broker: the control name.
const LEGAL_PROPERTY: &str = "x_crash_proof";

/// The value the legal control property carries; the consumer must see it verbatim.
const LEGAL_VALUE: &str = "legal-value";

/// The body of the control send.
const LEGAL_BODY: &str = "legal-prop-check";

/// Bound for both the rejection wait and the zero-delivery observation window.
const TIMEOUT: Duration = Duration::from_secs(35);

/// Poll interval matching the other rows in this suite.
const POLL: Duration = Duration::from_millis(200);

#[tokio::test(flavor = "multi_thread")]
async fn jms_invalid_property_rejected_on_artemis() {
    init_tracing();
    install_crypto_provider();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_artemis().await)
        .build()
        .await;

    // A consumer on the same destination: a rejected send must never reach it.
    let consumer_route = RouteBuilder::from("jms:queue:test-invalid-prop")
        .to("mock:not-delivered")
        .route_id("jms-invalid-prop-consumer")
        .build()
        .unwrap();

    let producer_route = RouteBuilder::from("timer:invalid-prop?delay=500&repeatCount=1")
        .set_body(ILLEGAL_BODY.to_string())
        .set_header(ILLEGAL_PROPERTY, Value::String(ILLEGAL_VALUE.to_string()))
        .to("jms:queue:test-invalid-prop?jmsHeaderAllowlist=x-crash-proof")
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:dlc"))
        .route_id("jms-invalid-prop-producer")
        .build()
        .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.add_route(producer_route).await.unwrap();
    h.start().await;

    // The route error handler owns the operational signal: the failed send is
    // dead-lettered with the name-only rejection as the exchange error.
    let dlc = h.mock().get_endpoint("dlc").unwrap();
    wait_until(
        "invalid-property rejection reaches the dead-letter channel",
        TIMEOUT,
        POLL,
        || {
            let dlc = dlc.clone();
            async move {
                Ok(dlc
                    .get_received_exchanges()
                    .await
                    .iter()
                    .any(|ex| error_text(ex).is_some_and(|text| text.contains(ILLEGAL_PROPERTY))))
            }
        },
    )
    .await
    .unwrap();

    let exchanges = dlc.get_received_exchanges().await;
    let rejection = exchanges
        .iter()
        .filter_map(error_text)
        .find(|text| text.contains(ILLEGAL_PROPERTY))
        .expect("the wait proved a dead-lettered rejection naming the property");

    // Compare to booleans so a regression can never print the offending error text
    // (which could carry the value) in a failure diagnostic.
    let names_property = rejection.contains(ILLEGAL_PROPERTY);
    assert!(
        names_property,
        "the dead-letter error text must name the rejected JMS property"
    );
    let leaks_value = rejection.contains(ILLEGAL_VALUE);
    assert!(
        !leaks_value,
        "the dead-letter error text must not carry the property value"
    );

    // MUST-STAY-ZERO: the send was rejected before delivery, so the destination
    // consumer receives nothing. Invert the bounded wait: pass only if the whole
    // window elapses with zero receipts.
    let not_delivered = h.mock().get_endpoint("not-delivered").unwrap();
    let delivered = tokio::time::timeout(TIMEOUT, async {
        loop {
            let received = not_delivered.get_received_exchanges().await.len();
            if received > 0 {
                return received;
            }
            tokio::time::sleep(POLL).await; // allow-test-sleep: bounded zero-delivery window
        }
    })
    .await;
    assert!(
        delivered.is_err(),
        "a rejected send must never be delivered to a consumer on the destination"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jms_legal_property_forwards_on_artemis() {
    init_tracing();
    install_crypto_provider();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(shared_jms_artemis().await)
        .build()
        .await;

    let consumer_route =
        RouteBuilder::from("jms:queue:test-legal-prop?jmsHeaderAllowlist=x_crash_proof")
            .to("mock:legal-props")
            .route_id("jms-legal-prop-consumer")
            .build()
            .unwrap();

    let producer_route = RouteBuilder::from("timer:legal-prop?delay=500&repeatCount=1")
        .set_body(LEGAL_BODY.to_string())
        .set_header(LEGAL_PROPERTY, Value::String(LEGAL_VALUE.to_string()))
        .to("jms:queue:test-legal-prop?jmsHeaderAllowlist=x_crash_proof")
        .route_id("jms-legal-prop-producer")
        .build()
        .unwrap();

    h.add_route(consumer_route).await.unwrap();
    h.add_route(producer_route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("legal-props").unwrap();
    wait_until(
        "legal-name control message reaches the Artemis consumer",
        TIMEOUT,
        POLL,
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    let exchanges = endpoint.get_received_exchanges().await;
    let received = exchanges
        .first()
        .expect("the wait proved the consumer received the control message");
    assert_eq!(
        received
            .input
            .header(LEGAL_PROPERTY)
            .and_then(|v| v.as_str()),
        Some(LEGAL_VALUE),
        "the legal property must round-trip with its sent value"
    );
}

/// The exchange error as displayed text, when the exchange carries one.
fn error_text(exchange: &camel_api::Exchange) -> Option<String> {
    exchange.error.as_ref().map(ToString::to_string)
}
