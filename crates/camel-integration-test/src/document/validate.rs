//! The `validate` grammar: what a `validate` action asserts against
//! (target), the paired expectation shapes, and the expectation
//! parsers (bd rc-m6xr, split out of the parent module; mirrors the
//! `document/error.rs` and `partner_script.rs` patterns).
//!
//! `ScenarioTarget`, `SqlTarget`, and `ValidateExpectation` are
//! re-exported at `crate::document` and the crate root, so consumers
//! keep the paths they had before the split. `PartnerExpectation`
//! stays the `camel_matchers::RequestExpectation` alias.

use std::collections::BTreeMap;

use camel_api::Value;
use camel_matchers::{CountBound, Expectation, PathFilter, RowsExpectation};

use super::{DocError, EndpointRef, PartnerExpectation};

/// What a `validate` action asserts against.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ScenarioTarget {
    /// The last message received on the endpoint.
    LastReceived(EndpointRef),
    /// A scenario variable set by an earlier `extract`. Variable
    /// existence is validated at run time.
    Variable(String),
    /// A partner endpoint: the assertion reads the partner's recorded
    /// request traffic. The URI must equal a harness endpoint
    /// reference declared by the scenario's own `send`/`receive`
    /// actions, or self-declare the reference: an object form with
    /// `provisioning: harness` on an `http` URI that also has a `partners:`
    /// entry naming it.
    Partner(EndpointRef),
    /// A named datasource: the assertion executes the doc-authored
    /// read and validates the returned rows. Reads only — the `sql:`
    /// prepare action owns mutations, and the two vocabularies never
    /// mix (bd rc-25lup.2).
    Sql(SqlTarget),
}

/// The sql `validate` target payload (bd rc-25lup.2): a read against a
/// configured datasource. The datasource obeys the identifier law: it
/// names an entry under `[datasources.*]` in `Camel.toml` and is never
/// interpolated. The query is doc-authored read text; every statement
/// that fails [`crate::sql_action::is_read_statement`] is rejected at
/// load — the `sql:` prepare action owns mutations.
#[derive(Debug, Clone, PartialEq)]
pub struct SqlTarget {
    /// The datasource name as declared under `[datasources.*]`.
    pub datasource: String,
    /// The read query executed against the datasource's pool.
    pub query: String,
}

/// The expectation of a `validate` action, keyed by its target: the
/// message matcher grammar for `lastReceived` and `variable` targets,
/// the partner count grammar for `partner` targets, the sql-target
/// row shape for `sql` targets.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ValidateExpectation {
    /// Message matcher expectation (`lastReceived` / `variable`).
    Message(Expectation),
    /// Partner request-count expectation (`partner`).
    Partner(PartnerExpectation),
    /// The sql-target row shape: concrete row patterns (`rows`) or a
    /// row-count bound (`bound`), with an optional named projection
    /// and order flag. `Message` and `Partner` unchanged.
    Rows(RowsExpectation),
}

/// Recognized expectation matcher keys.
fn is_matcher_key(key: &str) -> bool {
    matches!(
        key,
        "equals"
            | "regex"
            | "contains"
            | "startsWith"
            | "endsWith"
            | "exists"
            | "ignore"
            | "jsonSubset"
    )
}

/// Applies the expectation dual grammar: a bare value is a literal
/// `equals`; an object whose single key is a recognized matcher key is
/// that matcher; any other object is a literal `equals`. Payload shapes
/// mirror the mock-testkit matcher rules. The field name parameter
/// (`expectation`, `expectReply`) keeps one verb parser behind both
/// readers (rc-qvz6): the verbs never fork between `validate` and
/// send-level reply assertions.
pub(crate) fn expectation_from_value(
    value: &Value,
    index: usize,
    field: &'static str,
) -> Result<Expectation, DocError> {
    let invalid = |message: String| DocError::Validation { index, message };
    if let Value::Object(map) = value
        && map.len() == 1
        && let Some((key, payload)) = map.iter().next()
        && is_matcher_key(key)
    {
        return match key.as_str() {
            "equals" => Ok(Expectation::Equals(payload.clone())),
            "regex" | "contains" | "startsWith" | "endsWith" => {
                let Some(pattern) = payload.as_str() else {
                    return Err(invalid(format!(
                        "{field}: `{key}` requires a string payload"
                    )));
                };
                if key.as_str() == "regex"
                    && let Err(e) = regex::Regex::new(pattern)
                {
                    return Err(invalid(format!("{field}: invalid regex `{pattern}`: {e}")));
                }
                Ok(match key.as_str() {
                    "regex" => Expectation::Regex(pattern.to_string()),
                    "contains" => Expectation::Contains(pattern.to_string()),
                    "startsWith" => Expectation::StartsWith(pattern.to_string()),
                    _ => Expectation::EndsWith(pattern.to_string()),
                })
            }
            "exists" => {
                if payload.is_null() {
                    Ok(Expectation::Exists)
                } else {
                    Err(invalid(format!("{field}: `exists` takes no argument")))
                }
            }
            "ignore" => {
                if payload.is_null() {
                    Ok(Expectation::Any)
                } else {
                    Err(invalid(format!("{field}: `ignore` takes no argument")))
                }
            }
            _ => {
                if payload.is_object() {
                    Ok(Expectation::JsonSubset(payload.clone()))
                } else {
                    Err(invalid(format!("{field}: `jsonSubset` must be an object")))
                }
            }
        };
    }
    Ok(Expectation::Equals(value.clone()))
}

