use camel_wasm_sdk::{GuestState, WasmBody, WasmError, WasmExchange, WasmMessage};

#[test]
fn wasm_body_all_variants_construct_and_match() {
    let bodies = vec![
        WasmBody::empty(),
        WasmBody::text("hello"),
        WasmBody::bytes(vec![1, 2, 3]),
        WasmBody::json("{}"),
        WasmBody::xml("<root/>"),
    ];

    let mut empty_count = 0;
    let mut text_count = 0;
    let mut bytes_count = 0;
    let mut json_count = 0;
    let mut xml_count = 0;

    for body in &bodies {
        match body {
            WasmBody::Empty => empty_count += 1,
            WasmBody::Text(_) => text_count += 1,
            WasmBody::Bytes(_) => bytes_count += 1,
            WasmBody::Json(_) => json_count += 1,
            WasmBody::Xml(_) => xml_count += 1,
        }
    }

    assert_eq!(empty_count, 1);
    assert_eq!(text_count, 1);
    assert_eq!(bytes_count, 1);
    assert_eq!(json_count, 1);
    assert_eq!(xml_count, 1);
}

#[test]
fn wasm_message_builder_pattern() {
    let msg = WasmMessage::default()
        .with_header("content-type", "application/json")
        .with_header("x-request-id", "abc-123")
        .with_body(WasmBody::json(r#"{"key":"value"}"#));

    assert_eq!(msg.get_header("content-type"), Some("application/json"));
    assert_eq!(msg.get_header("x-request-id"), Some("abc-123"));
    assert!(msg.body.is_json());
    assert_eq!(msg.body.as_json(), Some(r#"{"key":"value"}"#));
}

#[test]
fn wasm_message_empty_headers_access() {
    let msg = WasmMessage::new(WasmBody::text("data"));

    assert!(msg.headers.is_empty());
    assert_eq!(msg.get_header("anything"), None);
}

#[test]
fn wasm_exchange_in_only_lifecycle() {
    let input = WasmMessage::new(WasmBody::text("request"));
    let mut ex = WasmExchange::new(input);

    assert!(ex.pattern.is_in_only());
    assert!(ex.output.is_none());
    assert!(ex.properties.is_empty());
    assert!(ex.correlation_id.is_empty());

    ex.set_output_body(WasmBody::text("response"));
    assert!(ex.output.is_some());
    assert_eq!(ex.output_body().unwrap().as_text(), Some("response"));
}

#[test]
fn wasm_exchange_in_out_lifecycle() {
    let input = WasmMessage::new(WasmBody::json(r#"{"action":"query"}"#));
    let ex = WasmExchange::new_in_out(input);

    assert!(ex.pattern.is_in_out());
    assert!(ex.output.is_none());
    assert!(ex.body().is_json());
}

#[test]
fn wasm_exchange_properties_full_crud() {
    let mut ex = WasmExchange::default();

    ex.set_property("route-id", "route-1");
    ex.set_property("retry-count", "3");
    assert_eq!(ex.get_property("route-id"), Some("route-1"));
    assert_eq!(ex.get_property("retry-count"), Some("3"));

    ex.set_property("route-id", "route-2");
    assert_eq!(ex.get_property("route-id"), Some("route-2"));
    assert_eq!(ex.properties.len(), 2);

    ex.remove_property("retry-count");
    assert_eq!(ex.get_property("retry-count"), None);
    assert_eq!(ex.properties.len(), 1);
}

#[test]
fn wasm_exchange_no_output_initial() {
    let ex = WasmExchange::new(WasmMessage::default());
    assert!(ex.output.is_none());
    assert!(ex.output_body().is_none());
}

#[test]
fn wasm_pattern_set_on_exchange() {
    let in_only = WasmExchange::new(WasmMessage::default());
    assert!(in_only.pattern.is_in_only());
    assert!(!in_only.pattern.is_in_out());

    let in_out = WasmExchange::new_in_out(WasmMessage::default());
    assert!(in_out.pattern.is_in_out());
    assert!(!in_out.pattern.is_in_only());
}

#[test]
fn wasm_error_all_constructors() {
    let processor = WasmError::processor("failed");
    assert!(matches!(processor, WasmError::ProcessorError(ref s) if s == "failed"));

    let conversion = WasmError::type_conversion("bad cast");
    assert!(matches!(conversion, WasmError::TypeConversion(ref s) if s == "bad cast"));

    let io = WasmError::io("connection refused");
    assert!(matches!(io, WasmError::Io(ref s) if s == "connection refused"));

    let timeout = WasmError::timeout();
    assert!(matches!(timeout, WasmError::Timeout));
}

#[test]
fn round_trip_text_uppercase() {
    let ex = WasmExchange::new(WasmMessage::new(WasmBody::text("hello world")));

    let transformed = if let Some(text) = ex.body().as_text() {
        WasmExchange {
            input: WasmMessage::new(WasmBody::text(text.to_uppercase())),
            ..ex
        }
    } else {
        ex
    };

    assert_eq!(transformed.body().as_text(), Some("HELLO WORLD"));
}

#[test]
fn round_trip_json_field_extract() {
    let ex = WasmExchange::new(WasmMessage::new(WasmBody::json(
        r#"{"name":"test","value":42}"#,
    )));

    let json = ex.body().as_json().unwrap();
    assert!(json.contains("name"));
    assert!(json.contains("test"));

    let result = WasmExchange {
        input: WasmMessage::new(WasmBody::text(format!("processed: {}", json.len()))),
        ..ex
    };

    assert!(result.body().is_text());
    assert_eq!(result.body().as_text(), Some("processed: 26"));
}

#[test]
fn round_trip_preserve_headers() {
    let input = WasmMessage::default()
        .with_header("content-type", "text/plain")
        .with_header("x-trace-id", "trace-42")
        .with_body(WasmBody::text("original"));

    let ex = WasmExchange::new(input);
    let original_headers = ex.input.headers.clone();

    let result = WasmExchange {
        input: WasmMessage {
            body: WasmBody::text("transformed"),
            headers: original_headers,
        },
        ..ex
    };

    assert_eq!(result.body().as_text(), Some("transformed"));
    assert_eq!(result.input.get_header("content-type"), Some("text/plain"));
    assert_eq!(result.input.get_header("x-trace-id"), Some("trace-42"));
}

#[test]
fn guest_state_static_init() {
    static STATE: GuestState<String> = GuestState::new();

    let val = STATE.get_or_init(|| "initialized".to_string());
    assert_eq!(val, "initialized");

    let val2 = STATE.get_or_init(|| "should not run".to_string());
    assert_eq!(val2, "initialized");
    assert!(std::ptr::eq(val, val2));
}

#[test]
fn guest_state_get_none_before_init() {
    static STATE: GuestState<u64> = GuestState::new();

    assert!(STATE.get().is_none());

    STATE.get_or_init(|| 12345);
    assert_eq!(STATE.get(), Some(&12345));
}

#[test]
fn exchange_default_usable() {
    let mut ex = WasmExchange::default();

    assert!(ex.body().is_empty());
    assert!(ex.output.is_none());
    assert!(ex.pattern.is_in_only());

    ex.set_body(WasmBody::text("data"));
    assert!(ex.body().is_text());

    ex.set_property("key", "value");
    assert_eq!(ex.get_property("key"), Some("value"));
}

#[test]
fn message_multiple_headers_get_first() {
    let mut msg = WasmMessage::default();
    msg.set_header("accept", "text/html");
    msg.set_header("accept", "application/json");

    assert_eq!(msg.get_header("accept"), Some("application/json"));
    assert_eq!(msg.headers.len(), 1);
}

#[test]
fn exchange_correlation_id_roundtrip() {
    let input = WasmMessage::new(WasmBody::text("request"));
    let mut ex = WasmExchange::new(input);

    assert!(ex.correlation_id.is_empty());

    ex.correlation_id = "corr-abc-123".to_string();
    assert_eq!(ex.correlation_id, "corr-abc-123");

    let result = WasmExchange {
        input: WasmMessage::new(WasmBody::text("response")),
        correlation_id: ex.correlation_id.clone(),
        ..ex
    };
    assert_eq!(result.correlation_id, "corr-abc-123");
}
