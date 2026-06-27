// MQTT URI parsing: scheme, host, port, and query parameters.

use crate::config::{AckMode, MqttEndpointConfig, QosLevel};
use camel_api::CamelError;

/// Parse an MQTT URI into MqttEndpointConfig.
///
/// Handles:
/// - `#` in path (not treated as a URL fragment)
/// - `topics=` as comma-separated multi-value, including repeated `topics=` keys
/// - `%2C` decoded to a literal comma within a single filter
pub fn parse_mqtt_uri(uri: &str) -> Result<MqttEndpointConfig, CamelError> {
    let rest = uri
        .strip_prefix("mqtt://")
        .or_else(|| uri.strip_prefix("mqtts://"))
        .ok_or_else(|| CamelError::Config(format!("mqtt: invalid URI scheme: {uri}")))?;

    let (authority_path, query) = match rest.split_once('?') {
        Some((a, q)) => (a, Some(q)),
        None => (rest, None),
    };

    let (broker_name, raw_path) = match authority_path.split_once('/') {
        Some((b, p)) => (b, Some(p)),
        None => (authority_path, None),
    };

    if broker_name.is_empty() {
        return Err(CamelError::Config("mqtt: URI missing broker_name".into()));
    }

    let params = parse_query(query.unwrap_or(""));

    // topics take precedence over the path topic. Split the RAW value on ','
    // FIRST, then percent-decode each filter, so `%2C` survives as a literal
    // comma within a single filter (not a separator).
    let subscriptions = if !params.get_all("topics").is_empty() {
        let mut filters = Vec::new();
        for raw in params.get_all("topics") {
            for part in raw.split(',') {
                filters.push(percent_decode(part));
            }
        }
        filters
    } else if let Some(path) = raw_path {
        if path.is_empty() {
            vec![]
        } else {
            vec![path.to_string()]
        }
    } else {
        vec![]
    };

    // publish_topic = path only if non-empty and has no wildcards (producer URI).
    let publish_topic = raw_path
        .filter(|p| !p.is_empty() && !p.contains('+') && !p.contains('#'))
        .map(|p| p.to_string());

    let qos = match params.get("qos") {
        Some("0") => QosLevel::AtMostOnce,
        Some("1") | None => QosLevel::AtLeastOnce,
        Some("2") => QosLevel::ExactlyOnce,
        Some(v) => return Err(CamelError::Config(format!("mqtt: invalid qos={v}"))),
    };

    let ack_mode = match params.get("ackMode") {
        Some("manual") => AckMode::Manual,
        Some("auto") | None => AckMode::Auto,
        Some(v) => return Err(CamelError::Config(format!("mqtt: invalid ackMode={v}"))),
    };

    let clean_session = !matches!(params.get("cleanSession"), Some("false"));

    let retain = matches!(params.get("retain"), Some("true"));

    let keep_alive_secs = params
        .get("keepAliveSecs")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(60);

    let max_payload_bytes = params
        .get("maxPayloadBytes")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(256 * 1024);

    let client_id_override = params.get("clientId").map(percent_decode);

    Ok(MqttEndpointConfig {
        broker_name: broker_name.to_string(),
        subscriptions,
        publish_topic,
        qos,
        ack_mode,
        clean_session,
        retain,
        keep_alive_secs,
        max_payload_bytes,
        client_id_override,
        reconnect: None,
    })
}

/// Ordered query-parameter list. A `Vec` (not `HashMap`) preserves repeated keys.
struct QueryParams(Vec<(String, String)>);

impl QueryParams {
    /// Last occurrence wins (standard query semantics).
    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
    /// All occurrences, in order (for repeated `topics=` keys).
    fn get_all(&self, key: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }
}

fn parse_query(query: &str) -> QueryParams {
    let mut out = Vec::new();
    if query.is_empty() {
        return QueryParams(out);
    }
    for part in query.split('&') {
        if let Some((k, v)) = part.split_once('=') {
            // Store RAW (undecoded) values. Topics must split on ',' BEFORE
            // decoding so `%2C` survives as a literal comma within one filter.
            out.push((k.to_string(), v.to_string()));
        }
    }
    QueryParams(out)
}

