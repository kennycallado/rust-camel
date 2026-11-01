use bytes::Bytes;
use camel_api::fragment_exchange;
use camel_api::{Body, CamelError, Exchange, Value};
use futures::Stream;
use mail_parser::{MessageParser, MessagePart, MimeHeaders, PartType};
use std::pin::Pin;

/// Zero-based index of the emitted fragment.
pub const CAMEL_MIME_PART_INDEX: &str = "CamelMimePartIndex";
/// Decoded attachment filename.
pub const CAMEL_MIME_PART_NAME: &str = "CamelMimePartName";
/// Declared Content-Type of the part, untrusted.
pub const CAMEL_MIME_PART_CONTENT_TYPE: &str = "CamelMimePartContentType";
/// Decoded byte length of the fragment body.
pub const CAMEL_MIME_PART_SIZE: &str = "CamelMimePartSize";
/// `attachment` or `inline`, from Content-Disposition.
pub const CAMEL_MIME_PART_DISPOSITION: &str = "CamelMimePartDisposition";
/// Content-ID of the part, when declared.
pub const CAMEL_MIME_PART_CONTENT_ID: &str = "CamelMimePartContentId";

/// Default maximum number of fragments emitted by [`MimeSplitConfig`].
pub const DEFAULT_MAX_PARTS: usize = 100;

/// Maximum byte length of sender-supplied header values.
const MAX_PART_VALUE_BYTES: usize = 255;

/// Configuration for the MIME splitter.
#[derive(Debug, Clone)]
pub struct MimeSplitConfig {
    /// Maximum number of fragments to emit.
    pub max_parts: usize,
    /// Maximum decoded size per part; defaults to the input length.
    pub max_part_size: Option<usize>,
    /// Optional allowlist of declared `type/subtype` values.
    pub content_types: Option<Vec<String>>,
}

impl Default for MimeSplitConfig {
    fn default() -> Self {
        Self {
            max_parts: DEFAULT_MAX_PARTS,
            max_part_size: None,
            content_types: None,
        }
    }
}

/// Effective per-part size limit: an explicit limit or the input length.
fn effective_part_limit(input_len: usize, max_part_size: Option<usize>) -> usize {
    max_part_size.unwrap_or(input_len)
}

/// Declared `type/subtype` of a part, ignoring parameters.
fn declared_content_type(part: &MessagePart<'_>) -> Option<String> {
    let content_type = part.content_type()?;
    match content_type.subtype() {
        Some(subtype) => Some(format!("{}/{}", content_type.ctype(), subtype)),
        None => Some(content_type.ctype().to_string()),
    }
}

