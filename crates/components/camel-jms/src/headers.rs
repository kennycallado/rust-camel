use camel_component_api::{Exchange, Value};

use crate::proto::JmsMessage;

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

pub fn extract_send_headers(exchange: &Exchange) -> std::collections::HashMap<String, String> {
    exchange
        .input
        .headers
        .iter()
        .filter_map(|(k, v)| {
            if is_internal_jms_header(k) {
                return None;
            }
            v.as_str().map(|s| (k.to_string(), s.to_string()))
        })
        .collect()
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
        let headers = extract_send_headers(&ex);
        assert!(!headers.contains_key("JMSMessageID"));
        assert_eq!(headers.get("x-custom"), Some(&"val".to_string()));
    }
}