/// Percent-decoder: decodes `%XX` byte sequences, then validates UTF-8.
///
/// Policy is deliberately lenient: a malformed escape (e.g. `%ZZ`) or a truncated
/// tail (e.g. `%4`) is left as-is, and a byte sequence that decodes to invalid
/// UTF-8 falls back to the original string. This is acceptable for MQTT topic
/// filters, which are arbitrary UTF-8-ish labels; a malformed URI is accepted
/// rather than rejected with a diagnostic. Scalar params that must be exact
/// (qos/ackMode) are validated explicitly after decoding.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_topic() {
        let cfg = parse_mqtt_uri("mqtt://local/sensors/temperature").unwrap();
        assert_eq!(cfg.broker_name, "local");
        assert_eq!(cfg.subscriptions, vec!["sensors/temperature"]);
    }

    #[test]
    fn parse_wildcard_hash() {
        let cfg = parse_mqtt_uri("mqtt://local/sensors/#").unwrap();
        assert_eq!(cfg.subscriptions, vec!["sensors/#"]);
    }

    #[test]
    fn parse_wildcard_plus() {
        let cfg = parse_mqtt_uri("mqtt://local/sensors/+/temperature").unwrap();
        assert_eq!(cfg.subscriptions, vec!["sensors/+/temperature"]);
    }

    #[test]
    fn parse_multi_topics_param() {
        let cfg = parse_mqtt_uri("mqtt://local?topics=sensors/temp,sensors/humidity").unwrap();
        assert_eq!(cfg.subscriptions, vec!["sensors/temp", "sensors/humidity"]);
    }

    #[test]
    fn parse_repeated_topics_keys() {
        let cfg =
            parse_mqtt_uri("mqtt://local?topics=sensors/temp&topics=sensors/humidity").unwrap();
        assert_eq!(cfg.subscriptions, vec!["sensors/temp", "sensors/humidity"]);
    }

    #[test]
    fn parse_qos_param() {
        let cfg = parse_mqtt_uri("mqtt://local/sensors/temp?qos=2").unwrap();
        assert_eq!(cfg.qos, QosLevel::ExactlyOnce);
    }

    #[test]
    fn parse_ack_mode_manual() {
        let cfg =
            parse_mqtt_uri("mqtt://local/sensors/temp?ackMode=manual&cleanSession=false").unwrap();
        assert_eq!(cfg.ack_mode, AckMode::Manual);
        assert!(!cfg.clean_session);
    }

    #[test]
    fn parse_client_id_override() {
        let cfg = parse_mqtt_uri("mqtt://local/sensors/temp?clientId=my-device-001").unwrap();
        assert_eq!(cfg.client_id_override, Some("my-device-001".to_string()));
    }

    #[test]
    fn parse_producer_uri_no_path() {
        let cfg = parse_mqtt_uri("mqtt://local?qos=1").unwrap();
        assert_eq!(cfg.broker_name, "local");
        assert!(cfg.subscriptions.is_empty());
        assert!(cfg.publish_topic.is_none());
    }

    #[test]
    fn parse_encoded_comma_in_topics() {
        let cfg = parse_mqtt_uri("mqtt://local?topics=sensors%2Ctest,sensors/temp").unwrap();
        // %2C decoded to literal comma in first filter; second is a normal separator
        assert_eq!(cfg.subscriptions[0], "sensors,test");
        assert_eq!(cfg.subscriptions[1], "sensors/temp");
    }

    #[test]
    fn parse_rejects_bad_scheme() {
        assert!(parse_mqtt_uri("tcp://local/sensors/temp").is_err());
    }

    #[test]
    fn parse_rejects_empty_broker() {
        // "mqtt:///sensors/temp" -> broker_name is empty
        assert!(parse_mqtt_uri("mqtt:///sensors/temp").is_err());
        // "mqtt://?topics=x" -> broker_name is empty
        assert!(parse_mqtt_uri("mqtt://?topics=x").is_err());
    }

    #[test]
    fn parse_rejects_invalid_qos() {
        assert!(parse_mqtt_uri("mqtt://local?topics=x&qos=9").is_err());
    }

    #[test]
    fn parse_rejects_invalid_ack_mode() {
        assert!(parse_mqtt_uri("mqtt://local?topics=x&ackMode=fast").is_err());
    }

    #[test]
    fn parse_defaults_when_only_broker_given() {
        let cfg = parse_mqtt_uri("mqtt://local?qos=1").unwrap();
        assert_eq!(cfg.broker_name, "local");
        assert!(cfg.subscriptions.is_empty());
        assert!(cfg.publish_topic.is_none());
        assert_eq!(cfg.qos, QosLevel::AtLeastOnce);
        assert_eq!(cfg.ack_mode, AckMode::Auto);
        assert!(cfg.clean_session);
        assert!(!cfg.retain);
        assert_eq!(cfg.keep_alive_secs, 60);
        assert_eq!(cfg.max_payload_bytes, 256 * 1024);
        assert!(cfg.client_id_override.is_none());
        assert!(cfg.reconnect.is_none());
    }
}
