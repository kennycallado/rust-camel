//! Task 5.5 drill: credentials never reach Classic broker JMS properties.
//!
//! Included as a focused submodule of `jms_test`, so the prescribed
//! `cargo test -p camel-test --features integration-tests --test jms_test <name>`
//! command selects the case by name.
//!
//! The spec's "Credentials never reach the wire" scenario has two halves: the
//! Rust producer must omit denied headers from the IPC `SendRequest` (covered by
//! the `camel-jms` unit test `wire_payload_omits_credentials_and_keeps_trace_id`),
//! and the broker must never expose a credential name or value as a JMS
//! property. This case proves the broker half against a **real** ActiveMQ
//! Classic broker.
//!
//! A real route sends `password`, `Authorization`, `api-key`, and a benign
//! `traceId`, and the producer URI names all four in `jmsHeaderAllowlist` — the
//! deny-before-allowlist rule must still drop the three credential variants.
//! The message stays on a consumer-free queue; the test browses it through the
//! Classic Queue MBean `browse()` operation (Jolokia) and asserts the positive
//! (`traceId` present with its sent value) and the negatives (no credential name
//! as a property, no credential value in any broker field).
//!
//! The credential values are distinct sentinels, never the broker's own
//! user/password, so a fixture credential can never be mistaken for a leaked
//! header. Assertions report generic labels only; no credential value is printed
//! in a log or a failure message.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_jms::{BrokerConfig, BrokerType, JmsBridgePool, JmsComponent, JmsPoolConfig};
use camel_test::CamelTestContext;

use super::support::activemq::{
    ACTIVEMQ_PASSWORD, ACTIVEMQ_USER, ActiveMqBroker, BrowsedMessage, safe_json_parse_error,
};
use super::support::wait::wait_until;
use super::support::{bridge_bg_rt, init_tracing, install_crypto_provider};

/// Consumer-free queue that holds the message until the browse.
const CREDENTIAL_QUEUE: &str = "queue.test-credentials-not-on-broker-properties-activemq";

/// A non-secret, known body; the browser must be able to read the message.
const KNOWN_BODY: &str = "credential-probe-body-5.5-known";

/// The four header names the allowlist names. The credentials are denied before
/// the allowlist; `traceId` is the benign header that must pass.
const PASSWORD_HEADER: &str = "password";
const AUTHORIZATION_HEADER: &str = "Authorization";
const API_KEY_HEADER: &str = "api-key";
const TRACE_HEADER: &str = "traceId";

/// Distinct credential sentinels. They are neither the broker's own
/// user/password (`admin`/`admin`) nor each other, so a fixture credential can
/// never be misread as a leaked header.
const PASSWORD_VALUE: &str = "pw-sentinel-5.5-9c1f";
const AUTHORIZATION_VALUE: &str = "auth-sentinel-5.5-7ab2";
const API_KEY_VALUE: &str = "apikey-sentinel-5.5-3d44";

/// The benign sentinel that must reach the broker as a JMS property.
const TRACE_VALUE: &str = "trace-sentinel-5.5-1e7a";

/// Every credential name/value pair the negatives must reject.
const CREDENTIALS: &[(&str, &str)] = &[
    (PASSWORD_HEADER, PASSWORD_VALUE),
    (AUTHORIZATION_HEADER, AUTHORIZATION_VALUE),
    (API_KEY_HEADER, API_KEY_VALUE),
];

/// Builds a JMS component pinned to the dedicated broker (identity and
/// credentials are fixture-owned, never ambient).
async fn dedicated_jms_component(broker: &ActiveMqBroker) -> JmsComponent {
    let pool_config = JmsPoolConfig {
        max_bridges: 1,
        brokers: HashMap::from([(
            "default".to_string(),
            BrokerConfig {
                broker_url: broker.broker_url.clone(),
                broker_type: BrokerType::ActiveMq,
                username: Some(ACTIVEMQ_USER.to_string()),
                password: Some(ACTIVEMQ_PASSWORD.to_string()),
            },
        )]),
        bridge_start_timeout_ms: 90_000,
        ..JmsPoolConfig::default()
    };

    let pool =
        Arc::new(JmsBridgePool::from_config(pool_config).expect("dedicated JMS pool config"));
    // Start the slot on the permanent background runtime so the tonic dispatch
    // task and health monitor are not killed when this test's runtime is dropped.
    let pool_init = pool.clone();
    let handle = bridge_bg_rt().spawn(async move { pool_init.get_or_create_slot("default").await });
    let _ = tokio::time::timeout(Duration::from_secs(90), handle).await;
    JmsComponent::with_scheme("jms", pool)
}

