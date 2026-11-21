use super::get_u64_header;
use super::invalid_header_value_preview;
use super::key::resolve_keys_pattern;
use crate::config::RedisCommand;
use camel_component_api::Body;
use camel_component_api::{CamelError, Exchange};
use redis::aio::MultiplexedConnection;
use std::collections::HashMap;

/// Default `COUNT` hint for a SCAN family page: the repository pager's page
/// size.
const SCAN_PAGE_COUNT: u64 = 100;

/// The cursor-based iteration command a page is being fetched for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanVerb {
    /// `SCAN`: iterate the keyspace.
    Scan,
    /// `SSCAN`: iterate a set's members.
    Sscan,
    /// `HSCAN`: iterate a hash's field/value pairs.
    Hscan,
    /// `ZSCAN`: iterate a sorted set's member/score pairs.
    Zscan,
}

impl ScanVerb {
    /// Renders the wire verb for this command.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ScanVerb::Scan => "SCAN",
            ScanVerb::Sscan => "SSCAN",
            ScanVerb::Hscan => "HSCAN",
            ScanVerb::Zscan => "ZSCAN",
        }
    }
}

/// Builds one SCAN family page frame.
///
/// The frame is always `<VERB> [<key>] <cursor> MATCH <pattern> COUNT <count>
/// [TYPE <type>]`: `MATCH` and `COUNT` are always sent, the key argument is
/// present only for the collection variants (`SSCAN`/`HSCAN`/`ZSCAN`), and
/// `TYPE` trails only when the caller resolved one (valid on `SCAN` only).
#[must_use]
pub fn encode_scan_page(
    verb: ScanVerb,
    key: Option<&str>,
    cursor: u64,
    pattern: &str,
    count: u64,
    scan_type: Option<&str>,
) -> redis::Cmd {
    let mut command = redis::cmd(verb.as_str());
    if let Some(key) = key {
        command.arg(key);
    }
    command
        .arg(cursor)
        .arg("MATCH")
        .arg(pattern)
        .arg("COUNT")
        .arg(count);
    if let Some(scan_type) = scan_type {
        command.arg("TYPE").arg(scan_type);
    }
    command
}

/// Parses the two-element `[cursor, elements]` reply of a SCAN family page.
///
/// A reply-shape failure (including an element that does not decode into `T`)
/// maps to [`CamelError::Io`] naming `verb_name`, matching the repository
/// pager's `from_redis_value` usage.
pub fn parse_scan_reply<T: redis::FromRedisValue>(
    reply: redis::Value,
    verb_name: &str,
) -> Result<(u64, Vec<T>), CamelError> {
    redis::from_redis_value::<(u64, Vec<T>)>(reply)
        .map_err(|e| CamelError::Io(format!("{verb_name} reply parse: {e}")))
}

/// Resolves the request cursor for a SCAN family page.
///
/// An absent header, a JSON `null`, an all-whitespace string, or `0` starts a
/// fresh iteration. Every other value goes through the shared numeric
/// resolver, so surrounding whitespace trims (`" 17 "` is cursor `17`) and a
/// malformed value is a named pre-send error naming `CamelRedis.Cursor`.
pub fn resolve_scan_cursor(exchange: &Exchange) -> Result<u64, CamelError> {
    const HEADER: &str = "CamelRedis.Cursor";
    match exchange.input.header(HEADER) {
        None | Some(serde_json::Value::Null) => Ok(0),
        Some(serde_json::Value::String(text)) if text.trim().is_empty() => Ok(0),
        // The header is present here, so `get_u64_header` reports a value or
        // a named error — never `None`. A silent `0` fallback would restart
        // the iteration if that contract ever loosened, so the impossible
        // case is loud instead (ADR-0032: fail closed).
        Some(_) => match get_u64_header(exchange, HEADER)? {
            Some(cursor) => Ok(cursor),
            None => unreachable!("get_u64_header returns None only for an absent header"),
        },
    }
}

