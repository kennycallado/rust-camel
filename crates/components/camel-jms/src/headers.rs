use std::collections::HashMap;

use camel_component_api::{Exchange, Value};

use crate::config::{BROKER_SENSITIVE_KEY_SUBSTRINGS, normalize_header_name};
use crate::proto::JmsMessage;

/// Credential substrings appended to the shared broker-redaction list for the
/// outbound send-header denylist (jms-bridge-080 task 1.2). Kept in normalized
/// form (lowercase, no separators) so a separator variant such as `api-key`
/// still matches after key normalization.
const DENIED_CREDENTIAL_SUBSTRINGS: &[&str] = &["apikey", "authorization", "bearer", "principal"];

pub fn apply_jms_headers(exchange: &mut Exchange, msg: &JmsMessage) {
    if !msg.message_id.is_empty() {
        exchange
            .input
            .set_header("JMSMessageID", Value::String(msg.message_id.clone()));
    }
    if !msg.correlation_id.is_empty() {
        exchange.input.set_header(
            "JMSCorrelationID",
            Value::String(msg.correlation_id.clone()),
        );
    }
    if msg.timestamp != 0 {
        exchange
            .input
            .set_header("JMSTimestamp", Value::String(msg.timestamp.to_string()));
    }
    if !msg.destination.is_empty() {
        exchange
            .input
            .set_header("JMSDestination", Value::String(msg.destination.clone()));
    }
    if !msg.content_type.is_empty() {
        exchange
            .input
            .set_header("Content-Type", Value::String(msg.content_type.clone()));
    }
    for (k, v) in &msg.headers {
        if !is_internal_jms_header(k) {
            exchange.input.set_header(k, Value::String(v.clone()));
        }
    }
}

/// Extract the outbound JMS property map from an exchange.
///
/// Deny first, then allowlist: a header is forwarded only when it is neither
/// a reserved/framework/credential name (`is_denied_send_header`) nor absent
/// from the normalized `allowlist`. An empty allowlist forwards nothing. Only
/// string values are forwarded. `Content-Type` is excluded because it travels
/// as the dedicated `SendRequest.content_type` field.
pub fn extract_send_headers(exchange: &Exchange, allowlist: &[String]) -> HashMap<String, String> {
    let normalized_allowlist: Vec<String> =
        allowlist.iter().map(|k| normalize_header_name(k)).collect();
    exchange
        .input
        .headers
        .iter()
        .filter_map(|(k, v)| {
            if k == "Content-Type" || is_denied_send_header(k).is_some() {
                return None;
            }
            let normalized_key = normalize_header_name(k);
            if !normalized_allowlist
                .iter()
                .any(|allowed| allowed == &normalized_key)
            {
                return None;
            }
            v.as_str().map(|s| (k.to_string(), s.to_string()))
        })
        .collect()
}

/// Returns the denial reason when a header name must never reach the outbound
/// JMS property map, regardless of the allowlist.
///
/// The name is normalized (casefold, strip `-`, `_`, `.`) before matching so
/// separator and case variants such as `api-key`, `API_KEY`, and `Api_Key`
/// all collide. Denied: the `jms` provider-control namespace (`JMS`, `JMSX`,
/// `JMS_` all collapse to the `jms` prefix), the `camel`/`xcamel` framework
/// namespaces, and any name containing a credential substring from the shared
/// `BROKER_SENSITIVE_KEY_SUBSTRINGS` list plus `apikey`, `authorization`,
/// `bearer`, and `principal`.
pub(crate) fn is_denied_send_header(key: &str) -> Option<&'static str> {
    let normalized = normalize_header_name(key);
    if normalized.starts_with("jms") {
        return Some("reserved JMS provider-control name");
    }
    if normalized.starts_with("xcamel") {
        return Some("xcamel framework namespace");
    }
    if normalized.starts_with("camel") {
        return Some("camel framework namespace");
    }
    if BROKER_SENSITIVE_KEY_SUBSTRINGS
        .iter()
        .chain(DENIED_CREDENTIAL_SUBSTRINGS)
        .any(|needle| normalized.contains(needle))
    {
        return Some("credential substring");
    }
    None
}

