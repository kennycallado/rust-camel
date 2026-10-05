//! The `redis` validate-target grammar (redis-state-tier task 1).
//!
//! A redis target is `{redis: {datasource, key, type, ttl?}}`. The
//! `datasource` and `key` are document-authored literals (the
//! identifier law); `type` is one of `string`, `hash`, `list`, `set`,
//! `zset`; `ttl` is an optional whole-millisecond count bound. The
//! expectation reuses the sql row grammar with the type's inherent
//! schema applied at load time.

use camel_api::Value;
use camel_matchers::{CountBound, RowsExpectation};
use noyalib::compat::serde_yaml;
use serde::Deserialize;

use super::DocError;

/// The redis value type a target asserts (redis-state-tier task 1):
/// one of `string`, `hash`, `list`, `set`, or `zset`. The type is
/// declared, never inferred, and its inherent schema drives the
/// load-time projection law.
///
/// `#[non_exhaustive]` matches the ADR-0049 posture for public enums:
/// the family may grow a type without breaking downstream matches.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedisType {
    /// A `GET` string value.
    String,
    /// A hash (`HGETALL`) field/value set.
    Hash,
    /// A list (`LRANGE`) index/value sequence.
    List,
    /// A set (`SMEMBERS`) member set.
    Set,
    /// A sorted set (`ZRANGE ... WITHSCORES`) member/score sequence.
    Zset,
}

impl RedisType {
    /// The inherent projection schema: the column names a snapshot of
    /// this type projects, in canonical order.
    pub fn schema(self) -> &'static [&'static str] {
        match self {
            RedisType::String => &["value"],
            RedisType::Hash => &["field", "value"],
            RedisType::List => &["index", "value"],
            RedisType::Set => &["member"],
            RedisType::Zset => &["member", "score"],
        }
    }

    /// The lowercase name the grammar and diagnostics render.
    pub fn as_str(self) -> &'static str {
        match self {
            RedisType::String => "string",
            RedisType::Hash => "hash",
            RedisType::List => "list",
            RedisType::Set => "set",
            RedisType::Zset => "zset",
        }
    }

    /// Parses a declared type name; `None` for an unrecognized token.
    pub fn from_name(name: &str) -> Option<RedisType> {
        match name {
            "string" => Some(RedisType::String),
            "hash" => Some(RedisType::Hash),
            "list" => Some(RedisType::List),
            "set" => Some(RedisType::Set),
            "zset" => Some(RedisType::Zset),
            _ => None,
        }
    }
}

/// The redis `validate` target payload (redis-state-tier task 1): a
/// key-level assertion against a configured datasource. `datasource`
/// and `key` are document-authored literals (the identifier law,
/// never env-interpolated); `type` selects the inherent projection;
/// `ttl` is an optional whole-millisecond count bound over the
/// remaining time to live.
#[derive(Debug, Clone, PartialEq)]
pub struct RedisTarget {
    /// The datasource name as declared under `[datasources.*]`.
    pub datasource: String,
    /// The document-authored key literal.
    pub key: String,
    /// The declared value type; selects the projection schema.
    pub r#type: RedisType,
    /// The optional whole-millisecond TTL bound.
    pub ttl: Option<CountBound>,
}

/// The raw `redis` target node: `type` is a string token validated
/// against [`RedisType`], `ttl` a nested bound node.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawRedisTarget {
    datasource: String,
    key: String,
    r#type: String,
    ttl: Option<RawRedisTtl>,
}

/// The raw `ttl` node: at least one of `atLeast` / `atMost`, each a
/// humantime duration string.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawRedisTtl {
    at_least: Option<String>,
    at_most: Option<String>,
}

/// Builds a [`RedisTarget`] from the raw `redis` node (redis-state-tier
/// task 1): unknown field names the action index; an empty
/// `datasource` or `key` names the field; an unrecognized `type` names
/// the action index and the token; `ttl` loads through
/// [`parse_redis_ttl`].
pub(crate) fn redis_target_from_value(
    content: &serde_yaml::Value,
    index: usize,
) -> Result<RedisTarget, DocError> {
    let invalid = |message: String| DocError::Validation { index, message };
    let raw: RawRedisTarget =
        serde_yaml::from_value(content.clone()).map_err(|e| invalid(e.to_string()))?;
    if raw.datasource.is_empty() {
        return Err(invalid(
            "validate `redis` target requires a non-empty `datasource`".to_string(),
        ));
    }
    if raw.key.is_empty() {
        return Err(invalid(
            "validate `redis` target requires a non-empty `key`".to_string(),
        ));
    }
    let r#type = RedisType::from_name(&raw.r#type).ok_or_else(|| {
        invalid(format!(
            "validate `redis` target: unknown type `{}`; expected `string`, `hash`, `list`, \
             `set`, or `zset`",
            raw.r#type
        ))
    })?;
    let ttl = match &raw.ttl {
        Some(raw_ttl) => Some(parse_redis_ttl(raw_ttl, index)?),
        None => None,
    };
    Ok(RedisTarget {
        datasource: raw.datasource,
        key: raw.key,
        r#type,
        ttl,
    })
}

