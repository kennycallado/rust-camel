use super::*;
use crate::config::CxfPoolConfig;
use camel_component_api::NetworkRetryPolicy;
use camel_component_api::test_support::PanicRuntimeObservability;
fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    std::sync::Arc::new(PanicRuntimeObservability)
}

fn test_pool() -> Arc<CxfBridgePool> {
    let pool_config = CxfPoolConfig {
        profiles: vec![],
        max_bridges: 1,
        bridge_start_timeout_ms: 5_000,
        health_check_interval_ms: 5_000,
        bridge_cache_dir: None,
        version: "0.1.0".to_string(),
        bind_address: None,
        listener_tls: None,
        reconnect: NetworkRetryPolicy::default(),
    };
    Arc::new(CxfBridgePool::from_config(pool_config).unwrap())
}

/// Minimal consumer request for the SOAP-header mapping tests; callers
/// override the fields they need.
fn request(operation: &str, security_profile: &str) -> ConsumerRequest {
    ConsumerRequest {
        request_id: String::new(),
        operation: operation.to_string(),
        payload: b"<hello/>".to_vec(),
        headers: Default::default(),
        soap_action: String::new(),
        security_profile: security_profile.to_string(),
        signer: None,
        soap_headers: vec![],
    }
}

#[test]
fn consumer_new() {
    let pool = test_pool();
    let consumer = CxfConsumer::new(pool, "baleares".to_string(), test_rt());
    assert!(consumer.cancel_token.is_none());
    assert!(consumer.task_handle.is_none());
    assert_eq!(consumer.profile_name, "baleares");
}

#[test]
fn build_exchange_with_payload() {
    let req = ConsumerRequest {
        request_id: "req-1".to_string(),
        operation: "sayHello".to_string(),
        payload: b"<soap:Body><hello>world</hello></soap:Body>".to_vec(),
        headers: [("CustomHeader".to_string(), "custom-value".to_string())]
            .into_iter()
            .collect(),
        soap_action: "urn:sayHello".to_string(),
        security_profile: "baleares".to_string(),
        signer: None,
        soap_headers: vec![],
    };

    let exchange = build_exchange(&req).expect("request must assemble");

    assert!(matches!(exchange.input.body, Body::Text(_)));
    if let Body::Text(s) = &exchange.input.body {
        assert!(s.contains("<hello>world</hello>"));
    }
    assert_eq!(
        exchange
            .input
            .header("CxfRequestId")
            .and_then(|v| v.as_str()),
        Some("req-1")
    );
    assert_eq!(
        exchange
            .input
            .header("CxfOperation")
            .and_then(|v| v.as_str()),
        Some("sayHello")
    );
    assert_eq!(
        exchange
            .input
            .header("CxfSoapAction")
            .and_then(|v| v.as_str()),
        Some("urn:sayHello")
    );
    assert_eq!(
        exchange
            .input
            .header("CxfSecurityProfile")
            .and_then(|v| v.as_str()),
        Some("baleares")
    );
    assert_eq!(
        exchange
            .input
            .header("CustomHeader")
            .and_then(|v| v.as_str()),
        Some("custom-value")
    );
}

#[test]
fn build_exchange_empty_payload() {
    let req = ConsumerRequest {
        request_id: "req-2".to_string(),
        operation: String::new(),
        payload: vec![],
        headers: Default::default(),
        soap_action: String::new(),
        security_profile: String::new(),
        signer: None,
        soap_headers: vec![],
    };

    let exchange = build_exchange(&req).expect("request must assemble");

    assert!(matches!(exchange.input.body, Body::Empty));
    assert_eq!(
        exchange
            .input
            .header("CxfRequestId")
            .and_then(|v| v.as_str()),
        Some("req-2")
    );
}

#[tokio::test]
async fn stop_without_start_is_noop() {
    let pool = test_pool();
    let mut consumer = CxfConsumer::new(pool, "test".to_string(), test_rt());
    assert!(consumer.stop().await.is_ok());
}