/// Waits, bounded, for the queue to hold a message, then returns the first
/// browsed message. Empty browses are retried.
async fn wait_for_browsed_message(broker: Arc<ActiveMqBroker>) -> BrowsedMessage {
    let wait_broker = broker.clone();
    wait_until(
        "credential probe message browsable on the queue",
        Duration::from_secs(30),
        Duration::from_millis(250),
        move || {
            let broker = wait_broker.clone();
            async move {
                let messages = broker.browse_messages(CREDENTIAL_QUEUE).await?;
                Ok(!messages.is_empty())
            }
        },
    )
    .await
    .expect("no browsable message on the credential queue");
    broker
        .browse_messages(CREDENTIAL_QUEUE)
        .await
        .expect("browse after wait")
        .into_iter()
        .next()
        .expect("the wait proved a message was browsable")
}

/// Fixed diagnostic for a missing or mismatched `traceId` property. It never
/// interpolates the broker's actual value (a value could be a credential on a
/// regression).
const TRACE_MISSING_DIAGNOSTIC: &str =
    "the allowlisted benign traceId property must be present with exactly its sent value";

/// The fixed, value-free diagnostic for the positive assertion.
fn trace_present_diagnostic(label: &str) -> String {
    format!("[{label}] {TRACE_MISSING_DIAGNOSTIC}")
}

/// Asserts the positive and negative broker-property contract on one browsed
/// message. Generic labels only; no credential value is printed.
fn assert_credential_contract(message: &BrowsedMessage, label: &str) {
    // Positive: the benign allowlisted header reached the broker as a property.
    // Compare to a boolean so a regression can never print the broker's actual
    // value; the diagnostic is a fixed string.
    let trace_present = message.property_value(TRACE_HEADER).as_deref() == Some(TRACE_VALUE);
    assert!(trace_present, "{}", trace_present_diagnostic(label));

    // Negative: no credential NAME is a broker JMS property.
    for (name, _) in CREDENTIALS {
        assert!(
            !message.has_property_name(name),
            "[{label}] a credential header name must not become a broker JMS property"
        );
    }

    // Negative: no credential VALUE appears in any broker-visible field.
    for (_, value) in CREDENTIALS {
        assert!(
            !message.any_field_contains(value),
            "[{label}] a credential header value must not appear in any broker property field"
        );
    }
}

/// Task 5.5: a route sends four headers with a producer allowlist naming all of
/// them; the benign `traceId` reaches the broker, and no credential name or
/// value appears in the broker JMS properties.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn jms_credentials_never_on_broker_properties_activemq() {
    init_tracing();
    install_crypto_provider();

    let broker = Arc::new(ActiveMqBroker::start().await);
    let component = dedicated_jms_component(&broker).await;

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(component)
        .build()
        .await;

    // The producer URI names all four headers in the allowlist. The three
    // credential variants are still denied by the deny-before-allowlist rule.
    let producer_uri = format!(
        "jms:queue:{CREDENTIAL_QUEUE}?jmsHeaderAllowlist={PASSWORD_HEADER},{AUTHORIZATION_HEADER},\
         {API_KEY_HEADER},{TRACE_HEADER}"
    );
    let route = RouteBuilder::from("timer:cred-inject?period=1000&delay=0&repeatCount=5")
        .set_body(KNOWN_BODY.to_string())
        .set_header(PASSWORD_HEADER, Value::String(PASSWORD_VALUE.to_string()))
        .set_header(
            AUTHORIZATION_HEADER,
            Value::String(AUTHORIZATION_VALUE.to_string()),
        )
        .set_header(API_KEY_HEADER, Value::String(API_KEY_VALUE.to_string()))
        .set_header(TRACE_HEADER, Value::String(TRACE_VALUE.to_string()))
        .to(producer_uri)
        .to("mock:sent")
        .route_id("jms-credentials-producer")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("sent").unwrap();
    wait_until(
        "credential producer route delivery",
        Duration::from_secs(35),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    let message = wait_for_browsed_message(broker).await;
    assert_credential_contract(&message, "activemq");

    eprintln!(
        "PROOF 5.5 [activemq]: trace_id_property_present=true \
         credential_names_present=0 credential_values_present=0 queue={CREDENTIAL_QUEUE}"
    );
}