/// Parses the optional `ttl` node (redis-state-tier task 1): an empty
/// map is an error naming the action index; each humantime string must
/// be a positive whole-millisecond duration (zero and sub-millisecond
/// are load errors, never truncated); the millisecond value converts
/// through a checked `u128` -> `u64` (overflow is a load error, never
/// wrapping); `atLeast > atMost` is rejected. At least one bound is an
/// [`CountBound::AtLeast`], [`CountBound::AtMost`], or
/// [`CountBound::Range`].
fn parse_redis_ttl(raw: &RawRedisTtl, index: usize) -> Result<CountBound, DocError> {
    let invalid = |message: String| DocError::Validation { index, message };
    if raw.at_least.is_none() && raw.at_most.is_none() {
        return Err(invalid(
            "validate `redis` target: `ttl` requires at least one of `atLeast` or `atMost`"
                .to_string(),
        ));
    }
    fn parse_ms(
        field: &str,
        text: &str,
        invalid: &impl Fn(String) -> DocError,
    ) -> Result<u64, DocError> {
        let duration = humantime::parse_duration(text).map_err(|e| {
            invalid(format!(
                "validate `redis` target: `{field}` must be a humantime duration: {e}"
            ))
        })?;
        let nanos = duration.as_nanos();
        if nanos == 0 || nanos % 1_000_000 != 0 {
            return Err(invalid(format!(
                "validate `redis` target: `{field}` must be a positive whole-millisecond \
                 duration, got `{text}`"
            )));
        }
        u64::try_from(duration.as_millis()).map_err(|_| {
            invalid(format!(
                "validate `redis` target: `{field}` overflows whole milliseconds"
            ))
        })
    }
    let at_least = raw
        .at_least
        .as_deref()
        .map(|text| parse_ms("atLeast", text, &invalid))
        .transpose()?;
    let at_most = raw
        .at_most
        .as_deref()
        .map(|text| parse_ms("atMost", text, &invalid))
        .transpose()?;
    match (at_least, at_most) {
        (Some(min), Some(max)) => {
            if min > max {
                return Err(invalid(format!(
                    "validate `redis` target: `atLeast` ({min}ms) must not exceed `atMost` \
                     ({max}ms)"
                )));
            }
            Ok(CountBound::Range(min, max))
        }
        (Some(n), None) => Ok(CountBound::AtLeast(n)),
        (None, Some(n)) => Ok(CountBound::AtMost(n)),
        (None, None) => Err(invalid(
            "validate `redis` target: `ttl` requires at least one of `atLeast` or `atMost`"
                .to_string(),
        )),
    }
}

/// Applies the redis expectation grammar (redis-state-tier task 1):
/// the shared sql row grammar with the declared type's inherent schema
/// applied at load time. `columns`, when declared, SHALL be a subset
/// of `schema`; every `rows` row SHALL carry exactly as many cells as
/// the effective projection (the declared `columns`, or the full
/// `schema` when absent) — the redis schema is inherent, so the width
/// is enforced at load whether or not `columns` is declared.
pub(crate) fn redis_expectation_from_value(
    value: &Value,
    index: usize,
    schema: &[&str],
) -> Result<RowsExpectation, DocError> {
    let invalid = |message: String| DocError::Validation { index, message };
    let expectation = super::validate::sql_expectation_from_value(value, index)?;
    if let Some(columns) = &expectation.columns {
        for name in columns {
            if !schema.contains(&name.as_str()) {
                return Err(invalid(format!(
                    "redis expectation: unknown projection column `{name}`; the declared type \
                     schema is [{}]",
                    schema.join(", ")
                )));
            }
        }
    }
    let effective: Vec<&str> = match &expectation.columns {
        Some(columns) => columns.iter().map(String::as_str).collect(),
        None => schema.to_vec(),
    };
    if let Some(rows) = &expectation.rows {
        for (row_index, row) in rows.iter().enumerate() {
            if row.len() != effective.len() {
                return Err(invalid(format!(
                    "redis expectation: row {row_index} declares {} cells but the effective \
                     projection names {}; the widths must match",
                    row.len(),
                    effective.len()
                )));
            }
        }
    }
    Ok(expectation)
}

#[cfg(test)]
mod tests {
    use crate::{CountBound, DocError, ScenarioAction, ScenarioTarget, parse_scenario_document};

    fn parse_case(text: &str) -> Result<crate::ScenarioDocument, DocError> {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("case.test.yaml");
        std::fs::write(&path, text).expect("write case file");
        parse_scenario_document(&path)
    }