#[tokio::test]
async fn test_consumer_stop_cleans_up() {
    let pool = test_pool();
    let mut consumer = CxfConsumer::new(pool, "test".to_string(), test_rt());

    // Simulate the internal state that start() would set up,
    // but without actually trying to connect to a bridge.
    let cancel = CancellationToken::new();
    consumer.cancel_token = Some(cancel.clone());

    // Spawn a trivial task that respects cancellation.
    let handle = tokio::spawn(async move {
        tokio::select! {
            _ = cancel.cancelled() => {}
            _ = tokio::time::sleep(Duration::from_secs(300)) => {}
        }
        Ok(())
    });
    consumer.task_handle = Some(handle);

    // Stop must cancel the token and await the handle within timeout.
    let stop_result = consumer.stop().await;
    assert!(stop_result.is_ok(), "stop should succeed: {stop_result:?}");
    assert!(
        consumer.task_handle.is_none(),
        "handle should be taken after stop"
    );
    assert!(
        consumer.cancel_token.is_none(),
        "cancel should be taken after stop"
    );
}

#[test]
fn build_response_body_from_text() {
    let exchange = Exchange::new(Message::new(Body::Text(
        "<response>ok</response>".to_string(),
    )));
    let body = build_response_body(&exchange).unwrap();
    assert_eq!(body, b"<response>ok</response>");
}

#[test]
fn build_response_body_from_empty() {
    let exchange = Exchange::new(Message::new(Body::Empty));
    let body = build_response_body(&exchange).unwrap();
    assert!(body.is_empty());
}

#[test]
fn build_response_body_from_json() {
    let json = serde_json::json!({"status": "ok", "code": 200});
    let exchange = Exchange::new(Message::new(Body::Json(json.clone())));
    let body = build_response_body(&exchange).unwrap();
    let expected = serde_json::to_vec(&json).unwrap();
    assert_eq!(body, expected);
}

#[test]
fn build_response_body_from_xml() {
    let xml = "<soap:Envelope><soap:Body><hello>world</hello></soap:Body></soap:Envelope>";
    let exchange = Exchange::new(Message::new(Body::Xml(xml.to_string())));
    let body = build_response_body(&exchange).unwrap();
    assert_eq!(body, xml.as_bytes());
}

#[test]
fn build_response_body_from_bytes() {
    let raw = b"binary\x00data\xff".to_vec();
    let exchange = Exchange::new(Message::new(Body::Bytes(bytes::Bytes::from(raw.clone()))));
    let body = build_response_body(&exchange).unwrap();
    assert_eq!(body, raw);
}

#[test]
fn build_response_body_stream_returns_error() {
    use camel_component_api::StreamBody;
    use futures::stream;
    use tokio::sync::Mutex;

    let s = stream::empty::<Result<bytes::Bytes, CamelError>>();
    let body = Body::Stream(StreamBody {
        stream: Arc::new(Mutex::new(Some(Box::pin(s)))),
        metadata: Default::default(),
    });
    let exchange = Exchange::new(Message::new(body));
    let result = build_response_body(&exchange);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("streaming body not supported"),
        "got: {err}"
    );
}

#[test]
fn reconnect_policy_delay_for_grows_exponentially() {
    let p = NetworkRetryPolicy {
        initial_delay: Duration::from_millis(500),
        multiplier: 2.0,
        max_delay: Duration::from_secs(30),
        jitter_factor: 0.0,
        ..NetworkRetryPolicy::default()
    };
    let d0 = p.delay_for(0);
    let d1 = p.delay_for(1);
    let d2 = p.delay_for(2);
    let d3 = p.delay_for(3);
    assert!(d0 < d1, "{d0:?} should be < {d1:?}");
    assert!(d1 < d2, "{d1:?} should be < {d2:?}");
    assert!(d2 < d3, "{d2:?} should be < {d3:?}");
    // Verify cap at 30s for large attempts
    assert_eq!(p.delay_for(10), Duration::from_secs(30));
    assert_eq!(p.delay_for(20), Duration::from_secs(30));
    assert_eq!(p.delay_for(100), Duration::from_secs(30));
}

#[test]
fn build_exchange_preserves_operation_header() {
    let req = ConsumerRequest {
        request_id: String::new(),
        operation: "sayHello".to_string(),
        payload: b"<hello/>".to_vec(),
        headers: Default::default(),
        soap_action: String::new(),
        security_profile: String::new(),
        signer: None,
        soap_headers: vec![],
    };
    let exchange = build_exchange(&req).expect("request must assemble");
    assert_eq!(
        exchange
            .input
            .header("CxfOperation")
            .and_then(|v| v.as_str()),
        Some("sayHello")
    );
}

