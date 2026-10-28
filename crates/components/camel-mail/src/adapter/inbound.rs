//! Inbound mail to Exchange mapping (untrusted input).
//!
//! Only the documented `CamelMailIn*` headers are set, always from parsed
//! fields; no inbound header becomes a routing or control header.

use camel_api::{Body, Exchange, Message, Value};
use mail_parser::{Address, MessageParser, PartType};

use crate::headers::{
    CAMEL_MAIL_IN_CC, CAMEL_MAIL_IN_DATE, CAMEL_MAIL_IN_FOLDER, CAMEL_MAIL_IN_FROM,
    CAMEL_MAIL_IN_HTML_BODY, CAMEL_MAIL_IN_MESSAGE_ID, CAMEL_MAIL_IN_SUBJECT, CAMEL_MAIL_IN_TO,
    CAMEL_MAIL_IN_UID,
};

/// A parsed inbound message ready for the route.
pub(crate) struct InboundMessage {
    pub(crate) exchange: Exchange,
    pub(crate) message_id: Option<String>,
}

fn join_addresses(addr: Option<&Address<'_>>) -> Option<String> {
    let parts: Vec<String> = addr?
        .iter()
        .filter_map(|a| {
            let email = a.address.as_deref()?;
            Some(match a.name.as_deref() {
                Some(name) if !name.is_empty() => format!("{name} <{email}>"),
                _ => email.to_string(),
            })
        })
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

/// Parse raw RFC 822 bytes into an exchange; `None` when the bytes are not
/// a message. Never panics on bad input.
pub(crate) fn parse_inbound(raw: &[u8], uid: u32, folder: &str) -> Option<InboundMessage> {
    let parsed = MessageParser::default().parse(raw)?;

    let text = parsed.text_bodies().find_map(|p| match &p.body {
        PartType::Text(t) => Some(t.to_string()),
        _ => None,
    });
    let html = parsed.html_bodies().find_map(|p| match &p.body {
        PartType::Html(h) => Some(h.to_string()),
        _ => None,
    });

    let message_id = parsed
        .message_id()
        .map(|m| m.trim_matches(|c| c == '<' || c == '>').to_string())
        .filter(|m| !m.is_empty());

    let mut headers: Vec<(&str, Value)> = Vec::new();
    if let (Some(_), Some(h)) = (&text, &html) {
        headers.push((CAMEL_MAIL_IN_HTML_BODY, Value::from(h.as_str())));
    }
    if let Some(v) = join_addresses(parsed.from()) {
        headers.push((CAMEL_MAIL_IN_FROM, Value::from(v)));
    }
    if let Some(v) = join_addresses(parsed.to()) {
        headers.push((CAMEL_MAIL_IN_TO, Value::from(v)));
    }
    if let Some(v) = join_addresses(parsed.cc()) {
        headers.push((CAMEL_MAIL_IN_CC, Value::from(v)));
    }
    if let Some(s) = parsed.subject() {
        headers.push((CAMEL_MAIL_IN_SUBJECT, Value::from(s)));
    }
    if let Some(id) = &message_id {
        headers.push((CAMEL_MAIL_IN_MESSAGE_ID, Value::from(id.as_str())));
    }
    headers.push((CAMEL_MAIL_IN_UID, Value::from(uid)));
    if let Some(d) = parsed.date() {
        headers.push((CAMEL_MAIL_IN_DATE, Value::from(d.to_rfc3339())));
    }
    headers.push((CAMEL_MAIL_IN_FOLDER, Value::from(folder)));

    let body = text.or(html).unwrap_or_default();
    let mut message = Message::new(Body::Text(body));
    for (k, v) in headers {
        message.set_header(k, v);
    }
    Some(InboundMessage {
        exchange: Exchange::new(message),
        message_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALT_EML: &[u8] = b"From: Alice <alice@corp.test>\r\n\
To: bob@corp.test\r\n\
Cc: carol@corp.test\r\n\
Subject: greetings\r\n\
Message-ID: <m1@corp.test>\r\n\
Date: Mon, 19 Oct 2026 10:00:00 +0000\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/alternative; boundary=\"b1\"\r\n\
\r\n\
--b1\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
hello text\r\n\
--b1\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<b>hello html</b>\r\n\
--b1--\r\n";

    const HTML_ONLY_EML: &[u8] = b"From: a@corp.test\r\n\
Subject: h\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<p>only</p>\r\n";

    const ATTACH_EML: &[u8] = b"From: a@corp.test\r\n\
Subject: att\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b2\"\r\n\
\r\n\
--b2\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
see file\r\n\
--b2\r\n\
Content-Type: application/octet-stream\r\n\
Content-Disposition: attachment; filename=\"a.bin\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
AAEC\r\n\
--b2--\r\n";

    const NO_ID_EML: &[u8] = b"From: a@corp.test\r\n\
Subject: noid\r\n\
Content-Type: text/plain\r\n\
\r\n\
plain\r\n";

    fn hdr(ex: &Exchange, k: &str) -> Option<String> {
        ex.input
            .header(k)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    }

    fn body(ex: &Exchange) -> String {
        match &ex.input.body {
            Body::Text(t) => t.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn mapping_alternative_message() {
        let m = parse_inbound(ALT_EML, 42, "INBOX").expect("parse");
        let ex = &m.exchange;
        assert_eq!(body(ex).trim(), "hello text");
        assert!(
            hdr(ex, CAMEL_MAIL_IN_HTML_BODY)
                .expect("html")
                .contains("<b>hello html</b>")
        );
        assert_eq!(
            hdr(ex, CAMEL_MAIL_IN_FROM).as_deref(),
            Some("Alice <alice@corp.test>")
        );
        assert_eq!(hdr(ex, CAMEL_MAIL_IN_TO).as_deref(), Some("bob@corp.test"));
        assert_eq!(
            hdr(ex, CAMEL_MAIL_IN_CC).as_deref(),
            Some("carol@corp.test")
        );
        assert_eq!(hdr(ex, CAMEL_MAIL_IN_SUBJECT).as_deref(), Some("greetings"));
        assert_eq!(
            hdr(ex, CAMEL_MAIL_IN_MESSAGE_ID).as_deref(),
            Some("m1@corp.test")
        );
        assert_eq!(m.message_id.as_deref(), Some("m1@corp.test"));
        assert_eq!(
            ex.input.header(CAMEL_MAIL_IN_UID),
            Some(&Value::from(42u32))
        );
        assert!(
            hdr(ex, CAMEL_MAIL_IN_DATE)
                .expect("date")
                .starts_with("2026-10-19T10:00:00")
        );
        assert_eq!(hdr(ex, CAMEL_MAIL_IN_FOLDER).as_deref(), Some("INBOX"));
    }

    #[test]
    fn html_only_body_is_html_source() {
        let m = parse_inbound(HTML_ONLY_EML, 1, "INBOX").expect("parse");
        assert!(body(&m.exchange).contains("<p>only</p>"));
        assert!(m.exchange.input.header(CAMEL_MAIL_IN_HTML_BODY).is_none());
    }

    #[test]
    fn attachments_ignored() {
        let m = parse_inbound(ATTACH_EML, 1, "INBOX").expect("parse");
        assert_eq!(body(&m.exchange).trim(), "see file");
        for v in m.exchange.input.headers.values() {
            assert!(!v.to_string().contains("a.bin"));
        }
    }

    #[test]
    fn missing_message_id_absent() {
        let m = parse_inbound(NO_ID_EML, 1, "INBOX").expect("parse");
        assert!(m.exchange.input.header(CAMEL_MAIL_IN_MESSAGE_ID).is_none());
        assert_eq!(m.message_id, None);
    }

    #[test]
    fn inbound_sets_only_in_headers() {
        let m = parse_inbound(ALT_EML, 42, "INBOX").expect("parse");
        assert!(!m.exchange.input.headers.is_empty());
        for k in m.exchange.input.headers.keys() {
            assert!(k.starts_with("CamelMailIn"), "unexpected header {k}");
        }
    }

    #[test]
    fn hostile_input_never_panics() {
        let mut nested = String::from("Content-Type: multipart/mixed; boundary=b\r\n\r\n");
        for _ in 0..200 {
            nested.push_str("--b\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n");
        }
        let inputs: [&[u8]; 6] = [
            b"",
            b"\0\0\0\xff\xfe",
            b"Subject: =?utf-8?b?////?=\r\nFrom: <<<@@@>>>\r\n\r\n",
            b"Content-Type: multipart/alternative; boundary=\"x\"\r\n\r\n--x\r\n",
            b"Date: 99 Foo 99999 99:99:99 +9999\r\nMessage-ID: <>\r\n\r\nbody",
            nested.as_bytes(),
        ];
        for raw in inputs {
            if let Some(m) = parse_inbound(raw, 1, "INBOX") {
                assert_eq!(
                    m.exchange.input.header(CAMEL_MAIL_IN_FOLDER),
                    Some(&Value::from("INBOX"))
                );
                assert_ne!(m.message_id.as_deref(), Some(""));
            }
        }
    }
}
