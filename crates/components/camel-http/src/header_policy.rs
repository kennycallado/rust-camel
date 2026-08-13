//! HTTP header emission policy classification (ADR-0057).
//!
//! Pure functions that classify header names into the RFC-derived buckets
//! used by the producer outbound filter and the consumer reply finaliser.
//! See `docs/adr/0057-http-header-emission-policy.md`.

/// Compatibility hop-by-hop set (RFC 2616 section 13.5.1 conventions + RFC
/// 7230 per-section definitions), plus the non-standard `proxy-connection`
/// compatibility addition. RFC 7230 section 6.1 mandates removing
/// `Connection` and every header named by its connection-options; this static
/// list covers the conventional members that are hop-by-hop on every
/// connection. Stripped in both directions.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "proxy-connection",
];

/// Request-only headers: meaningful on requests but not on responses.
/// `proxy-authorization` is intentionally absent — it lives in [`HOP_BY_HOP`].
const REQUEST_ONLY: &[&str] = &[
    "host",
    "user-agent",
    "accept",
    "accept-encoding",
    "accept-language",
    "accept-charset",
    "accept-datetime",
    "authorization",
    "cookie",
    "expect",
    "from",
    "if-match",
    "if-modified-since",
    "if-none-match",
    "if-range",
    "if-unmodified-since",
    "max-forwards",
    "range",
    "referer",
];

/// Server-owned headers (RFC 7231 section 7.1.1.2). The origin server
/// derives these; a proxy must not forward a client-supplied value.
const SERVER_OWNED: &[&str] = &["date"];

/// Returns true when `c` is an RFC 7230 `tchar` (token character).
const fn is_tchar(c: u8) -> bool {
    matches!(c, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9')
        || matches!(
            c,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// Returns true when `token` is a valid RFC 7230 `token` (one or more tchars).
fn is_valid_token(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(is_tchar)
}

/// Parse the `Connection` header value(s) into the set of connection-named
/// tokens that must be treated as hop-by-hop in both directions (RFC 7230
/// section 6.1).
///
/// Each input value is split on `,`; segments are trimmed and lowercased.
/// Only segments that form a valid RFC 7230 `token` (one or more `tchar`s)
/// are kept. Results are de-duplicated preserving first-seen order. Malformed
/// or empty segments are ignored. This function never panics.
pub(crate) fn connection_tokens<'a, I>(connection_header_values: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut seen = Vec::new();
    for value in connection_header_values {
        for raw in value.split(',') {
            let candidate = raw.trim().to_ascii_lowercase();
            if is_valid_token(&candidate) && !seen.contains(&candidate) {
                seen.push(candidate);
            }
        }
    }
    seen
}

/// Outbound (producer) request filter: returns true when `name` must be
/// excluded from the forwarded request.
///
/// Excludes hop-by-hop/framing headers, `content-length` (re-derived by the
/// client), `host` (destination-derived), and any header named by a
/// connection token. Request-only headers ARE forwarded.
pub(crate) fn excluded_outbound(name: &str, connection_tokens: &[String]) -> bool {
    let lower = name.to_ascii_lowercase();
    HOP_BY_HOP.contains(&lower.as_str())
        || lower == "content-length"
        || lower == "host"
        || connection_tokens.contains(&lower)
}

/// Consumer reply filter: returns true when `name` must be excluded from the
/// emitted HTTP response.
///
/// Excludes hop-by-hop/framing, request-only, server-owned headers,
/// `content-length` and `content-type` (re-derived), and any header named by
/// a connection token. Returns false for `cache-control`, `pragma`, `warning`,
/// and `via` (valid response headers).
pub(crate) fn excluded_response(name: &str, connection_tokens: &[String]) -> bool {
    let lower = name.to_ascii_lowercase();
    HOP_BY_HOP.contains(&lower.as_str())
        || REQUEST_ONLY.contains(&lower.as_str())
        || SERVER_OWNED.contains(&lower.as_str())
        || lower == "content-length"
        || lower == "content-type"
        || connection_tokens.contains(&lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excluded_outbound_strips_hop_by_hop() {
        for name in ["Connection", "Transfer-Encoding", "Upgrade"] {
            assert!(
                excluded_outbound(name, &[]),
                "{name} should be excluded outbound"
            );
        }
    }

    #[test]
    fn excluded_outbound_excludes_host_and_content_length() {
        assert!(excluded_outbound("Host", &[]));
        assert!(excluded_outbound("Content-Length", &[]));
    }

    #[test]
    fn excluded_outbound_forwards_request_only() {
        assert!(!excluded_outbound("Accept", &[]));
        assert!(!excluded_outbound("User-Agent", &[]));
    }

    #[test]
    fn excluded_outbound_dynamic_connection() {
        let tokens = connection_tokens(["X-Custom, Keep-Alive"]);
        assert!(excluded_outbound("X-Custom", &tokens));
        assert!(!excluded_outbound("X-Other", &tokens));
    }

    #[test]
    fn connection_tokens_casefold_dedup() {
        let tokens = connection_tokens(["X-Custom, x-custom,  X-Custom "]);
        assert_eq!(tokens, ["x-custom"]);
    }

    #[test]
    fn connection_tokens_malformed_no_panic() {
        let tokens = connection_tokens(["X-Custom, bad token, ,"]);
        assert_eq!(tokens, ["x-custom"]);
        assert!(!excluded_outbound("X-Unrelated", &tokens));
        assert!(!excluded_response("X-Unrelated", &tokens));
    }

    #[test]
    fn excluded_response_strips_hop_by_hop() {
        for name in ["Connection", "Transfer-Encoding", "Upgrade"] {
            assert!(
                excluded_response(name, &[]),
                "{name} should be excluded from response"
            );
        }
    }

    #[test]
    fn excluded_response_keeps_cache_control_pragma_warning_via() {
        for name in ["Cache-Control", "Pragma", "Warning", "Via"] {
            assert!(
                !excluded_response(name, &[]),
                "{name} should be kept on response"
            );
        }
    }

    #[test]
    fn excluded_response_excludes_request_only_and_server_owned() {
        assert!(excluded_response("User-Agent", &[]));
        assert!(excluded_response("Accept", &[]));
        assert!(excluded_response("Date", &[]));
    }

    #[test]
    fn excluded_response_excludes_content_length_and_type() {
        assert!(excluded_response("Content-Length", &[]));
        assert!(excluded_response("Content-Type", &[]));
    }

    #[test]
    fn excluded_response_dynamic_connection() {
        let tokens = connection_tokens(["X-Custom"]);
        assert!(excluded_response("X-Custom", &tokens));
    }
}