#[test]
fn is_bridge_transport_error_detects_transport() {
    assert!(is_bridge_transport_error(&CamelError::ProcessorError(
        "transport error".to_string()
    )));
    assert!(is_bridge_transport_error(&CamelError::ProcessorError(
        "connection refused".to_string()
    )));
    assert!(is_bridge_transport_error(&CamelError::ProcessorError(
        "UNAVAILABLE".to_string()
    )));
    assert!(!is_bridge_transport_error(&CamelError::ProcessorError(
        "SOAP fault".to_string()
    )));
}

#[test]
fn reconnect_policy_delay_for_exact_values() {
    let p = NetworkRetryPolicy {
        initial_delay: Duration::from_millis(500),
        multiplier: 2.0,
        max_delay: Duration::from_secs(30),
        jitter_factor: 0.0,
        ..NetworkRetryPolicy::default()
    };
    assert_eq!(p.delay_for(0), Duration::from_millis(500));
    assert_eq!(p.delay_for(1), Duration::from_secs(1));
    assert_eq!(p.delay_for(2), Duration::from_secs(2));
    assert_eq!(p.delay_for(10), Duration::from_secs(30));
}

fn test_signer() -> crate::proto::SignerIdentity {
    crate::proto::SignerIdentity {
        subject: "CN=test-alice,O=RustCamel Test".to_string(),
        issuer: "CN=test-alice,O=RustCamel Test".to_string(),
        serial: "0a1b2c".to_string(),
        fingerprint_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            .to_string(),
    }
}

#[test]
fn signer_is_stored_as_dedicated_properties_only() {
    use crate::signer::{SIGNER_FINGERPRINT, SIGNER_ISSUER, SIGNER_SERIAL, SIGNER_SUBJECT};

    let req = ConsumerRequest {
        request_id: "req-signer".to_string(),
        operation: "sayHello".to_string(),
        payload: b"<hello/>".to_vec(),
        headers: Default::default(),
        soap_action: String::new(),
        security_profile: "esap".to_string(),
        signer: Some(test_signer()),
        soap_headers: vec![],
    };

    let exchange = build_exchange(&req).expect("valid signer must assemble");

    assert_eq!(
        exchange.property(SIGNER_SUBJECT).and_then(|v| v.as_str()),
        Some("CN=test-alice,O=RustCamel Test")
    );
    assert_eq!(
        exchange.property(SIGNER_ISSUER).and_then(|v| v.as_str()),
        Some("CN=test-alice,O=RustCamel Test")
    );
    assert_eq!(
        exchange.property(SIGNER_SERIAL).and_then(|v| v.as_str()),
        Some("0a1b2c")
    );
    assert_eq!(
        exchange
            .property(SIGNER_FINGERPRINT)
            .and_then(|v| v.as_str()),
        Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
    );

    // The four dedicated keys are the whole property surface: no
    // `camel.auth.*` key (so `principal_from_exchange` reports no
    // authenticated principal) and no claims/roles/scopes.
    assert!(
        camel_api::security_policy::principal_from_exchange(&exchange).is_none(),
        "signer metadata must never be promoted into a Principal"
    );
    for key in exchange.properties.keys() {
        assert!(
            !key.starts_with("camel.auth."),
            "no camel.auth.* key may be written, got: {key}"
        );
        assert!(
            !matches!(
                key.as_str(),
                "claims"
                    | "roles"
                    | "scopes"
                    | "camel.auth.claims"
                    | "camel.auth.roles"
                    | "camel.auth.scopes"
            ),
            "no claims/roles/scopes may be written, got: {key}"
        );
    }
    assert_eq!(
        exchange.properties.len(),
        4,
        "only the four camel.cxf.signer.* properties may be written, got: {:?}",
        exchange.properties.keys().collect::<Vec<_>>()
    );
}