fn is_internal_jms_header(key: &str) -> bool {
    matches!(
        key,
        "JMSMessageID"
            | "JMSTimestamp"
            | "JMSDestination"
            | "JMSExpiration"
            | "JMSDeliveryMode"
            | "JMSPriority"
            // Content-Type travels as a dedicated field in SendRequest, not as
            // a JMS property. Artemis rejects property names containing hyphens
            // (AMQ139012), so we must not forward it as a header.
            | "Content-Type"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::{Body, Message};

    fn make_msg(
        message_id: &str,
        correlation_id: &str,
        timestamp: i64,
        content_type: &str,
    ) -> JmsMessage {
        JmsMessage {
            message_id: message_id.to_string(),
            correlation_id: correlation_id.to_string(),
            timestamp,
            destination: "queue:orders".to_string(),
            body: vec![],
            headers: Default::default(),
            content_type: content_type.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn applies_message_id_header() {
        let mut ex = Exchange::new(Message::new(Body::Empty));
        let msg = make_msg("ID:123", "", 0, "");
        apply_jms_headers(&mut ex, &msg);
        assert_eq!(
            ex.input.header("JMSMessageID").unwrap().as_str(),
            Some("ID:123")
        );
    }

    #[test]
    fn applies_correlation_id_header() {
        let mut ex = Exchange::new(Message::new(Body::Empty));
        let msg = make_msg("ID:123", "CORR:456", 0, "");
        apply_jms_headers(&mut ex, &msg);
        assert_eq!(
            ex.input.header("JMSCorrelationID").unwrap().as_str(),
            Some("CORR:456")
        );
    }

    #[test]
    fn applies_content_type_header() {
        let mut ex = Exchange::new(Message::new(Body::Empty));
        let msg = make_msg("ID:1", "", 0, "application/json");
        apply_jms_headers(&mut ex, &msg);
        assert_eq!(
            ex.input.header("Content-Type").unwrap().as_str(),
            Some("application/json")
        );
    }

    #[test]
    fn empty_correlation_id_not_set() {
        let mut ex = Exchange::new(Message::new(Body::Empty));
        let msg = make_msg("ID:1", "", 0, "");
        apply_jms_headers(&mut ex, &msg);
        assert!(ex.input.header("JMSCorrelationID").is_none());
    }

    #[test]
    fn internal_headers_excluded_from_send() {
        let mut ex = Exchange::new(Message::new(Body::Empty));
        ex.input
            .set_header("JMSMessageID", Value::String("ID:old".to_string()));
        ex.input
            .set_header("x-custom", Value::String("val".to_string()));
        // Explicit allowlist naming both; the reserved name is still denied and
        // the business name passes.
        let allowlist = vec!["JMSMessageID".to_string(), "x-custom".to_string()];
        let headers = extract_send_headers(&ex, &allowlist);
        assert!(!headers.contains_key("JMSMessageID"));
        assert_eq!(headers.get("x-custom"), Some(&"val".to_string()));
    }

    #[test]
    fn extract_send_headers_empty_allowlist_drops_user_headers() {
        let mut ex = Exchange::new(Message::new(Body::Empty));
        ex.input
            .set_header("x-custom", Value::String("val".to_string()));
        let headers = extract_send_headers(&ex, &[]);
        assert!(
            headers.is_empty(),
            "an empty allowlist must forward no user header: {headers:?}"
        );
    }

    #[test]
    fn extract_send_headers_forwards_listed_business_header() {
        let mut ex = Exchange::new(Message::new(Body::Empty));
        ex.input
            .set_header("customerId", Value::String("42".to_string()));
        let allowlist = vec!["customerId".to_string()];
        let headers = extract_send_headers(&ex, &allowlist);
        assert_eq!(headers.get("customerId"), Some(&"42".to_string()));
    }

    #[test]
    fn extract_send_headers_denies_reserved_and_framework_names() {
        let mut ex = Exchange::new(Message::new(Body::Empty));
        let names = [
            "JMSMessageID",
            "JMSXDeliveryCount",
            "CamelStop",
            "x-camel-leader-epoch",
        ];
        for name in names {
            ex.input.set_header(name, Value::String("v".to_string()));
        }
        let allowlist: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        let headers = extract_send_headers(&ex, &allowlist);
        assert!(headers.is_empty(), "denied names passed: {headers:?}");
    }

    #[test]
    fn extract_send_headers_denies_credential_variants() {
        let mut ex = Exchange::new(Message::new(Body::Empty));
        let names = ["password", "Authorization", "api-key", "API_KEY"];
        for name in names {
            ex.input
                .set_header(name, Value::String("secret".to_string()));
        }
        let allowlist: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        let headers = extract_send_headers(&ex, &allowlist);
        assert!(headers.is_empty(), "credentials passed: {headers:?}");
    }

    #[test]
    fn inbound_apply_jms_headers_unchanged() {
        let mut ex = Exchange::new(Message::new(Body::Empty));
        let mut msg = make_msg("ID:9", "", 0, "application/json");
        msg.headers
            .insert("customerId".to_string(), "42".to_string());
        apply_jms_headers(&mut ex, &msg);
        assert_eq!(
            ex.input.header("Content-Type").unwrap().as_str(),
            Some("application/json")
        );
        assert_eq!(ex.input.header("customerId").unwrap().as_str(), Some("42"));
    }

    /// Shared cross-language corpus for jms-bridge-080 task 1.4. The Java
    /// `HeaderPolicyParityTest` loads the same file from the test classpath;
    /// `include_str!` embeds it at compile time so the Rust and Java
    /// classifiers can never drift apart. Path is relative to this file:
    /// `crates/components/camel-jms/src` -> repo root -> `bridges/jms/...`.
    const HEADER_POLICY_CORPUS: &str =
        include_str!("../../../../bridges/jms/src/test/resources/header-policy-corpus.tsv");

    struct CorpusRow {
        name: &'static str,
        denied: bool,
    }

    fn parse_header_policy_corpus(raw: &'static str) -> Vec<CorpusRow> {
        raw.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let (name, decision) = line
                    .split_once('\t')
                    .unwrap_or_else(|| panic!("corpus row is not tab-separated: {line:?}"));
                let denied = match decision {
                    "denied" => true,
                    "allowed" => false,
                    other => panic!("corpus decision must be denied|allowed: {other:?}"),
                };
                CorpusRow { name, denied }
            })
            .collect()
    }

    fn exchange_with_header(name: &str, value: &str) -> Exchange {
        let mut ex = Exchange::new(Message::new(Body::Empty));
        ex.input.set_header(name, Value::String(value.to_string()));
        ex
    }

    /// Parity with the Java `HeaderPolicyParityTest`: the same corpus decides
    /// `is_denied_send_header`, the allowlist gate, and the empty-allowlist
    /// rule on both sides.
    #[test]
    fn rust_header_policy_matches_shared_corpus() {
        let rows = parse_header_policy_corpus(HEADER_POLICY_CORPUS);
        assert!(!rows.is_empty(), "shared corpus must not be empty");

        for row in &rows {
            let actual = is_denied_send_header(row.name).is_some();
            assert_eq!(
                actual, row.denied,
                "corpus mismatch for {:?}: expected denied={}, got denied={actual}",
                row.name, row.denied
            );
        }

        // Allowlist parity: an allowlist naming every allowed row forwards a
        // name exactly when the corpus marks it allowed; an unlisted (but not
        // denied) name does not pass, and an empty allowlist forwards nothing.
        let allowlist: Vec<String> = rows
            .iter()
            .filter(|row| !row.denied)
            .map(|row| row.name.to_string())
            .collect();
        for row in &rows {
            let ex = exchange_with_header(row.name, "v");
            assert_eq!(
                extract_send_headers(&ex, &allowlist).contains_key(row.name),
                !row.denied,
                "allowlist mismatch for {:?}",
                row.name
            );
            let empty = extract_send_headers(&ex, &[]);
            assert!(
                empty.is_empty(),
                "an empty allowlist must forward nothing, got {empty:?}"
            );
        }

        let listed_only = vec!["customerId".to_string()];
        let listed = exchange_with_header("customerId", "42");
        assert_eq!(
            extract_send_headers(&listed, &listed_only).get("customerId"),
            Some(&"42".to_string())
        );
        let unlisted = exchange_with_header("region", "eu");
        assert!(extract_send_headers(&unlisted, &listed_only).is_empty());
    }
}
