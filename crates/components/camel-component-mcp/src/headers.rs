//! Header constants for the MCP component.

use std::collections::HashSet;

use http::{HeaderMap, HeaderValue};

pub const CAMEL_MCP_TOOL_CALL: &str = "CamelMcpToolCall";
pub const CAMEL_MCP_RESULT: &str = "CamelMcpResult";

/// Normalize repeated inbound HTTP headers according to their field semantics.
///
/// List-valued fields are comma-joined in first-seen order. Cookie fields use
/// the cookie-specific semicolon separator. Authentication credentials are
/// singular: the first value wins and a repeated credential is reported.
pub fn normalize_repeated(headers: &HeaderMap) -> HeaderMap {
    let mut normalized = HeaderMap::new();
    let mut warned = HashSet::new();

    for (name, value) in headers {
        let Some(existing) = normalized.get_mut(name) else {
            normalized.insert(name.clone(), value.clone());
            continue;
        };

        if matches!(name.as_str(), "authorization" | "proxy-authorization") {
            if warned.insert(name.clone()) {
                let header = if name.as_str() == "authorization" {
                    "Authorization"
                } else {
                    "Proxy-Authorization"
                };
                tracing::warn!(
                    header,
                    "repeated authentication header; keeping first value"
                );
            }
            continue;
        }

        let separator = if name.as_str() == "cookie" {
            "; "
        } else {
            ", "
        };
        let mut joined = Vec::with_capacity(existing.len() + separator.len() + value.len());
        joined.extend_from_slice(existing.as_bytes());
        joined.extend_from_slice(separator.as_bytes());
        joined.extend_from_slice(value.as_bytes());
        if let Ok(joined) = HeaderValue::from_bytes(&joined) {
            *existing = joined;
        }
    }

    normalized
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::normalize_repeated;
    use http::{HeaderMap, HeaderValue};
    use tracing_subscriber::layer::SubscriberExt;

    struct WarningLayer(Arc<Mutex<Vec<String>>>);

    impl<S> tracing_subscriber::Layer<S> for WarningLayer
    where
        S: tracing::Subscriber,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if *event.metadata().level() != tracing::Level::WARN {
                return;
            }
            let mut visitor = EventVisitor::default();
            event.record(&mut visitor);
            if let Ok(mut warnings) = self.0.lock() {
                warnings.push(visitor.message());
            }
        }
    }

    #[derive(Default)]
    struct EventVisitor(Vec<String>);

    impl EventVisitor {
        fn message(self) -> String {
            self.0.join(" ")
        }
    }

    impl tracing::field::Visit for EventVisitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.push(format!("{field}={value:?}"));
        }

        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0.push(format!("{field}={value}"));
        }
    }

    #[test]
    fn multiple_cookies_join_semicolon() {
        let mut headers = HeaderMap::new();
        headers.append("Cookie", HeaderValue::from_static("a=1"));
        headers.append("Cookie", HeaderValue::from_static("b=2"));

        let normalized = normalize_repeated(&headers);

        assert_eq!(
            normalized.get("cookie").map(HeaderValue::as_bytes),
            Some(b"a=1; b=2".as_slice())
        );
    }

    #[test]
    fn repeated_authorization_first_value_warns() {
        let mut headers = HeaderMap::new();
        headers.append("Authorization", HeaderValue::from_static("Bearer first"));
        headers.append("Authorization", HeaderValue::from_static("Bearer second"));

        let warnings = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(WarningLayer(warnings.clone()));
        let normalized =
            tracing::subscriber::with_default(subscriber, || normalize_repeated(&headers));

        assert_eq!(
            normalized.get("authorization").map(HeaderValue::as_bytes),
            Some(b"Bearer first".as_slice())
        );
        if let Ok(warnings) = warnings.lock() {
            assert_eq!(warnings.len(), 1);
            assert!(warnings[0].contains("Authorization"));
        } else {
            panic!("warning recorder lock poisoned");
        }
    }

    #[test]
    fn arbitrary_header_joins_comma() {
        let mut headers = HeaderMap::new();
        headers.append("X-Trace-Id", HeaderValue::from_static("t1"));
        headers.append("X-Trace-Id", HeaderValue::from_static("t2"));

        let normalized = normalize_repeated(&headers);

        assert_eq!(
            normalized.get("x-trace-id").map(HeaderValue::as_bytes),
            Some(b"t1, t2".as_slice())
        );
    }
}