/// Resolves the shared page options `(pattern, count, type)`.
///
/// `pattern` comes from `CamelRedis.Pattern` (default `"*"`, the `KEYS`
/// precedent), `count` from `CamelRedis.Count` (default `SCAN_PAGE_COUNT`,
/// must be positive), and `type` from `CamelRedis.Type` (valid on `SCAN` only,
/// so `allow_type` is `false` for the collection variants).
///
/// `CamelRedis.Type` presence is checked before its shape: a present value on
/// a collection variant is a named error even when it is not a string, so it
/// never silently vanishes. On `SCAN` the value must be a string; any other
/// JSON type is a named error carrying a bounded value preview.
pub fn resolve_scan_options(
    exchange: &Exchange,
    allow_type: bool,
) -> Result<(String, u64, Option<String>), CamelError> {
    const COUNT_HEADER: &str = "CamelRedis.Count";
    const TYPE_HEADER: &str = "CamelRedis.Type";
    let pattern = resolve_keys_pattern(exchange);
    let count = match get_u64_header(exchange, COUNT_HEADER)? {
        Some(0) => {
            return Err(CamelError::ProcessorError(format!(
                "Invalid value for header {COUNT_HEADER}: expected a positive integer, got 0"
            )));
        }
        Some(count) => count,
        None => SCAN_PAGE_COUNT,
    };
    let scan_type = match exchange.input.header(TYPE_HEADER) {
        None => None,
        Some(_) if !allow_type => {
            return Err(CamelError::ProcessorError(format!(
                "{TYPE_HEADER} is valid on SCAN only; remove it from SSCAN/HSCAN/ZSCAN"
            )));
        }
        Some(value) => match value.as_str() {
            Some(text) => Some(text.to_string()),
            None => {
                return Err(CamelError::ProcessorError(format!(
                    "Invalid value for header {TYPE_HEADER}: expected a string, got {}",
                    invalid_header_value_preview(value)
                )));
            }
        },
    };
    Ok((pattern, count, scan_type))
}

/// Resolves the [`ScanVerb`] a command maps to, plus whether
/// `CamelRedis.Type` is valid for it (`SCAN` only).
fn scan_verb(cmd: &RedisCommand) -> Option<(ScanVerb, bool)> {
    match cmd {
        RedisCommand::Scan => Some((ScanVerb::Scan, true)),
        RedisCommand::Sscan => Some((ScanVerb::Sscan, false)),
        RedisCommand::Hscan => Some((ScanVerb::Hscan, false)),
        RedisCommand::Zscan => Some((ScanVerb::Zscan, false)),
        _ => None,
    }
}