#[test]
fn spoofed_signer_header_stays_header() {
    use crate::signer::{SIGNER_FINGERPRINT, SIGNER_SUBJECT};

    let req = ConsumerRequest {
        request_id: "req-spoof".to_string(),
        operation: "sayHello".to_string(),
        payload: b"<hello/>".to_vec(),
        headers: [
            (SIGNER_SUBJECT.to_string(), "CN=attacker".to_string()),
            (
                SIGNER_FINGERPRINT.to_string(),
                "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
        soap_action: String::new(),
        security_profile: String::new(),
        signer: None,
        soap_headers: vec![],
    };

    let exchange = build_exchange(&req).expect("request must assemble");

    // A client-supplied camel.cxf.signer.* name is an ordinary message
    // header: preserved as a header, never written as a property.
    assert_eq!(
        exchange
            .input
            .header(SIGNER_SUBJECT)
            .and_then(|v| v.as_str()),
        Some("CN=attacker")
    );
    assert_eq!(
        exchange
            .input
            .header(SIGNER_FINGERPRINT)
            .and_then(|v| v.as_str()),
        Some("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
    );
    assert!(
        exchange.property(SIGNER_SUBJECT).is_none(),
        "spoofed header must not become a property"
    );
    assert!(
        exchange.property(SIGNER_FINGERPRINT).is_none(),
        "spoofed header must not become a property"
    );
    assert!(
        exchange.properties.is_empty(),
        "no signer property may be set from headers, got: {:?}",
        exchange.properties.keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn failed_start_releases_profile_registration() {
    use crate::pool::BridgeSlot;
    let pool = test_pool();
    let slot =
        BridgeSlot::new_ready_for_test(Channel::from_static("http://127.0.0.1:1").connect_lazy());
    slot.state_tx
        .send_replace(BridgeState::Degraded("test".into()));
    pool.insert_slot_for_test(CxfBridgePool::slot_key(), slot);

    let mut consumer = CxfConsumer::new(Arc::clone(&pool), "v11".to_string(), test_rt());
    let (tx, _rx) = mpsc::channel(1);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "r".to_string());
    let res = consumer.start(ctx).await;
    let err = res.expect_err("degraded bridge must fail start");
    assert!(
        err.to_string().contains("CXF bridge not available"),
        "{err}"
    );
    assert!(!pool.consumer_profile_active_for_test("v11"));
}

#[test]
fn http_spoof_cannot_shadow_soap_or_cxf_headers() {
    let proto_xml = "<rh:requestHeaderType xmlns:rh=\"http://tempuri.org/commonSchema\"><rh:organisation>28071</rh:organisation></rh:requestHeaderType>";
    let mut req = request("bridge-op", "bridge-profile");
    req.request_id = "bridge-request-id".to_string();
    req.soap_action = "bridge-soap-action".to_string();
    req.headers = [
        (
            "cxfsoapheader.organisation".to_string(),
            "attacker".to_string(),
        ),
        (
            "cxfsoapheader.requestheadertype".to_string(),
            "spoofed".to_string(),
        ),
        ("cxfoperation".to_string(), "spoofed".to_string()),
        ("cxfsecurityprofile".to_string(), "spoofed".to_string()),
        ("cxfrequestid".to_string(), "spoofed".to_string()),
        ("cxfsoapaction".to_string(), "spoofed".to_string()),
        ("CxfOperation".to_string(), "spoofed".to_string()),
        ("CxfSecurityProfile".to_string(), "spoofed".to_string()),
        ("CxfRequestId".to_string(), "spoofed".to_string()),
        ("CxfSoapAction".to_string(), "spoofed".to_string()),
    ]
    .into_iter()
    .collect();
    req.soap_headers = vec![crate::proto::SoapHeader {
        ns: "http://tempuri.org/commonSchema".to_string(),
        local_name: "requestHeaderType".to_string(),
        xml: proto_xml.to_string(),
        signed: true,
    }];

    let exchange = build_exchange(&req).expect("request must assemble");

    assert_eq!(
        exchange
            .input
            .header_ic("CxfOperation")
            .and_then(|v| v.as_str()),
        Some("bridge-op")
    );
    assert_eq!(
        exchange
            .input
            .header_ic("CxfSecurityProfile")
            .and_then(|v| v.as_str()),
        Some("bridge-profile")
    );
    assert_eq!(
        exchange
            .input
            .header_ic("CxfRequestId")
            .and_then(|v| v.as_str()),
        Some("bridge-request-id")
    );
    assert_eq!(
        exchange
            .input
            .header_ic("CxfSoapAction")
            .and_then(|v| v.as_str()),
        Some("bridge-soap-action")
    );
    assert!(
        exchange
            .input
            .header_ic("cxfsoapheader.organisation")
            .is_none(),
        "an HTTP key must never smuggle cxfsoapheader.* into the exchange"
    );
    assert_eq!(
        exchange
            .input
            .header_ic("CxfSoapHeader.requestHeaderType")
            .and_then(|v| v.as_str()),
        Some(proto_xml)
    );
    assert_eq!(
        exchange
            .input
            .header_ic("CxfSoapHeaderSigned.requestHeaderType")
            .and_then(|v| v.as_str()),
        Some("true")
    );
}

#[test]
fn http_cannot_inject_soap_header_without_proto_field() {
    let mut req = request("bridge-op", "");
    req.request_id = "req-inject".to_string();
    req.headers = [(
        "cxfsoapheader.requestheadertype".to_string(),
        "spoofed".to_string(),
    )]
    .into_iter()
    .collect();

    let exchange = build_exchange(&req).expect("request must assemble");

    assert!(
        exchange
            .input
            .header_ic("CxfSoapHeader.requestHeaderType")
            .is_none(),
        "only the dedicated proto field may populate CxfSoapHeader.*"
    );
    assert!(
        exchange
            .input
            .header_ic("CxfSoapHeaderSigned.requestHeaderType")
            .is_none(),
        "only the dedicated proto field may populate CxfSoapHeaderSigned.*"
    );
}

#[test]
fn soap_headers_map_to_exchange_with_flag() {
    let mut req = request("bridge-op", "");
    req.request_id = "req-soap".to_string();
    req.soap_headers = vec![
        crate::proto::SoapHeader {
            ns: "http://tempuri.org/commonSchema".to_string(),
            local_name: "signedHeader".to_string(),
            xml: "<rh:signedHeader/>".to_string(),
            signed: true,
        },
        crate::proto::SoapHeader {
            ns: "http://tempuri.org/commonSchema".to_string(),
            local_name: "unsignedHeader".to_string(),
            xml: "<rh:unsignedHeader/>".to_string(),
            signed: false,
        },
    ];

    let exchange = build_exchange(&req).expect("request must assemble");

    assert_eq!(
        exchange
            .input
            .header("CxfSoapHeader.signedHeader")
            .and_then(|v| v.as_str()),
        Some("<rh:signedHeader/>")
    );
    assert_eq!(
        exchange
            .input
            .header("CxfSoapHeaderSigned.signedHeader")
            .and_then(|v| v.as_str()),
        Some("true")
    );
    assert_eq!(
        exchange
            .input
            .header("CxfSoapHeader.unsignedHeader")
            .and_then(|v| v.as_str()),
        Some("<rh:unsignedHeader/>")
    );
    assert_eq!(
        exchange
            .input
            .header("CxfSoapHeaderSigned.unsignedHeader")
            .and_then(|v| v.as_str()),
        Some("false")
    );
}

#[test]
fn empty_soap_header_local_name_is_skipped() {
    let mut req = request("bridge-op", "");
    req.request_id = "req-empty".to_string();
    req.soap_headers = vec![crate::proto::SoapHeader {
        ns: "http://tempuri.org/commonSchema".to_string(),
        local_name: String::new(),
        xml: "<rh:empty/>".to_string(),
        signed: true,
    }];

    let exchange = build_exchange(&req).expect("request must assemble");

    assert!(
        !exchange
            .input
            .headers
            .keys()
            .any(|k| k.to_ascii_lowercase().starts_with("cxfsoapheader.")),
        "an empty local name must not create a CxfSoapHeader.* key"
    );
    assert!(
        !exchange
            .input
            .headers
            .keys()
            .any(|k| k.to_ascii_lowercase().starts_with("cxfsoapheadersigned.")),
        "an empty local name must not create a CxfSoapHeaderSigned.* key"
    );
}

#[test]
fn ordinary_http_header_still_maps() {
    let mut req = request("", "");
    req.headers = [("CustomHeader".to_string(), "custom-value".to_string())]
        .into_iter()
        .collect();

    let exchange = build_exchange(&req).expect("request must assemble");

    assert_eq!(
        exchange
            .input
            .header("CustomHeader")
            .and_then(|v| v.as_str()),
        Some("custom-value")
    );
}
