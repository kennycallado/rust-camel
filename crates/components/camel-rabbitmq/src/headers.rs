//! Two-way mapping between camel headers and AMQP basic properties.
//!
//! This module is the single source of property mapping: reserved
//! basic-property names map onto [`lapin::BasicProperties`] in both
//! directions and every other header rides the free-form AMQP `FieldTable`
//! as a `LongString`. No other module duplicates this mapping.
//!
//! # Timestamps
//!
//! AMQP's `timestamp` basic property is a 64-bit count of Unix **seconds**
//! (lapin 4.12's `Timestamp` is `u64`, not a `chrono` type). Camel carries
//! epoch **milliseconds** as `i64` (the JMS precedent). Inbound therefore
//! multiplies seconds by 1000 (checked) and outbound divides milliseconds by
//! 1000. The plan's "lapin props are chrono-typed" note is a units/type
//! mismatch against the real protocol; no `chrono` dependency is needed.

use camel_api::Headers;
use lapin::BasicProperties;
use lapin::types::{AMQPValue, FieldTable, LongString, ShortString};
use serde_json::{Value, json};

/// Reserved basic-property names with a dedicated AMQP meaning.
///
/// They map onto [`BasicProperties`] and never travel as free-form AMQP
/// headers, so a free-form header can never bypass the reserved mapping.
pub(crate) const RESERVED_HEADERS: &[&str] = &[
    "contentType",
    "contentEncoding",
    "priority",
    "messageId",
    "correlationId",
    "replyTo",
    "expiration",
    "timestamp",
];

/// Header carrying the broker's redelivery flag.
pub(crate) const REDELIVERED_HEADER: &str = "rabbitmq.redelivered";

/// Convert one free-form AMQP value into a camel JSON value.
///
/// Free-form headers are written as `LongString` (plain text), so the inbound
/// direction is lossless for this component's own traffic; the scalar/array
/// arms keep externally published AMQP headers readable without inventing a
/// serialization framework.
fn amqp_to_json(value: &AMQPValue) -> Value {
    match value {
        AMQPValue::Boolean(v) => json!(v),
        AMQPValue::ShortShortInt(v) => json!(v),
        AMQPValue::ShortShortUInt(v) => json!(v),
        AMQPValue::ShortInt(v) => json!(v),
        AMQPValue::ShortUInt(v) => json!(v),
        AMQPValue::LongInt(v) => json!(v),
        AMQPValue::LongUInt(v) => json!(v),
        AMQPValue::LongLongInt(v) => json!(v),
        AMQPValue::Float(v) => json!(v),
        AMQPValue::Double(v) => json!(v),
        AMQPValue::Timestamp(v) => json!(v),
        AMQPValue::ShortString(v) => json!(v.as_str()),
        AMQPValue::LongString(v) => json!(String::from_utf8_lossy(v.as_bytes())),
        AMQPValue::ByteArray(v) => json!(String::from_utf8_lossy(v.as_slice())),
        AMQPValue::FieldArray(v) => Value::Array(v.as_slice().iter().map(amqp_to_json).collect()),
        AMQPValue::FieldTable(v) => Value::Object(
            v.inner()
                .iter()
                .map(|(key, value)| (key.as_str().to_string(), amqp_to_json(value)))
                .collect(),
        ),
        AMQPValue::DecimalValue(v) => json!({ "scale": v.scale, "value": v.value }),
        AMQPValue::Void => Value::Null,
    }
}

