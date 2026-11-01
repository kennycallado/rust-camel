//! Contract tests for the `Mime` stream split format and its mime-only options.

use camel_api::{StreamSplitConfig, StreamSplitFormat};

#[test]
fn mime_variant_serializes_snake_case() {
    assert_eq!(
        serde_json::to_value(StreamSplitFormat::Mime).unwrap(),
        serde_json::json!("mime")
    );
}

#[test]
fn mime_options_accepted() {
    let config = StreamSplitConfig {
        format: StreamSplitFormat::Mime,
        max_parts: Some(10),
        max_part_size: Some(4096),
        content_types: Some(vec!["text/plain".to_string(), "image/png".to_string()]),
        ..Default::default()
    };
    config.validate().expect("mime options must be accepted");
}

#[test]
fn mime_options_rejected_on_zip() {
    let config = StreamSplitConfig {
        format: StreamSplitFormat::Zip,
        max_parts: Some(5),
        ..Default::default()
    };
    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("max_parts"),
        "non-mime error must name max_parts, got: {err}"
    );
}

#[test]
fn mime_rejects_chunk_size() {
    let config = StreamSplitConfig {
        format: StreamSplitFormat::Mime,
        chunk_size: Some(1024),
        ..Default::default()
    };
    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("chunk_size"),
        "mime chunk_size error must name chunk_size, got: {err}"
    );
}

#[test]
fn mime_rejects_batch_size() {
    let config = StreamSplitConfig {
        format: StreamSplitFormat::Mime,
        batch_size: 2,
        ..Default::default()
    };
    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("batch_size"),
        "mime batch_size error must name batch_size, got: {err}"
    );
}

#[test]
fn zero_limits_rejected() {
    let zero_parts = StreamSplitConfig {
        format: StreamSplitFormat::Mime,
        max_parts: Some(0),
        ..Default::default()
    };
    let err = zero_parts.validate().unwrap_err();
    assert!(
        err.to_string().contains("max_parts"),
        "zero max_parts error must name max_parts, got: {err}"
    );

    let zero_part_size = StreamSplitConfig {
        format: StreamSplitFormat::Mime,
        max_part_size: Some(0),
        ..Default::default()
    };
    let err = zero_part_size.validate().unwrap_err();
    assert!(
        err.to_string().contains("max_part_size"),
        "zero max_part_size error must name max_part_size, got: {err}"
    );
}

#[test]
fn existing_config_deserializes_without_new_fields() {
    let json = serde_json::json!({
        "format": "lines",
        "max_record_bytes": 1024,
        "batch_size": 2,
        "chunk_size": null,
        "include_origin": true
    });

    let config: StreamSplitConfig =
        serde_json::from_value(json).expect("legacy config must deserialize");
    assert_eq!(config.max_parts, None);
    assert_eq!(config.max_part_size, None);
    assert_eq!(config.content_types, None);

    let value = serde_json::to_value(&config).unwrap();
    let obj = value.as_object().expect("config serializes to object");
    for key in ["max_parts", "max_part_size", "content_types"] {
        assert!(
            !obj.contains_key(key),
            "unset option must be skipped on serialize: {key}"
        );
    }
}