/// Fetches exactly one SCAN family page and assembles the reply exchange.
///
/// The request cursor and options come from the exchange headers
/// ([`resolve_scan_cursor`] / [`resolve_scan_options`]); the collection
/// variants additionally require `CamelRedis.Key`. The wire frame is built by
/// [`encode_scan_page`] — the single builder for this family — and the reply
/// is decoded per verb into the crate's existing body shapes. The next cursor
/// always lands on `CamelRedis.Cursor`, and `CamelRedis.ScanComplete=true` is
/// added only on the final page (`cursor == 0`); the header is absent
/// otherwise.
pub async fn dispatch(
    cmd: &RedisCommand,
    conn: &mut MultiplexedConnection,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let Some((verb, allow_type)) = scan_verb(cmd) else {
        return Err(CamelError::ProcessorError(
            "Not a SCAN family command".into(),
        ));
    };

    let cursor = resolve_scan_cursor(exchange)?;
    let (pattern, count, scan_type) = resolve_scan_options(exchange, allow_type)?;
    let key = match verb {
        ScanVerb::Scan => None,
        ScanVerb::Sscan | ScanVerb::Hscan | ScanVerb::Zscan => Some(super::require_key(exchange)?),
    };

    let reply = encode_scan_page(
        verb,
        key.as_deref(),
        cursor,
        &pattern,
        count,
        scan_type.as_deref(),
    )
    .query_async(conn)
    .await
    .map_err(|e| crate::transport_error::redis_error_to_camel(verb.as_str(), e))?;

    let (next_cursor, body) = match verb {
        // SCAN and SSCAN reply with a flat member array: the `KEYS` body shape.
        ScanVerb::Scan | ScanVerb::Sscan => {
            let (next, elements) = parse_scan_reply::<String>(reply, verb.as_str())?;
            (next, serde_json::json!(elements))
        }
        // HSCAN decodes into the `HGETALL` body shape.
        ScanVerb::Hscan => {
            let (next, elements) = parse_scan_reply::<(String, String)>(reply, verb.as_str())?;
            let map: HashMap<String, String> = elements.into_iter().collect();
            (next, serde_json::json!(map))
        }
        // ZSCAN decodes into the `ZRANGE ... WITHSCORES` body shape.
        ScanVerb::Zscan => {
            let (next, elements) = parse_scan_reply::<(String, f64)>(reply, verb.as_str())?;
            (next, super::zset::json_from_scored_members(elements))
        }
    };

    exchange.input.body = Body::Json(body);
    exchange
        .input
        .set_header("CamelRedis.Cursor", serde_json::json!(next_cursor));
    if next_cursor == 0 {
        exchange
            .input
            .set_header("CamelRedis.ScanComplete", serde_json::json!(true));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::{Exchange, Message};

    fn ex_with(headers: &[(&str, serde_json::Value)]) -> Exchange {
        let mut msg = Message::default();
        for (k, v) in headers {
            msg.set_header(*k, v.clone());
        }
        Exchange::new(msg)
    }

    /// Flattened command frame: every `Simple` argument's bytes in order, any
    /// other argument variant as an empty entry. Mirrors the repository
    /// crate's `test_support::cmd_args`.
    fn cmd_args(cmd: &redis::Cmd) -> Vec<Vec<u8>> {
        cmd.args_iter()
            .map(|arg| match arg {
                redis::Arg::Simple(bytes) => bytes.to_vec(),
                redis::Arg::Cursor => Vec::new(),
                _ => Vec::new(),
            })
            .collect()
    }

    fn expected_args(items: &[&str]) -> Vec<Vec<u8>> {
        items.iter().map(|item| item.as_bytes().to_vec()).collect()
    }

    fn bulk(value: &str) -> redis::Value {
        redis::Value::BulkString(value.as_bytes().to_vec())
    }

    fn scan_reply(elements: redis::Value) -> redis::Value {
        redis::Value::Array(vec![bulk("0"), elements])
    }

    #[test]
    fn encode_scan_page_scan_start_frame() {
        let cmd = encode_scan_page(ScanVerb::Scan, None, 0, "*", 100, None);
        assert_eq!(
            cmd_args(&cmd),
            expected_args(&["SCAN", "0", "MATCH", "*", "COUNT", "100"])
        );
    }

    #[test]
    fn encode_scan_page_sscan_with_all_options() {
        let cmd = encode_scan_page(ScanVerb::Sscan, Some("s"), 0, "*", 100, None);
        assert_eq!(
            cmd_args(&cmd),
            expected_args(&["SSCAN", "s", "0", "MATCH", "*", "COUNT", "100"])
        );
    }

    #[test]
    fn encode_scan_page_scan_explicit_match_count() {
        let cmd = encode_scan_page(ScanVerb::Scan, None, 17, "a*", 500, None);
        assert_eq!(
            cmd_args(&cmd),
            expected_args(&["SCAN", "17", "MATCH", "a*", "COUNT", "500"])
        );
    }

    #[test]
    fn encode_scan_page_scan_type_trailing() {
        let cmd = encode_scan_page(ScanVerb::Scan, None, 0, "*", 100, Some("set"));
        let args = cmd_args(&cmd);
        assert_eq!(
            args,
            expected_args(&["SCAN", "0", "MATCH", "*", "COUNT", "100", "TYPE", "set"])
        );
    }

    #[test]
    fn parse_scan_reply_keys_page() {
        let reply = scan_reply(redis::Value::Array(vec![bulk("a"), bulk("b")]));
        let (cursor, keys) =
            parse_scan_reply::<String>(reply, "SCAN").expect("a key page must parse");
        assert_eq!(cursor, 0);
        assert_eq!(keys, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn parse_scan_reply_hscan_flat_pairs() {
        let reply = scan_reply(redis::Value::Array(vec![
            bulk("f1"),
            bulk("v1"),
            bulk("f2"),
            bulk("v2"),
        ]));
        let (_, fields) =
            parse_scan_reply::<(String, String)>(reply, "HSCAN").expect("a hash page must parse");
        assert_eq!(
            fields,
            vec![
                ("f1".to_string(), "v1".to_string()),
                ("f2".to_string(), "v2".to_string())
            ]
        );
    }

    #[test]
    fn parse_scan_reply_zscan_pairs() {
        let reply = scan_reply(redis::Value::Array(vec![
            bulk("m1"),
            bulk("1.5"),
            bulk("m2"),
            bulk("2"),
        ]));
        let (_, members) = parse_scan_reply::<(String, f64)>(reply, "ZSCAN")
            .expect("a sorted-set page must parse");
        assert_eq!(
            members,
            vec![("m1".to_string(), 1.5), ("m2".to_string(), 2.0)]
        );
    }

    #[test]
    fn parse_scan_reply_malformed_is_named_error() {
        let err = parse_scan_reply::<String>(redis::Value::Int(1), "SCAN")
            .expect_err("a non-array reply must fail");
        let CamelError::Io(msg) = &err else {
            panic!("a reply-shape failure must be Io, got {err:?}");
        };
        assert!(msg.contains("SCAN"), "error must name the verb: {msg}");
    }

    #[test]
    fn parse_scan_reply_empty_page() {
        let reply = redis::Value::Array(vec![bulk("918"), redis::Value::Array(Vec::new())]);
        let (cursor, elements) =
            parse_scan_reply::<String>(reply, "SCAN").expect("an empty page must parse");
        assert_eq!(cursor, 918);
        assert!(elements.is_empty());
    }

    #[test]
    fn resolve_scan_cursor_absent_null_empty_and_zero() {
        for ex in [
            Exchange::new(Message::default()),
            ex_with(&[("CamelRedis.Cursor", serde_json::json!(null))]),
            ex_with(&[("CamelRedis.Cursor", serde_json::json!(""))]),
            ex_with(&[("CamelRedis.Cursor", serde_json::json!("  "))]),
            ex_with(&[("CamelRedis.Cursor", serde_json::json!(0))]),
        ] {
            assert_eq!(
                resolve_scan_cursor(&ex).expect("an empty cursor starts a fresh iteration"),
                0
            );
        }
    }

    #[test]
    fn resolve_scan_cursor_trims_and_parses() {
        let trimmed = ex_with(&[("CamelRedis.Cursor", serde_json::json!(" 17 "))]);
        assert_eq!(
            resolve_scan_cursor(&trimmed).expect("a trimmed numeric cursor must resolve"),
            17
        );

        let numeric = ex_with(&[("CamelRedis.Cursor", serde_json::json!(918))]);
        assert_eq!(
            resolve_scan_cursor(&numeric).expect("a numeric cursor must resolve"),
            918
        );
    }

    #[test]
    fn resolve_scan_cursor_malformed_names_header() {
        for value in [serde_json::json!("abc"), serde_json::json!(-1)] {
            let ex = ex_with(&[("CamelRedis.Cursor", value.clone())]);
            let err =
                resolve_scan_cursor(&ex).expect_err("a malformed cursor must fail before send");
            let msg = err.to_string();
            assert!(msg.contains("CamelRedis.Cursor"), "{value}: {msg}");
        }
    }

    #[test]
    fn resolve_scan_options_defaults() {
        let ex = Exchange::new(Message::default());
        assert_eq!(
            resolve_scan_options(&ex, false).expect("absent options must default"),
            ("*".to_string(), 100, None)
        );
    }

    #[test]
    fn resolve_scan_options_count_zero_and_malformed_named() {
        for value in [serde_json::json!(0), serde_json::json!("abc")] {
            let ex = ex_with(&[("CamelRedis.Count", value.clone())]);
            let err = resolve_scan_options(&ex, false)
                .expect_err("an invalid count must fail before send");
            let msg = err.to_string();
            assert!(msg.contains("CamelRedis.Count"), "{value}: {msg}");
        }
    }

    #[test]
    fn resolve_scan_options_type_rejected_on_collection_variants() {
        let present = ex_with(&[("CamelRedis.Type", serde_json::json!("hash"))]);
        let err = resolve_scan_options(&present, false)
            .expect_err("TYPE on a collection variant must fail");
        assert!(err.to_string().contains("CamelRedis.Type"), "{err}");

        let non_string = ex_with(&[("CamelRedis.Type", serde_json::json!(true))]);
        let err = resolve_scan_options(&non_string, false)
            .expect_err("TYPE presence must fire regardless of stringness");
        assert!(err.to_string().contains("CamelRedis.Type"), "{err}");

        let err = resolve_scan_options(&non_string, true)
            .expect_err("a non-string TYPE must fail strict resolution");
        assert!(err.to_string().contains("CamelRedis.Type"), "{err}");

        let accepted = ex_with(&[("CamelRedis.Type", serde_json::json!("set"))]);
        assert_eq!(
            resolve_scan_options(&accepted, true).expect("a string TYPE on SCAN must resolve"),
            ("*".to_string(), 100, Some("set".to_string()))
        );
    }

    #[test]
    fn scan_collection_variants_require_key_header() {
        let ex = ex_with(&[("CamelRedis.Cursor", serde_json::json!(5))]);
        let err = crate::commands::require_key(&ex)
            .expect_err("SSCAN/HSCAN/ZSCAN need the CamelRedis.Key header");
        assert!(err.to_string().contains("CamelRedis.Key"), "{err}");
    }

    #[test]
    fn scan_type_rejected_on_hscan_before_send() {
        let ex = ex_with(&[
            ("CamelRedis.Key", serde_json::json!("h")),
            ("CamelRedis.Type", serde_json::json!("hash")),
        ]);
        let err =
            resolve_scan_options(&ex, false).expect_err("TYPE on HSCAN must fail before send");
        assert!(err.to_string().contains("CamelRedis.Type"), "{err}");
    }
}