/// Map an inbound AMQP delivery onto camel headers.
///
/// `headers` is the free-form AMQP `FieldTable` (the delivery's
/// `BasicProperties.headers`), `props` the basic properties, and
/// `redelivered` the broker redelivery flag. Reserved names are skipped in the
/// free-form pass and then written from `props`, so a free-form entry can
/// never override a reserved property.
pub(crate) fn inbound(headers: &FieldTable, props: &BasicProperties, redelivered: bool) -> Headers {
    let mut out = Headers::new();

    for (name, value) in headers {
        if RESERVED_HEADERS.contains(&name.as_str()) {
            continue;
        }
        out.insert(name.as_str().to_string(), amqp_to_json(value));
    }

    if let Some(value) = props.content_type() {
        out.insert("contentType".to_string(), json!(value.as_str()));
    }
    if let Some(value) = props.content_encoding() {
        out.insert("contentEncoding".to_string(), json!(value.as_str()));
    }
    if let Some(value) = props.priority() {
        out.insert("priority".to_string(), json!(value));
    }
    if let Some(value) = props.message_id() {
        out.insert("messageId".to_string(), json!(value.as_str()));
    }
    if let Some(value) = props.correlation_id() {
        out.insert("correlationId".to_string(), json!(value.as_str()));
    }
    if let Some(value) = props.reply_to() {
        out.insert("replyTo".to_string(), json!(value.as_str()));
    }
    if let Some(value) = props.expiration() {
        out.insert("expiration".to_string(), json!(value.as_str()));
    }
    if let Some(seconds) = props.timestamp() {
        // AMQP timestamp is Unix seconds; camel carries milliseconds. Guard
        // both the u64->i64 narrowing and the x1000 multiply: an out-of-range
        // broker timestamp is dropped rather than panicking.
        if let Some(millis) = i64::try_from(*seconds)
            .ok()
            .and_then(|seconds| seconds.checked_mul(1000))
        {
            out.insert("timestamp".to_string(), json!(millis));
        }
    }

    out.insert(REDELIVERED_HEADER.to_string(), json!(redelivered));
    out
}

/// Map camel headers onto `(BasicProperties, free-form FieldTable)`.
///
/// The properties carry the reserved names; the table carries everything else
/// as a `LongString`. A reserved name is excluded from the table even when a
/// route also set it, so the free-form path cannot bypass the property
/// mapping. Malformed reserved values (non-string, out-of-range `priority`,
/// negative `timestamp`, or a >255-byte `ShortString`) are dropped without
/// panicking.
pub(crate) fn outbound(headers: &Headers) -> (BasicProperties, FieldTable) {
    let mut props = BasicProperties::default();
    let mut table = FieldTable::default();

    for (name, value) in headers {
        match name.as_str() {
            "contentType" => {
                if let Some(value) = short_string(value) {
                    props = props.with_content_type(value);
                }
            }
            "contentEncoding" => {
                if let Some(value) = short_string(value) {
                    props = props.with_content_encoding(value);
                }
            }
            "priority" => {
                if let Some(value) = value.as_u64().and_then(|value| u8::try_from(value).ok()) {
                    props = props.with_priority(value);
                }
            }
            "messageId" => {
                if let Some(value) = short_string(value) {
                    props = props.with_message_id(value);
                }
            }
            "correlationId" => {
                if let Some(value) = short_string(value) {
                    props = props.with_correlation_id(value);
                }
            }
            "replyTo" => {
                if let Some(value) = short_string(value) {
                    props = props.with_reply_to(value);
                }
            }
            "expiration" => {
                if let Some(value) = short_string(value) {
                    props = props.with_expiration(value);
                }
            }
            "timestamp" => {
                if let Some(millis) = value.as_i64()
                    && millis >= 0
                {
                    props = props.with_timestamp((millis / 1000) as u64);
                }
            }
            _ => {
                let Ok(name) = ShortString::try_new(name.as_str()) else {
                    continue;
                };
                let text = value
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| value.to_string());
                table.insert(name, AMQPValue::LongString(LongString::from(text)));
            }
        }
    }

    (props, table)
}