/// Focused synthetic-data cases proving the browse-property assertions are
/// discriminating: the positive requires the exact value, and the negatives
/// reject a credential name (normalized) or value. Test-only; no production
/// code changes.
#[cfg(test)]
mod browse_assertions {
    use super::*;
    use serde_json::json;

    fn browsed(properties_text: &str) -> BrowsedMessage {
        BrowsedMessage::from_jolokia(&json!({ "PropertiesText": properties_text }))
    }

    #[test]
    fn trace_property_positive_requires_exact_value() {
        let message = browsed(&format!("{{traceId={TRACE_VALUE}}}"));
        assert_eq!(
            message.property_value(TRACE_HEADER).as_deref(),
            Some(TRACE_VALUE)
        );
        let other = browsed("{traceId=a-different-value}");
        assert_ne!(
            other.property_value(TRACE_HEADER).as_deref(),
            Some(TRACE_VALUE),
            "a different trace value must not satisfy the positive assertion"
        );
    }

    #[test]
    fn credential_name_is_detected() {
        let message = browsed(&format!("{{password={PASSWORD_VALUE}}}"));
        assert!(
            message.has_property_name(PASSWORD_HEADER),
            "a leaked credential name must be detected"
        );
        let clean = browsed(&format!("{{traceId={TRACE_VALUE}}}"));
        assert!(!clean.has_property_name(PASSWORD_HEADER));
    }

    #[test]
    fn credential_name_normalizes_separator_variants() {
        for variant in ["API_KEY", "api-key", "api.key", "ApiKey"] {
            let message = browsed(&format!("{{{variant}={API_KEY_VALUE}}}"));
            assert!(
                message.has_property_name(API_KEY_HEADER),
                "normalized variant {variant} must be detected as the api-key credential"
            );
        }
    }

    #[test]
    fn credential_value_is_detected_in_any_field() {
        let message = BrowsedMessage::from_jolokia(&json!({
            "PropertiesText": format!("{{traceId={TRACE_VALUE}}}"),
            "Text": AUTHORIZATION_VALUE,
        }));
        assert!(
            message.any_field_contains(AUTHORIZATION_VALUE),
            "a credential value in any broker field must be detected"
        );
    }

    #[test]
    fn absent_credentials_and_extra_fields_are_clean() {
        let message = BrowsedMessage::from_jolokia(&json!({
            "PropertiesText": format!("{{traceId={TRACE_VALUE}, region=eu}}"),
            "JMSMessageID": "ID:host-1:1:1:1:1",
            "Text": KNOWN_BODY,
        }));
        assert_eq!(
            message.property_value(TRACE_HEADER).as_deref(),
            Some(TRACE_VALUE)
        );
        for (name, _) in CREDENTIALS {
            assert!(!message.has_property_name(name));
        }
        for (_, value) in CREDENTIALS {
            assert!(
                !message.any_field_contains(value),
                "sentinel must be absent from a clean message"
            );
        }
    }

    #[test]
    fn empty_and_null_property_text_yield_no_properties() {
        for text in ["", "null", "{}"] {
            let message = browsed(text);
            assert_eq!(message.property_value(TRACE_HEADER), None, "text {text:?}");
            assert!(!message.has_property_name(TRACE_HEADER));
        }
    }

    #[test]
    fn value_mismatch_diagnostic_is_value_free() {
        let diagnostic = trace_present_diagnostic("activemq");
        assert!(
            diagnostic.contains(TRACE_HEADER),
            "the diagnostic must name the property"
        );
        for (_, value) in CREDENTIALS {
            assert!(
                !diagnostic.contains(value),
                "the value-mismatch diagnostic must not embed a broker value"
            );
        }
        assert!(
            !diagnostic.contains(TRACE_VALUE),
            "the diagnostic must not embed the expected value either"
        );
    }

    #[test]
    fn browse_parse_error_omits_response_body() {
        // A malformed reply whose body carries a credential sentinel.
        let body = format!("<not-json>{PASSWORD_VALUE}</not-json>");
        let error = serde_json::from_str::<serde_json::Value>(&body)
            .expect_err("synthetic malformed body must fail to parse");
        let message = safe_json_parse_error("http://127.0.0.1:1/api/jolokia/", "200 OK", &error);
        assert!(
            !message.contains(PASSWORD_VALUE),
            "the browse parse error must not echo the response body"
        );
        assert!(
            message.contains("syntax"),
            "the browse parse error must still report the serde error class"
        );
        assert!(
            message.contains("200 OK"),
            "the browse parse error must report the HTTP status"
        );
    }
}