/// Applies the partner expectation grammar: a map with exactly one
/// count bound (`count`; or `atLeast`, `atMost`, or their range), an
/// optional `method` string, at most one path filter (`path`,
/// `pathContains`, `pathMatches` — the regex compiled at load), and
/// an optional `query` subset map of string keys to string values;
/// unknown keys fail. Field-by-field extraction, like the
/// endpoint-reference reader, so errors name the offending key.
pub(super) fn partner_expectation_from_value(
    value: &Value,
    index: usize,
) -> Result<PartnerExpectation, DocError> {
    const FIELD: &str = "partner expectation";
    const KEYS: &[&str] = &[
        "count",
        "atLeast",
        "atMost",
        "method",
        "path",
        "pathContains",
        "pathMatches",
        "query",
    ];
    let invalid = |message: String| DocError::Validation { index, message };
    let Value::Object(map) = value else {
        return Err(invalid(format!(
            "{FIELD} must be a map with a count bound, got {value:?}"
        )));
    };
    let mut count: Option<u64> = None;
    let mut at_least: Option<u64> = None;
    let mut at_most: Option<u64> = None;
    let mut method: Option<String> = None;
    let mut path: Option<PathFilter> = None;
    let mut path_key: Option<&str> = None;
    let mut query: Option<BTreeMap<String, String>> = None;
    for (key, payload) in map {
        match key.as_str() {
            "count" | "atLeast" | "atMost" => {
                let bound = payload.as_u64().ok_or_else(|| {
                    invalid(format!(
                        "{FIELD}: `{key}` must be a non-negative integer, got {payload}"
                    ))
                })?;
                match key.as_str() {
                    "count" => count = Some(bound),
                    "atLeast" => at_least = Some(bound),
                    _ => at_most = Some(bound),
                }
            }
            "method" => {
                let text = payload.as_str().ok_or_else(|| {
                    invalid(format!("{FIELD}: `{key}` must be a string, got {payload}"))
                })?;
                method = Some(text.to_string());
            }
            "path" | "pathContains" | "pathMatches" => {
                if let Some(first) = path_key {
                    return Err(invalid(format!(
                        "{FIELD}: `{first}` and `{key}` are exclusive: at most one path filter"
                    )));
                }
                let text = payload.as_str().ok_or_else(|| {
                    invalid(format!("{FIELD}: `{key}` must be a string, got {payload}"))
                })?;
                path = Some(match key.as_str() {
                    "path" => PathFilter::Exact(text.to_string()),
                    "pathContains" => PathFilter::Contains(text.to_string()),
                    _ => {
                        if let Err(e) = regex::Regex::new(text) {
                            return Err(invalid(format!("{FIELD}: invalid regex `{text}`: {e}")));
                        }
                        PathFilter::Matches(text.to_string())
                    }
                });
                path_key = Some(key.as_str());
            }
            "query" => {
                let Value::Object(pairs) = payload else {
                    return Err(invalid(format!(
                        "{FIELD}: `query` must be a map of string keys to string values, got {payload}"
                    )));
                };
                let mut subset = BTreeMap::new();
                for (name, pair) in pairs {
                    let Some(text) = pair.as_str() else {
                        return Err(invalid(format!(
                            "{FIELD}: `query` value for `{name}` must be a string, got {pair}"
                        )));
                    };
                    subset.insert(name.clone(), text.to_string());
                }
                query = Some(subset);
            }
            other => {
                return Err(invalid(format!(
                    "{FIELD}: unknown field `{other}`; expected {}",
                    backticked(KEYS)
                )));
            }
        }
    }
    if count.is_some() && (at_least.is_some() || at_most.is_some()) {
        let mut others: Vec<&str> = Vec::new();
        if at_least.is_some() {
            others.push("atLeast");
        }
        if at_most.is_some() {
            others.push("atMost");
        }
        return Err(invalid(format!(
            "{FIELD}: `count` and {} are exclusive: declare exactly one bound form",
            backticked(&others)
        )));
    }
    let bound = if let Some(exact) = count {
        CountBound::Exact(exact)
    } else if let (Some(min), Some(max)) = (at_least, at_most) {
        if min > max {
            return Err(invalid(format!(
                "{FIELD}: `atLeast` ({min}) must not exceed `atMost` ({max})"
            )));
        }
        CountBound::Range(min, max)
    } else if let Some(n) = at_least {
        CountBound::AtLeast(n)
    } else if let Some(n) = at_most {
        CountBound::AtMost(n)
    } else {
        return Err(invalid(format!(
            "{FIELD}: requires a count bound: `count`, `atLeast`, or `atMost`"
        )));
    };
    Ok(PartnerExpectation {
        bound,
        method,
        path,
        query,
    })
}