/// Read a camel value as a bounded [`ShortString`], or `None` when it is not a
/// string or exceeds the AMQP short-string limit.
fn short_string(value: &Value) -> Option<ShortString> {
    value
        .as_str()
        .and_then(|text| ShortString::try_new(text).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All eight reserved properties set on one `BasicProperties`.
    fn fully_populated_props() -> BasicProperties {
        BasicProperties::default()
            .with_content_type(ShortString::from("application/json"))
            .with_content_encoding(ShortString::from("utf-8"))
            .with_priority(5)
            .with_message_id(ShortString::from("m-1"))
            .with_correlation_id(ShortString::from("c-1"))
            .with_reply_to(ShortString::from("reply.q"))
            .with_expiration(ShortString::from("60000"))
            .with_timestamp(1_700_000_000)
    }

    #[test]
    fn inbound_maps_redelivered_flag() {
        let redelivered = inbound(&FieldTable::default(), &BasicProperties::default(), true);
        assert_eq!(
            redelivered.get(REDELIVERED_HEADER),
            Some(&json!(true)),
            "a redelivered delivery must carry rabbitmq.redelivered=true"
        );

        let first = inbound(&FieldTable::default(), &BasicProperties::default(), false);
        assert_eq!(
            first.get(REDELIVERED_HEADER),
            Some(&json!(false)),
            "the flag must be present (false) on a first delivery"
        );
    }

    #[test]
    fn inbound_maps_reserved_properties() {
        let headers = inbound(&FieldTable::default(), &fully_populated_props(), false);

        assert_eq!(headers.get("contentType"), Some(&json!("application/json")));
        assert_eq!(headers.get("contentEncoding"), Some(&json!("utf-8")));
        assert_eq!(
            headers.get("priority"),
            Some(&json!(5)),
            "priority must map as a number"
        );
        assert_eq!(headers.get("messageId"), Some(&json!("m-1")));
        assert_eq!(headers.get("correlationId"), Some(&json!("c-1")));
        assert_eq!(headers.get("replyTo"), Some(&json!("reply.q")));
        assert_eq!(headers.get("expiration"), Some(&json!("60000")));
        assert_eq!(
            headers.get("timestamp"),
            Some(&json!(1_700_000_000_000_i64))
        );

        // An out-of-range AMQP timestamp must be dropped, never panic.
        let huge = BasicProperties::default().with_timestamp(u64::MAX);
        let headers = inbound(&FieldTable::default(), &huge, false);
        assert_eq!(
            headers.get("timestamp"),
            None,
            "an unrepresentable timestamp must be dropped without panicking"
        );
    }

    #[test]
    fn inbound_maps_timestamp_as_millis() {
        // Real lapin 4.12: `TimeStamp`/`Timestamp` is `u64` Unix SECONDS.
        let props = BasicProperties::default().with_timestamp(1_700_000_000);
        let headers = inbound(&FieldTable::default(), &props, false);
        assert_eq!(
            headers.get("timestamp"),
            Some(&json!(1_700_000_000_000_i64)),
            "AMQP seconds must surface as i64 epoch milliseconds"
        );
    }

    #[test]
    fn outbound_round_trips_free_form() {
        let mut headers = Headers::new();
        headers.insert("x-custom".to_string(), json!("abc"));
        // A reserved name set by the route must NOT land in the free-form table:
        // reserved-property precedence cannot be bypassed via free-form.
        headers.insert("contentType".to_string(), json!("text/plain"));
        // Malformed reserved values are dropped without panicking.
        headers.insert("priority".to_string(), json!(999));
        headers.insert("timestamp".to_string(), json!(-1));

        let (props, table) = outbound(&headers);

        match table.inner().get("x-custom") {
            Some(AMQPValue::LongString(value)) => {
                assert_eq!(value.as_bytes(), b"abc", "x-custom must be a LongString")
            }
            other => panic!("expected LongString for x-custom, got {other:?}"),
        }
        assert!(
            !table.contains_key("contentType"),
            "reserved contentType must not ride the free-form table"
        );
        assert_eq!(
            props.content_type().as_ref().map(|value| value.as_str()),
            Some("text/plain"),
            "reserved contentType must map onto BasicProperties"
        );
        assert_eq!(
            props.priority(),
            &None,
            "an out-of-range priority must be dropped, not truncated"
        );
        assert_eq!(
            props.timestamp(),
            &None,
            "a negative timestamp must be dropped, not wrapped"
        );

        // Free-form round trip: outbound table -> inbound restores the header.
        let restored = inbound(&table, &props, false);
        assert_eq!(
            restored.get("x-custom"),
            Some(&json!("abc")),
            "inbound must restore the free-form header"
        );
        assert_eq!(
            restored.get("contentType"),
            Some(&json!("text/plain")),
            "inbound must restore the reserved property"
        );
    }
}