/// Disposition from Content-Disposition; absent means `attachment`.
fn part_disposition(part: &MessagePart<'_>) -> &'static str {
    match part.content_disposition() {
        Some(disposition) if disposition.is_inline() => "inline",
        _ => "attachment",
    }
}

/// Remove Unicode control characters (NUL, C0, DEL, C1).
fn strip_controls(raw: &str) -> String {
    raw.chars()
        .filter(|character| !character.is_control())
        .collect()
}

/// Cap a string at `max_bytes` without splitting a UTF-8 sequence.
fn cap_utf8_bytes(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

/// Sanitize a sender-supplied header value: strip controls and cap it.
fn clean_part_value(raw: &str) -> String {
    cap_utf8_bytes(strip_controls(raw), MAX_PART_VALUE_BYTES)
}

/// Sanitize a sender-supplied attachment name for use as a fragment header.
fn sanitize_part_name(raw: Option<&str>, index: usize) -> String {
    let Some(raw) = raw else {
        return format!("part-{index}");
    };
    let basename = match raw.rfind(['/', '\\']) {
        Some(position) => &raw[position + 1..],
        None => raw,
    };
    let stripped = strip_controls(basename);
    let trimmed = stripped.trim_matches([' ', '.']);
    if trimmed.is_empty() {
        return format!("part-{index}");
    }
    cap_utf8_bytes(trimmed.to_string(), MAX_PART_VALUE_BYTES)
}

/// Whether a declared content type matches the allowlist.
fn allowlist_matches(allowlist: Option<&[String]>, declared: Option<&str>) -> bool {
    let Some(allowlist) = allowlist.filter(|list| !list.is_empty()) else {
        return true;
    };
    let Some(declared) = declared else {
        return false;
    };
    allowlist
        .iter()
        .any(|allowed| allowed.trim().eq_ignore_ascii_case(declared))
}

/// Split a MIME message's attachment parts into a stream of Exchanges.
///
/// Emits one fragment per attachment reported by `mail-parser`, in document
/// order. Text and HTML body parts, multipart containers, and nested
/// `message/rfc822` descendants are not emitted.
pub fn split_mime_bytes(
    parent: Exchange,
    bytes: Bytes,
    config: MimeSplitConfig,
) -> Pin<Box<dyn Stream<Item = Result<Exchange, CamelError>> + Send>> {
    Box::pin(async_stream::stream! {
        let part_limit = effective_part_limit(bytes.len(), config.max_part_size);
        let allowlist = config.content_types.as_deref();
        let Some(message) = MessageParser::default().parse(bytes.as_ref()) else {
            yield Err(CamelError::TypeConversionFailed(
                "MIME message could not be parsed".into(),
            ));
            return;
        };

        let mut index = 0usize;
        for (current_part, part) in message.attachments().enumerate() {
            let declared = declared_content_type(part);
            if !allowlist_matches(allowlist, declared.as_deref()) {
                continue;
            }
            if part.is_encoding_problem {
                yield Err(CamelError::TypeConversionFailed(format!(
                    "MIME part {current_part} has a malformed transfer encoding"
                )));
                return;
            }
            if index >= config.max_parts {
                yield Err(CamelError::TypeConversionFailed(format!(
                    "MIME message exceeds max_parts ({})",
                    config.max_parts
                )));
                return;
            }
            let decoded = match &part.body {
                PartType::Text(text) | PartType::Html(text) => Some(text.as_bytes().to_vec()),
                PartType::Binary(binary) | PartType::InlineBinary(binary) => Some(binary.to_vec()),
                // `mail-parser` 0.11.9 stores the outer raw message in the
                // nested message's `raw_message` field, so slice the part
                // offsets to preserve the exact embedded RFC 822 bytes. Fall
                // back to nothing rather than emitting the outer message.
                PartType::Message(_) => bytes
                    .get(part.raw_body_offset() as usize..part.raw_end_offset() as usize)
                    .map(<[u8]>::to_vec),
                PartType::Multipart(_) => continue,
            };
            let Some(decoded) = decoded else {
                yield Err(CamelError::TypeConversionFailed(
                    "message/rfc822 part offsets unavailable".into(),
                ));
                return;
            };
            let size = decoded.len();
            if size > part_limit {
                yield Err(CamelError::TypeConversionFailed(format!(
                    "MIME part {current_part} exceeds max_part_size ({size} > {part_limit})"
                )));
                return;
            }

            let mut exchange = fragment_exchange(&parent, Body::Bytes(Bytes::from(decoded)));
            for inherited in [
                CAMEL_MIME_PART_INDEX,
                CAMEL_MIME_PART_NAME,
                CAMEL_MIME_PART_CONTENT_TYPE,
                CAMEL_MIME_PART_SIZE,
                CAMEL_MIME_PART_DISPOSITION,
                CAMEL_MIME_PART_CONTENT_ID,
            ] {
                exchange.input.headers.remove(inherited);
            }

            exchange.input.headers.insert(
                CAMEL_MIME_PART_INDEX.to_string(),
                Value::from(index as u64),
            );
            exchange.input.headers.insert(
                CAMEL_MIME_PART_NAME.to_string(),
                Value::String(sanitize_part_name(part.attachment_name(), index)),
            );
            if let Some(content_type) = declared {
                exchange.input.headers.insert(
                    CAMEL_MIME_PART_CONTENT_TYPE.to_string(),
                    Value::String(clean_part_value(&content_type)),
                );
            }
            exchange.input.headers.insert(
                CAMEL_MIME_PART_SIZE.to_string(),
                Value::from(size as u64),
            );
            exchange.input.headers.insert(
                CAMEL_MIME_PART_DISPOSITION.to_string(),
                Value::String(part_disposition(part).to_string()),
            );
            if let Some(content_id) = part.content_id() {
                exchange.input.headers.insert(
                    CAMEL_MIME_PART_CONTENT_ID.to_string(),
                    Value::String(clean_part_value(content_id)),
                );
            }

            index += 1;
            yield Ok(exchange);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Message, Value};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use futures::StreamExt;
    use std::io::Write;

    fn make_parent(headers: &[(&str, &str)]) -> Exchange {
        let mut msg = Message::new(Body::Empty);
        for (name, value) in headers {
            msg.headers
                .insert((*name).to_string(), Value::String((*value).to_string()));
        }
        Exchange::new(msg)
    }

    async fn collect(
        parent: Exchange,
        raw: Vec<u8>,
        config: MimeSplitConfig,
    ) -> Vec<Result<Exchange, CamelError>> {
        split_mime_bytes(parent, Bytes::from(raw), config)
            .collect()
            .await
    }

    async fn collect_default(raw: Vec<u8>) -> Vec<Result<Exchange, CamelError>> {
        collect(make_parent(&[]), raw, MimeSplitConfig::default()).await
    }

    fn body_bytes(exchange: &Exchange) -> &[u8] {
        match &exchange.input.body {
            Body::Bytes(bytes) => bytes.as_ref(),
            other => panic!("expected Body::Bytes, got {other:?}"),
        }
    }

    fn header_str<'a>(exchange: &'a Exchange, name: &str) -> Option<&'a str> {
        match exchange.input.headers.get(name) {
            Some(Value::String(value)) => Some(value.as_str()),
            _ => None,
        }
    }

    fn header_u64(exchange: &Exchange, name: &str) -> Option<u64> {
        match exchange.input.headers.get(name) {
            Some(Value::Number(value)) => value.as_u64(),
            _ => None,
        }
    }

    fn multipart_mixed(parts: &[String]) -> Vec<u8> {
        multipart("mixed", "test-boundary", parts)
    }

    fn multipart(kind: &str, boundary: &str, parts: &[String]) -> Vec<u8> {
        let mut raw = format!(
            "MIME-Version: 1.0\r\nContent-Type: multipart/{kind}; boundary=\"{boundary}\"\r\n\r\n"
        );
        for part in parts {
            raw.push_str(&format!("--{boundary}\r\n{part}\r\n"));
        }
        raw.push_str(&format!("--{boundary}--\r\n"));
        raw.into_bytes()
    }

    fn text_attachment(filename: &str, body: &str) -> String {
        format!(
            "Content-Type: text/plain; name=\"{filename}\"\r\n\
             Content-Disposition: attachment; filename=\"{filename}\"\r\n\r\n{body}"
        )
    }

    fn typed_attachment(content_type: &str, filename: &str, body: &str) -> String {
        format!(
            "Content-Type: {content_type}; name=\"{filename}\"\r\n\
             Content-Disposition: attachment; filename=\"{filename}\"\r\n\r\n{body}"
        )
    }

    fn error_message(results: &[Result<Exchange, CamelError>]) -> String {
        match results.last() {
            Some(Err(error)) => error.to_string(),
            other => panic!("expected a trailing error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn emits_zero_fragments_for_plain_text() {
        let raw =
            b"MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nhello world\r\n"
                .to_vec();
        let results = collect_default(raw).await;
        assert_eq!(results.len(), 0, "{results:?}");
    }

    #[tokio::test]
    async fn emits_one_and_three_in_document_order() {
        let one = multipart_mixed(&[text_attachment("only.txt", "one")]);
        let results = collect_default(one).await;
        assert_eq!(results.len(), 1, "{results:?}");
        let first = results[0].as_ref().expect("one fragment");
        assert_eq!(header_str(first, CAMEL_MIME_PART_NAME), Some("only.txt"));

        let three = multipart_mixed(&[
            text_attachment("a.txt", "alpha"),
            text_attachment("b.txt", "beta"),
            text_attachment("c.txt", "gamma"),
        ]);
        let results = collect_default(three).await;
        assert_eq!(results.len(), 3, "{results:?}");
        let names: Vec<_> = results
            .iter()
            .map(|r| header_str(r.as_ref().expect("fragment"), CAMEL_MIME_PART_NAME).unwrap())
            .collect();
        assert_eq!(names, ["a.txt", "b.txt", "c.txt"]);
        let indexes: Vec<_> = results
            .iter()
            .map(|r| header_u64(r.as_ref().expect("fragment"), CAMEL_MIME_PART_INDEX).unwrap())
            .collect();
        assert_eq!(indexes, [0, 1, 2]);
    }

    #[tokio::test]
    async fn alternative_bodies_not_emitted() {
        let parts = vec![
            "Content-Type: text/plain\r\n\r\nplain body".to_string(),
            "Content-Type: text/html\r\n\r\n<p>html body</p>".to_string(),
        ];
        let raw = multipart("alternative", "alt-boundary", &parts);
        let results = collect_default(raw).await;
        assert_eq!(results.len(), 0, "{results:?}");
    }

    #[tokio::test]
    async fn text_csv_attachment_emitted() {
        let mut raw = b"MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/csv; charset=iso-8859-1; name=\"data.csv\"\r\nContent-Disposition: attachment; filename=\"data.csv\"\r\n\r\na,".to_vec();
        raw.extend_from_slice(&[0xE9]);
        raw.extend_from_slice(b"\r\n--b--\r\n");

        let results = collect_default(raw).await;
        assert_eq!(results.len(), 1, "{results:?}");
        let fragment = results[0].as_ref().expect("csv fragment");
        assert_eq!(body_bytes(fragment), b"a,\xC3\xA9");
        assert_eq!(
            header_str(fragment, CAMEL_MIME_PART_CONTENT_TYPE),
            Some("text/csv")
        );
        assert_eq!(
            header_str(fragment, CAMEL_MIME_PART_DISPOSITION),
            Some("attachment")
        );
    }

    #[tokio::test]
    async fn inline_image_emitted_with_content_id() {
        let parts = vec![
            "Content-Type: text/html\r\n\r\n<img src=\"cid:img1\">".to_string(),
            "Content-Type: image/png; name=\"pic.png\"\r\n\
             Content-Disposition: inline; filename=\"pic.png\"\r\n\
             Content-ID: <img1>\r\n\
             Content-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo="
                .to_string(),
        ];
        let raw = multipart("related", "rel-boundary", &parts);
        let results = collect_default(raw).await;
        assert_eq!(results.len(), 1, "{results:?}");
        let fragment = results[0].as_ref().expect("inline image");
        assert_eq!(
            header_str(fragment, CAMEL_MIME_PART_DISPOSITION),
            Some("inline")
        );
        let content_id = header_str(fragment, CAMEL_MIME_PART_CONTENT_ID).unwrap_or_default();
        assert!(content_id.contains("img1"), "content id was {content_id:?}");
    }

    #[tokio::test]
    async fn message_rfc822_one_raw_fragment_no_descendants() {
        let inner = "From: inner@example.com\r\nSubject: Inner\r\nContent-Type: multipart/mixed; boundary=\"inner\"\r\n\r\n--inner\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=\"hidden.txt\"\r\n\r\npayload\r\n--inner--";
        let part = format!(
            "Content-Type: message/rfc822\r\n\
             Content-Disposition: attachment; filename=\"inner.eml\"\r\n\r\n{inner}"
        );
        let outer = multipart_mixed(&[part]);

        let results = collect_default(outer).await;
        assert_eq!(results.len(), 1, "{results:?}");
        let fragment = results[0].as_ref().expect("raw message fragment");
        assert_eq!(body_bytes(fragment), inner.as_bytes());
        assert_eq!(
            header_str(fragment, CAMEL_MIME_PART_NAME),
            Some("inner.eml")
        );
    }

    #[tokio::test]
    async fn base64_aaec_decodes_to_0_1_2() {
        let part = "Content-Type: application/octet-stream; name=\"blob.bin\"\r\n\
                    Content-Disposition: attachment; filename=\"blob.bin\"\r\n\
                    Content-Transfer-Encoding: base64\r\n\r\nAAEC"
            .to_string();
        let raw = multipart_mixed(&[part]);
        let results = collect_default(raw).await;
        assert_eq!(results.len(), 1, "{results:?}");
        let fragment = results[0].as_ref().expect("base64 fragment");
        assert_eq!(body_bytes(fragment), [0, 1, 2]);
        assert_eq!(header_u64(fragment, CAMEL_MIME_PART_SIZE), Some(3));
        assert_eq!(
            header_str(fragment, CAMEL_MIME_PART_DISPOSITION),
            Some("attachment")
        );
    }

    #[tokio::test]
    async fn quoted_printable_decoded() {
        let part = "Content-Type: application/octet-stream; name=\"qp.bin\"\r\n\
                    Content-Disposition: attachment; filename=\"qp.bin\"\r\n\
                    Content-Transfer-Encoding: quoted-printable\r\n\r\n=00=01=02"
            .to_string();
        let raw = multipart_mixed(&[part]);
        let results = collect_default(raw).await;
        assert_eq!(results.len(), 1, "{results:?}");
        let fragment = results[0].as_ref().expect("quoted-printable fragment");
        assert_eq!(body_bytes(fragment), [0, 1, 2]);
    }

    #[tokio::test]
    async fn parent_headers_copied() {
        let raw = multipart_mixed(&[
            text_attachment("first.txt", "first"),
            text_attachment("second.txt", "second"),
        ]);
        let parent = make_parent(&[("X-Parent", "kept")]);
        let results = collect(parent, raw, MimeSplitConfig::default()).await;
        assert_eq!(results.len(), 2, "{results:?}");
        for (expected_index, result) in results.iter().enumerate() {
            let fragment = result.as_ref().expect("fragment");
            assert_eq!(header_str(fragment, "X-Parent"), Some("kept"));
            assert_eq!(
                header_u64(fragment, CAMEL_MIME_PART_INDEX),
                Some(expected_index as u64)
            );
            assert!(header_str(fragment, CAMEL_MIME_PART_CONTENT_TYPE).is_some());
            assert!(header_u64(fragment, CAMEL_MIME_PART_SIZE).is_some());
            assert!(header_str(fragment, CAMEL_MIME_PART_DISPOSITION).is_some());
        }
    }

    #[tokio::test]
    async fn default_max_part_size_is_input_length() {
        let mut raw = b"MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/csv; charset=iso-8859-1; name=\"latin.csv\"\r\nContent-Disposition: attachment; filename=\"latin.csv\"\r\n\r\n".to_vec();
        raw.extend(std::iter::repeat_n(0xE9u8, 500));
        raw.extend_from_slice(b"\r\n--b--\r\n");

        let config = MimeSplitConfig::default();
        let limit = effective_part_limit(raw.len(), config.max_part_size);
        assert_eq!(
            limit,
            raw.len(),
            "default per-part limit must be the input length"
        );

        let message = MessageParser::default()
            .parse(raw.as_slice())
            .expect("parse");
        let part = message.attachments().next().expect("latin-1 attachment");
        let converted_len = match &part.body {
            PartType::Text(text) | PartType::Html(text) => text.len(),
            other => panic!("expected text part, got {other:?}"),
        };
        assert!(
            converted_len > limit,
            "UTF-8 conversion ({converted_len}) must exceed the input length ({limit})"
        );

        let results = collect_default(raw).await;
        assert_eq!(results.len(), 1, "{results:?}");
        assert!(
            error_message(&results).contains("max_part_size"),
            "error must name the default limit: {results:?}"
        );
    }

    #[tokio::test]
    async fn gzip_attachment_not_decompressed() {
        let payload = b"the quick brown fox jumps over the lazy dog";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(payload).expect("gzip encode");
        let compressed = encoder.finish().expect("finish gzip");
        assert_eq!(&compressed[..2], b"\x1f\x8b");

        let mut raw = b"MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: application/gzip; name=\"data.gz\"\r\nContent-Disposition: attachment; filename=\"data.gz\"\r\n\r\n".to_vec();
        raw.extend_from_slice(&compressed);
        raw.extend_from_slice(b"\r\n--b--\r\n");

        let results = collect_default(raw).await;
        assert_eq!(results.len(), 1, "{results:?}");
        let fragment = results[0].as_ref().expect("gzip fragment");
        let body = body_bytes(fragment);
        assert_eq!(&body[..2], b"\x1f\x8b");
        assert_eq!(body, compressed.as_slice());
    }

    #[tokio::test]
    async fn max_parts_excess_fails() {
        let raw = multipart_mixed(&[
            text_attachment("a.txt", "alpha"),
            text_attachment("b.txt", "beta"),
            text_attachment("c.txt", "gamma"),
        ]);
        let config = MimeSplitConfig {
            max_parts: 2,
            ..MimeSplitConfig::default()
        };
        let results = collect(make_parent(&[]), raw, config).await;

        assert_eq!(results.len(), 3, "{results:?}");
        assert_eq!(
            results[..2]
                .iter()
                .map(|r| header_u64(r.as_ref().expect("fragment"), CAMEL_MIME_PART_INDEX).unwrap())
                .collect::<Vec<_>>(),
            [0, 1]
        );
        let message = error_message(&results);
        assert!(message.contains("max_parts"), "error was {message:?}");
    }

    #[tokio::test]
    async fn filtered_parts_do_not_consume_limit() {
        let raw = multipart_mixed(&[
            typed_attachment("application/zip", "a.zip", "a"),
            typed_attachment("application/zip", "b.zip", "b"),
            typed_attachment("application/zip", "c.zip", "c"),
            typed_attachment("application/pdf", "d.pdf", "%PDF-1.7"),
        ]);
        let config = MimeSplitConfig {
            max_parts: 2,
            content_types: Some(vec!["application/pdf".to_string()]),
            ..MimeSplitConfig::default()
        };
        let results = collect(make_parent(&[]), raw, config).await;

        assert_eq!(results.len(), 1, "{results:?}");
        let fragment = results[0].as_ref().expect("pdf fragment");
        assert_eq!(header_u64(fragment, CAMEL_MIME_PART_INDEX), Some(0));
        assert_eq!(
            header_str(fragment, CAMEL_MIME_PART_CONTENT_TYPE),
            Some("application/pdf")
        );
    }

    #[tokio::test]
    async fn max_part_size_excess_fails() {
        let raw = multipart_mixed(&[typed_attachment(
            "application/octet-stream",
            "blob.bin",
            "0123456789a",
        )]);
        let config = MimeSplitConfig {
            max_part_size: Some(10),
            ..MimeSplitConfig::default()
        };
        let results = collect(make_parent(&[]), raw, config).await;

        assert_eq!(results.len(), 1, "{results:?}");
        let message = error_message(&results);
        assert!(message.contains("max_part_size"), "error was {message:?}");
    }

    #[tokio::test]
    async fn allowlist_drops_non_matching() {
        let raw = multipart_mixed(&[
            typed_attachment("application/zip", "a.zip", "zip"),
            typed_attachment("application/pdf", "b.pdf", "%PDF-1.7"),
        ]);
        let config = MimeSplitConfig {
            content_types: Some(vec!["application/zip".to_string()]),
            ..MimeSplitConfig::default()
        };
        let results = collect(make_parent(&[]), raw, config).await;

        assert_eq!(results.len(), 1, "{results:?}");
        assert_eq!(
            header_str(
                results[0].as_ref().expect("zip fragment"),
                CAMEL_MIME_PART_CONTENT_TYPE
            ),
            Some("application/zip")
        );
    }

    #[tokio::test]
    async fn all_parts_filtered_succeeds_empty() {
        let raw = multipart_mixed(&[typed_attachment("application/pdf", "b.pdf", "%PDF-1.7")]);
        let config = MimeSplitConfig {
            content_types: Some(vec!["application/zip".to_string()]),
            ..MimeSplitConfig::default()
        };
        let results = collect(make_parent(&[]), raw, config).await;

        assert_eq!(results.len(), 0, "{results:?}");
    }

    #[tokio::test]
    async fn allowlist_case_insensitive_parameter_blind() {
        let raw = multipart_mixed(&[typed_attachment("Application/ZIP", "a.zip", "zip")]);
        let config = MimeSplitConfig {
            content_types: Some(vec!["application/zip".to_string()]),
            ..MimeSplitConfig::default()
        };
        let results = collect(make_parent(&[]), raw, config).await;

        assert_eq!(results.len(), 1, "{results:?}");
        assert_eq!(
            header_str(
                results[0].as_ref().expect("zip fragment"),
                CAMEL_MIME_PART_NAME
            ),
            Some("a.zip")
        );
    }

    #[test]
    fn sanitization_table() {
        for (raw, expected) in [
            ("../../etc/passwd", "passwd"),
            ("a\0b.txt", "ab.txt"),
            ("  .hidden.  ", "hidden"),
            ("..", "part-0"),
            (" report .zip ", "report .zip"),
        ] {
            assert_eq!(
                sanitize_part_name(Some(raw), 0),
                expected,
                "sanitizing {raw:?}"
            );
        }
    }

    #[test]
    fn multibyte_name_capped_at_utf8_boundary() {
        let raw = "é".repeat(300);
        let sanitized = sanitize_part_name(Some(&raw), 0);
        assert!(
            sanitized.len() <= 255,
            "capped name has {} bytes",
            sanitized.len()
        );
        assert_eq!(sanitized.chars().count(), 127);
    }

    #[tokio::test]
    async fn unparseable_message_fails() {
        let results = collect_default(Vec::new()).await;

        assert_eq!(results.len(), 1, "{results:?}");
        let message = error_message(&results);
        assert!(
            message.contains("could not be parsed"),
            "error was {message:?}"
        );
    }

    #[tokio::test]
    async fn malformed_transfer_encoding_fails() {
        let part = "Content-Type: application/octet-stream; name=\"bad.bin\"\r\n\
                    Content-Disposition: attachment; filename=\"bad.bin\"\r\n\
                    Content-Transfer-Encoding: base64\r\n\r\n*AAA"
            .to_string();
        let raw = multipart_mixed(&[part]);
        let results = collect_default(raw).await;

        assert_eq!(results.len(), 1, "{results:?}");
        let message = error_message(&results);
        assert!(
            message.contains("malformed transfer encoding"),
            "error was {message:?}"
        );
        assert!(message.contains('0'), "error names the part: {message:?}");
    }

    #[tokio::test]
    async fn inherited_content_id_not_leaked() {
        let raw = multipart_mixed(&[text_attachment("plain.txt", "body")]);
        let parent = make_parent(&[(CAMEL_MIME_PART_CONTENT_ID, "inherited-id")]);
        let results = collect(parent, raw, MimeSplitConfig::default()).await;

        assert_eq!(results.len(), 1, "{results:?}");
        let fragment = results[0].as_ref().expect("fragment");
        assert_eq!(header_str(fragment, CAMEL_MIME_PART_CONTENT_ID), None);
    }

    #[tokio::test]
    async fn content_type_and_id_control_stripped_and_capped() {
        let long_subtype = format!("{}{}", "a".repeat(320), '\u{1}');
        let long_id = format!("<{}{}{}>", "a".repeat(200), '\u{7f}', "b".repeat(200));
        let part = format!(
            "Content-Type: text/{long_subtype}; name=\"long.txt\"\r\n\
             Content-Disposition: attachment; filename=\"long.txt\"\r\n\
             Content-ID: {long_id}\r\n\r\npayload"
        );
        let raw = multipart_mixed(&[part]);
        let results = collect_default(raw).await;

        assert_eq!(results.len(), 1, "{results:?}");
        let fragment = results[0].as_ref().expect("fragment");
        let content_type =
            header_str(fragment, CAMEL_MIME_PART_CONTENT_TYPE).expect("content type");
        let content_id = header_str(fragment, CAMEL_MIME_PART_CONTENT_ID).expect("content id");
        for value in [content_type, content_id] {
            assert!(
                !value.chars().any(char::is_control),
                "value carries a control character: {value:?}"
            );
            assert!(value.len() <= 255, "value is {} bytes", value.len());
        }
    }

    #[tokio::test]
    async fn name_falls_back_to_content_type_name() {
        let raw = multipart_mixed(&[
            "Content-Type: text/plain\r\n\r\nbody".to_string(),
            "Content-Type: text/plain; name=\"x.pdf\"\r\n\r\npayload".to_string(),
        ]);
        let results = collect_default(raw).await;

        assert_eq!(results.len(), 1, "{results:?}");
        assert_eq!(
            header_str(results[0].as_ref().expect("fragment"), CAMEL_MIME_PART_NAME),
            Some("x.pdf")
        );
    }

    #[tokio::test]
    async fn rfc2231_filename_decoded() {
        let part = "Content-Type: application/zip\r\n\
                    Content-Disposition: attachment; filename*=UTF-8''caf%C3%A9.zip\r\n\r\nzip"
            .to_string();
        let raw = multipart_mixed(&[part]);
        let results = collect_default(raw).await;

        assert_eq!(results.len(), 1, "{results:?}");
        assert_eq!(
            header_str(results[0].as_ref().expect("fragment"), CAMEL_MIME_PART_NAME),
            Some("café.zip")
        );
    }
}