/// Backticks and comma-joins field names for error messages.
pub(super) fn backticked(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|field| format!("`{field}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Applies the sql-target expectation grammar (bd rc-25lup.2): either
/// concrete row patterns (`rows`, an optional `columns` projection, an
/// optional `unordered` flag) or a row-count bound (`count`, `atLeast`,
/// `atMost` — the partner count-key semantics: exactly one bound form,
/// `atLeast <= atMost` for a range) — never both. Unknown keys fail.
/// Field-by-field extraction, like the partner reader, so errors name
/// the offending key and, for row shapes, the offending row index.
pub(crate) fn sql_expectation_from_value(
    value: &Value,
    index: usize,
) -> Result<RowsExpectation, DocError> {
    const FIELD: &str = "sql expectation";
    const KEYS: &[&str] = &["rows", "columns", "unordered", "count", "atLeast", "atMost"];
    let invalid = |message: String| DocError::Validation { index, message };
    let Value::Object(map) = value else {
        return Err(invalid(format!(
            "{FIELD} must be a map with `rows` or a count bound, got {value:?}"
        )));
    };
    let mut rows: Option<Vec<Vec<Expectation>>> = None;
    let mut columns: Option<Vec<String>> = None;
    let mut unordered = false;
    let mut count: Option<u64> = None;
    let mut at_least: Option<u64> = None;
    let mut at_most: Option<u64> = None;
    for (key, payload) in map {
        match key.as_str() {
            "rows" => {
                let Value::Array(raw_rows) = payload else {
                    return Err(invalid(format!(
                        "{FIELD}: `rows` must be a sequence of rows, got {payload}"
                    )));
                };
                if raw_rows.is_empty() {
                    return Err(invalid(format!("{FIELD}: `rows` must not be empty")));
                }
                let mut parsed_rows = Vec::with_capacity(raw_rows.len());
                for (row_index, raw_row) in raw_rows.iter().enumerate() {
                    let Value::Array(cells) = raw_row else {
                        return Err(invalid(format!(
                            "{FIELD}: `rows` row {row_index} must be a sequence of cell \
                             expectations, got {raw_row}"
                        )));
                    };
                    let mut row = Vec::with_capacity(cells.len());
                    for cell in cells {
                        row.push(expectation_from_value(cell, index, "rows")?);
                    }
                    parsed_rows.push(row);
                }
                rows = Some(parsed_rows);
            }
            "columns" => {
                let Value::Array(raw_names) = payload else {
                    return Err(invalid(format!(
                        "{FIELD}: `columns` must be a sequence of column names, got {payload}"
                    )));
                };
                if raw_names.is_empty() {
                    return Err(invalid(format!("{FIELD}: `columns` must not be empty")));
                }
                let mut names = Vec::with_capacity(raw_names.len());
                for raw_name in raw_names {
                    let Some(name) = raw_name.as_str() else {
                        return Err(invalid(format!(
                            "{FIELD}: `columns` entries must be strings, got {raw_name}"
                        )));
                    };
                    if names.iter().any(|existing| existing == name) {
                        return Err(invalid(format!("{FIELD}: duplicate column name `{name}`")));
                    }
                    names.push(name.to_string());
                }
                columns = Some(names);
            }
            "unordered" => {
                let Some(flag) = payload.as_bool() else {
                    return Err(invalid(format!(
                        "{FIELD}: `unordered` must be a boolean, got {payload}"
                    )));
                };
                unordered = flag;
            }
            "count" | "atLeast" | "atMost" => {
                let bound = payload.as_u64().ok_or_else(|| {
                    invalid(format!(
                        "{FIELD}: `{key}` must be a non-negative integer, got {payload}"
                    ))
                })?;
                match key.as_str() {
                    "count" => count = Some(bound),
                    "atLeast" => at_least = Some(bound),
                    _ => at_most = Some(bound),
                }
            }
            other => {
                return Err(invalid(format!(
                    "{FIELD}: unknown field `{other}`; expected {}",
                    backticked(KEYS)
                )));
            }
        }
    }
    // Row patterns and a count bound describe different subjects (the
    // returned rows vs their number): declaring both has no meaning.
    if rows.is_some() && (count.is_some() || at_least.is_some() || at_most.is_some()) {
        let mut bound_keys: Vec<&str> = Vec::new();
        if count.is_some() {
            bound_keys.push("count");
        }
        if at_least.is_some() {
            bound_keys.push("atLeast");
        }
        if at_most.is_some() {
            bound_keys.push("atMost");
        }
        return Err(invalid(format!(
            "{FIELD}: `rows` and {} are exclusive: declare either row patterns or a row-count \
             bound",
            backticked(&bound_keys)
        )));
    }
    // The count keys themselves follow the partner exclusivity law:
    // exactly one bound form, and a range needs `atLeast <= atMost`.
    if count.is_some() && (at_least.is_some() || at_most.is_some()) {
        let mut others: Vec<&str> = Vec::new();
        if at_least.is_some() {
            others.push("atLeast");
        }
        if at_most.is_some() {
            others.push("atMost");
        }
        return Err(invalid(format!(
            "{FIELD}: `count` and {} are exclusive: declare exactly one bound form",
            backticked(&others)
        )));
    }
    let bound = if let Some(exact) = count {
        Some(CountBound::Exact(exact))
    } else if let (Some(min), Some(max)) = (at_least, at_most) {
        if min > max {
            return Err(invalid(format!(
                "{FIELD}: `atLeast` ({min}) must not exceed `atMost` ({max})"
            )));
        }
        Some(CountBound::Range(min, max))
    } else if let Some(n) = at_least {
        Some(CountBound::AtLeast(n))
    } else {
        at_most.map(CountBound::AtMost)
    };
    // An expectation that names neither shape asserts nothing and
    // almost certainly hides a typo'd key.
    if rows.is_none() && bound.is_none() {
        return Err(invalid(format!(
            "{FIELD}: requires either `rows` or a count bound: `count`, `atLeast`, or `atMost`"
        )));
    }
    // When `columns` is declared, every row must match its width: the
    // projection happens by name at the call site, so a mismatched row
    // would silently misalign. Without `columns`, widths are checked
    // at execution against the query's projection.
    if let (Some(columns), Some(rows)) = (&columns, &rows) {
        for (row_index, row) in rows.iter().enumerate() {
            if row.len() != columns.len() {
                return Err(invalid(format!(
                    "{FIELD}: row {row_index} declares {} cells but `columns` names {}; the \
                     widths must match",
                    row.len(),
                    columns.len()
                )));
            }
        }
    }
    Ok(RowsExpectation {
        columns,
        unordered,
        rows,
        bound,
    })
}

/// Whether the sql query carries no `ORDER BY` clause (bd rc-25lup.2):
/// a case-insensitive token search for `order` and `by` separated by
/// whitespace (`\s+` covers `ORDER\nBY`). A string literal containing
/// the words (`select 'totally ordered by intent' from t`) trips the
/// predicate — a documented false positive; the advisory only warns,
/// it never rejects. The pattern is static: the `is_ok_and` fallback
/// treats an impossible compile failure as "lacks", which only
/// over-warns.
pub(crate) fn sql_query_lacks_order_by(query: &str) -> bool {
    !regex::Regex::new(r"(?i)\border\s+by\b").is_ok_and(|order_by| order_by.is_match(query))
}
