//! Exchange-to-message mapping with class-A header validation.

use camel_api::{Body, Exchange, Value};

use crate::address::MailAddress;
use crate::config::{ContentKind, SmtpEndpointConfig};
use crate::error::MailError;
use crate::fence;
use crate::fence::Recipients;
use crate::headers;
use crate::headers::{CAMEL_MAIL_CONTENT_TYPE, CAMEL_MAIL_HTML_BODY, CAMEL_MAIL_SUBJECT};

/// A fully validated outgoing message, ready for the transport adapter.
#[derive(Clone, Debug)]
pub(crate) struct OutboundMessage {
    pub(crate) from: MailAddress,
    pub(crate) recipients: Recipients,
    pub(crate) subject: Option<String>,
    pub(crate) content: ContentKind,
    pub(crate) body: String,
    pub(crate) html_alternative: Option<String>,
    pub(crate) extra_headers: Vec<(String, String)>,
    pub(crate) message_id: String,
}

fn string_header<'a>(
    exchange: &'a Exchange,
    name: &'static str,
    reason: &'static str,
) -> Result<Option<&'a str>, MailError> {
    match exchange.input.headers.get(name) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(MailError::HeaderValidation {
            header: name.to_string(),
            reason,
        }),
    }
}

fn valid_header_name(name: &str) -> bool {
    (1..=76).contains(&name.len()) && name.bytes().all(|b| (33..=126).contains(&b) && b != b':')
}