    fn parse_redis_ttl(text: &str) -> Option<CountBound> {
        let doc = parse_case(text).expect("parse must succeed");
        match doc.scenario.first().expect("one action") {
            ScenarioAction::Validate {
                target: ScenarioTarget::Redis(target),
                ..
            } => target.ttl.clone(),
            other => panic!("expected redis target, got {other:?}"),
        }
    }

    fn assert_validation_index(err: DocError) {
        match err {
            DocError::Validation { index, .. } => {
                assert_eq!(index, 0, "error must name the action index");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    fn assert_validation_naming(err: DocError, needle: &str) {
        match err {
            DocError::Validation { index, message } => {
                assert_eq!(index, 0, "error must name the action index");
                assert!(
                    message.contains(needle),
                    "error must name `{needle}`: {message}"
                );
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn redis_missing_datasource_key_or_type_is_load_error() {
        let cases = [
            (
                r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      redis:
        key: "user:1"
        type: hash
    expectation:
      count: 1
"#,
                "datasource",
            ),
            (
                r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      redis:
        datasource: statedb
        type: hash
    expectation:
      count: 1
"#,
                "key",
            ),
            (
                r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      redis:
        datasource: statedb
        key: "user:1"
    expectation:
      count: 1
"#,
                "type",
            ),
        ];
        for (text, field) in cases {
            let err = parse_case(text).expect_err("parse must fail");
            assert_validation_naming(err, field);
        }
    }

    #[test]
    fn redis_unknown_type_is_load_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      redis:
        datasource: statedb
        key: "user:1"
        type: stream
    expectation:
      count: 1
"#,
        )
        .expect_err("parse must fail");
        assert_validation_naming(err, "stream");
    }

    #[test]
    fn redis_unknown_projection_column_is_load_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      redis:
        datasource: statedb
        key: "user:1"
        type: hash
    expectation:
      columns: [field, missing]
      rows: [["a", "b"]]
"#,
        )
        .expect_err("parse must fail");
        assert_validation_naming(err, "missing");
    }

    #[test]
    fn redis_row_length_mismatch_without_columns_is_load_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      redis:
        datasource: statedb
        key: "k"
        type: list
    expectation:
      rows: [[0, "a", "b"]]
"#,
        )
        .expect_err("parse must fail");
        assert_validation_naming(err, "row 0");
    }

    #[test]
    fn redis_row_length_mismatch_with_columns_is_load_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      redis:
        datasource: statedb
        key: "k"
        type: zset
    expectation:
      columns: [member]
      rows: [["a", 1]]
"#,
        )
        .expect_err("parse must fail");
        assert_validation_naming(err, "row 0");
    }

    #[test]
    fn redis_empty_ttl_bound_is_load_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      redis:
        datasource: statedb
        key: "k"
        type: string
        ttl: {}
    expectation:
      count: 1
"#,
        )
        .expect_err("parse must fail");
        assert_validation_index(err);
    }

    #[test]
    fn redis_inverted_ttl_bound_is_load_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      redis:
        datasource: statedb
        key: "k"
        type: string
        ttl: {atLeast: 60s, atMost: 30s}
    expectation:
      count: 1
"#,
        )
        .expect_err("parse must fail");
        assert_validation_naming(err, "atLeast");
    }

    #[test]
    fn redis_sub_millisecond_ttl_bound_is_load_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      redis:
        datasource: statedb
        key: "k"
        type: string
        ttl: {atLeast: 500us}
    expectation:
      count: 1
"#,
        )
        .expect_err("parse must fail");
        assert_validation_index(err);
    }

    #[test]
    fn redis_zero_ttl_bound_is_load_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      redis:
        datasource: statedb
        key: "k"
        type: string
        ttl: {atMost: 0s}
    expectation:
      count: 1
"#,
        )
        .expect_err("parse must fail");
        assert_validation_index(err);
    }

    #[test]
    fn redis_overflowing_ttl_bound_is_load_error() {
        let err = parse_case(
            r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      redis:
        datasource: statedb
        key: "k"
        type: string
        ttl: {atLeast: 60000000000000000s}
    expectation:
      count: 1
"#,
        )
        .expect_err("parse must fail");
        assert_validation_index(err);
    }

    #[test]
    fn redis_ttl_bounds_parse_to_count_bound() {
        let doc = r#"
routeFiles: [routes.yaml]
scenario:
- validate:
    target:
      redis:
        datasource: statedb
        key: "k"
        type: string
        ttl: {%s}
    expectation:
      count: 1
"#;
        assert_eq!(
            parse_redis_ttl(&doc.replace("%s", "atLeast: 30s")),
            Some(CountBound::AtLeast(30_000))
        );
        assert_eq!(
            parse_redis_ttl(&doc.replace("%s", "atMost: 60s")),
            Some(CountBound::AtMost(60_000))
        );
        assert_eq!(
            parse_redis_ttl(&doc.replace("%s", "atLeast: 1ms, atMost: 60s")),
            Some(CountBound::Range(1, 60_000))
        );
    }
}