/// Map an exchange to an outgoing message; no I/O happens here.
pub(crate) fn build_outbound(
    cfg: &SmtpEndpointConfig,
    exchange: &Exchange,
) -> Result<OutboundMessage, MailError> {
    let body = match &exchange.input.body {
        // `set_body(Value::String(..))` and a DSL `constant` arrive as a JSON string.
        Body::Text(s) | Body::Json(Value::String(s)) => s.clone(),
        Body::Bytes(b) => std::str::from_utf8(b)
            .map_err(|_| MailError::BodyType("bytes body is not valid UTF-8".to_string()))?
            .to_string(),
        other => {
            let variant = match other {
                Body::Empty => "Empty",
                Body::Json(_) => "Json",
                Body::Xml(_) => "Xml",
                Body::Stream(_) => "Stream",
                _ => "other",
            };
            return Err(MailError::BodyType(format!(
                "body type {variant} is not supported; use a String or UTF-8 bytes"
            )));
        }
    };

    let content = match string_header(
        exchange,
        CAMEL_MAIL_CONTENT_TYPE,
        "must be text/plain or text/html",
    )? {
        None => cfg.content_type,
        Some(raw) => {
            headers::validate_header_value(CAMEL_MAIL_CONTENT_TYPE, raw)?;
            ContentKind::parse(raw).ok_or_else(|| MailError::HeaderValidation {
                header: CAMEL_MAIL_CONTENT_TYPE.to_string(),
                reason: "must be text/plain or text/html",
            })?
        }
    };

    let html_alternative = match string_header(exchange, CAMEL_MAIL_HTML_BODY, "must be a string")?
    {
        None => None,
        Some(h) if h.contains('\0') => {
            return Err(MailError::HeaderValidation {
                header: CAMEL_MAIL_HTML_BODY.to_string(),
                reason: "contains NUL",
            });
        }
        Some(h) => {
            if content == ContentKind::Html {
                return Err(MailError::ConflictingBody);
            }
            Some(h.to_string())
        }
    };

    let subject = match string_header(exchange, CAMEL_MAIL_SUBJECT, "must be a string")? {
        Some(s) => {
            headers::validate_header_value(CAMEL_MAIL_SUBJECT, s)?;
            Some(s.to_string())
        }
        None => cfg.subject.clone(),
    };

    let mut extra_headers = Vec::new();
    for (name, value) in &exchange.input.headers {
        if !(name.starts_with("X-") || name.starts_with("x-")) {
            continue;
        }
        if !valid_header_name(name) {
            return Err(MailError::HeaderValidation {
                header: name.escape_debug().to_string(),
                reason: "invalid header name",
            });
        }
        let text = match value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => {
                return Err(MailError::HeaderValidation {
                    header: name.clone(),
                    reason: "value must be a string, number or bool",
                });
            }
        };
        headers::validate_header_value(name, &text)?;
        extra_headers.push((name.clone(), text));
    }
    extra_headers.sort();

    let recipients = fence::resolve_recipients(cfg, &exchange.input.headers)?;
    let message_id = format!("<{}@{}>", uuid::Uuid::new_v4().simple(), cfg.from.domain());

    Ok(OutboundMessage {
        from: cfg.from.clone(),
        recipients,
        subject,
        content,
        body,
        html_alternative,
        extra_headers,
        message_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::Message;

    const BASE: &str = "smtp://h?from=bot@corp.test&to=a@corp.test";

    fn ex(body: Body, headers: &[(&str, Value)]) -> Exchange {
        let mut m = Message::new(body);
        for (k, v) in headers {
            m.set_header(*k, v.clone());
        }
        Exchange::new(m)
    }

    fn cfg(uri: &str) -> SmtpEndpointConfig {
        SmtpEndpointConfig::from_uri(uri).expect("test uri")
    }

    fn s(v: &str) -> Value {
        Value::String(v.to_string())
    }

    fn text(v: &str) -> Body {
        Body::Text(v.to_string())
    }

    fn hv_header(r: Result<OutboundMessage, MailError>) -> (String, &'static str) {
        match r {
            Err(MailError::HeaderValidation { header, reason }) => (header, reason),
            other => panic!("expected HeaderValidation, got {other:?}"),
        }
    }

    #[test]
    fn text_body_maps_to_plain() {
        let m = build_outbound(&cfg(BASE), &ex(text("hello"), &[])).expect("ok");
        assert_eq!(m.content, ContentKind::Plain);
        assert_eq!(m.body, "hello");
        assert_eq!(m.html_alternative, None);
        let to: Vec<&str> = m.recipients.to.iter().map(|a| a.email.as_str()).collect();
        assert_eq!(to, ["a@corp.test"]);
        assert!(m.recipients.cc.is_empty());
        assert!(m.recipients.bcc.is_empty());
    }

    #[test]
    fn utf8_bytes_body_accepted() {
        let b = Body::Bytes(bytes::Bytes::from_static("héllo".as_bytes()));
        let m = build_outbound(&cfg(BASE), &ex(b, &[])).expect("ok");
        assert_eq!(m.body, "héllo");
    }

    #[test]
    fn invalid_utf8_bytes_rejected() {
        let b = Body::Bytes(bytes::Bytes::from_static(&[0xff, 0xfe]));
        let r = build_outbound(&cfg(BASE), &ex(b, &[]));
        assert!(matches!(r, Err(MailError::BodyType(_))), "{r:?}");
    }

    #[test]
    fn json_string_body_is_text() {
        let b = Body::Json(Value::String("hello".to_string()));
        let m = build_outbound(&cfg(BASE), &ex(b, &[])).expect("ok");
        assert_eq!(m.body, "hello");
    }

    #[test]
    fn json_body_rejected() {
        let b = Body::Json(serde_json::json!({"a":1}));
        match build_outbound(&cfg(BASE), &ex(b, &[])) {
            Err(MailError::BodyType(msg)) => assert!(msg.contains("Json"), "{msg}"),
            other => panic!("expected BodyType, got {other:?}"),
        }
    }

    #[test]
    fn html_option_selects_html() {
        let c = cfg(&format!("{BASE}&contentType=text/html"));
        let m = build_outbound(&c, &ex(text("<b>x</b>"), &[])).expect("ok");
        assert_eq!(m.content, ContentKind::Html);
    }

    #[test]
    fn content_type_header_overrides_option() {
        let e = ex(text("x"), &[("CamelMailContentType", s("text/html"))]);
        let m = build_outbound(&cfg(BASE), &e).expect("ok");
        assert_eq!(m.content, ContentKind::Html);
    }

    #[test]
    fn html_alternative_collected() {
        let e = ex(text("x"), &[("CamelMailHtmlBody", s("<b>x</b>"))]);
        let m = build_outbound(&cfg(BASE), &e).expect("ok");
        assert_eq!(m.html_alternative.as_deref(), Some("<b>x</b>"));
    }

    #[test]
    fn html_content_with_alternative_conflicts() {
        let c = cfg(&format!("{BASE}&contentType=text/html"));
        let e = ex(text("x"), &[("CamelMailHtmlBody", s("<b>x</b>"))]);
        let r = build_outbound(&c, &e);
        assert!(matches!(r, Err(MailError::ConflictingBody)), "{r:?}");
    }

    #[test]
    fn html_alternative_rejects_nul_only() {
        let ok = ex(text("x"), &[("CamelMailHtmlBody", s("a\r\nb"))]);
        assert!(build_outbound(&cfg(BASE), &ok).is_ok());
        let bad = ex(text("x"), &[("CamelMailHtmlBody", s("a\0b"))]);
        let (header, _) = hv_header(build_outbound(&cfg(BASE), &bad));
        assert_eq!(header, "CamelMailHtmlBody");
    }

    #[test]
    fn subject_header_overrides_option() {
        let c = cfg(&format!("{BASE}&subject=a"));
        let e = ex(text("x"), &[("CamelMailSubject", s("b"))]);
        let m = build_outbound(&c, &e).expect("ok");
        assert_eq!(m.subject.as_deref(), Some("b"));
    }

    #[test]
    fn crlf_subject_rejected() {
        let e = ex(
            text("x"),
            &[("CamelMailSubject", s("hi\r\nBcc: evil@x.test"))],
        );
        let (header, _) = hv_header(build_outbound(&cfg(BASE), &e));
        assert_eq!(header, "CamelMailSubject");
    }

    #[test]
    fn nul_x_header_rejected() {
        let e = ex(text("x"), &[("X-Trace", s("a\0b"))]);
        let (header, _) = hv_header(build_outbound(&cfg(BASE), &e));
        assert_eq!(header, "X-Trace");
    }

    #[test]
    fn overlong_subject_rejected() {
        let e = ex(text("x"), &[("CamelMailSubject", s(&"a".repeat(999)))]);
        let (_, reason) = hv_header(build_outbound(&cfg(BASE), &e));
        assert_eq!(reason, "longer than 998 bytes");
    }

    #[test]
    fn x_header_collected() {
        let e = ex(text("x"), &[("X-Trace", s("abc"))]);
        let m = build_outbound(&cfg(BASE), &e).expect("ok");
        assert_eq!(
            m.extra_headers,
            [("X-Trace".to_string(), "abc".to_string())]
        );
    }

    #[test]
    fn spoofed_from_ignored() {
        let e = ex(
            text("x"),
            &[
                ("From", s("ceo@corp.test")),
                ("Sender", s("x@y.test")),
                ("Reply-To", s("z@y.test")),
            ],
        );
        let m = build_outbound(&cfg(BASE), &e).expect("ok");
        assert_eq!(m.from.email, "bot@corp.test");
        assert!(m.extra_headers.is_empty());
    }

    #[test]
    fn message_id_uses_from_domain() {
        let m = build_outbound(&cfg(BASE), &ex(text("x"), &[])).expect("ok");
        assert!(m.message_id.starts_with('<'));
        assert!(m.message_id.ends_with("@corp.test>"));
    }
}
