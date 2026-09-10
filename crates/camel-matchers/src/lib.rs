//! Pure matcher algebra shared by the test tiers: the message
//! expectation grammar, the recorded-request count bound grammar, and
//! the pure predicates over them. The crate has zero camel
//! dependencies so unit and integration tiers can share one matcher
//! implementation (ADR-0072).

use std::collections::BTreeMap;

/// The recorded-request count bound of a [`RequestExpectation`]:
/// exactly one bound form per expectation. Poll semantics per bound
/// (arrivals only add, so the filtered count is monotone
/// non-decreasing):
///
/// - Without a deadline, one immediate snapshot decides for every
///   bound.
/// - [`CountBound::Exact`] polls until a snapshot's count equals `n`;
///   a snapshot above never passes.
/// - [`CountBound::AtLeast`] succeeds early, once the count reaches
///   `n` (sound: the count only grows).
/// - [`CountBound::AtMost`] is an absence claim over the window: it
///   waits the full deadline, fails immediately on any snapshot above
///   `n`, and decides on the final snapshot — an early passing
///   snapshot cannot prove the count stays within bounds.
/// - [`CountBound::Range`] fails immediately above the maximum and
///   otherwise waits the full deadline, deciding on the final
///   snapshot within `[min, max]`.
///
/// These semantics document a monotone subject: arrivals only add,
/// so the filtered count is non-decreasing and a reached count stays
/// reached. SQL row counts are NOT monotone — a DELETE shrinks the
/// row set — so SQL count assertions never settle early; the final
/// snapshot at the deadline decides.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum CountBound {
    /// Exactly `n` matching requests.
    Exact(u64),
    /// At least `n` matching requests (early success at `n` or more).
    AtLeast(u64),
    /// At most `n` matching requests (absence claim over the window).
    AtMost(u64),
    /// Between `min` and `max` matching requests, inclusive.
    Range(u64, u64),
}

/// The path filter of a [`RequestExpectation`] over the recorded
/// path-and-query; at most one filter per expectation.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PathFilter {
    /// Exact path-and-query match (strict bytes).
    Exact(String),
    /// Substring containment against the recorded path-and-query.
    Contains(String),
    /// Regular expression match, compile-verified at load time.
    Matches(String),
}

/// A recorded-request expectation: a count bound plus optional
/// `method`, `path`, and `query` subset filters.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestExpectation {
    /// The count bound the recorded requests must satisfy.
    pub bound: CountBound,
    /// Optional request-method filter.
    pub method: Option<String>,
    /// Optional request-path filter (path-and-query).
    pub path: Option<PathFilter>,
    /// Optional query subset filter: every declared pair must be
    /// present (order- and encoding-independent) in the recorded
    /// request's percent-decoded query.
    pub query: Option<BTreeMap<String, String>>,
}

/// A validation expectation. The grammar keys mirror the mock-testkit
/// matcher rules: `equals`, `regex`, `contains`, `startsWith`,
/// `endsWith`, `exists`, `jsonSubset`.
///
/// Grammar (dual, value-style): a bare value is a literal
/// `equals`; an object with exactly one recognized matcher key is that
/// matcher (this reading takes precedence over the literal one); any
/// other object — zero, multiple, or unrecognized keys — is a literal
/// `equals` compared structurally. `regex` patterns are
/// compile-verified at load time, matching the unit-tier matcher
/// rules.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Expectation {
    /// Exact equality against the value.
    Equals(serde_json::Value),
    /// Regular expression match, compile-verified at load time.
    Regex(String),
    /// Substring containment.
    Contains(String),
    /// Prefix match.
    StartsWith(String),
    /// Suffix match.
    EndsWith(String),
    /// The value under validation is present.
    Exists,
    /// Recursive-subset match against an object.
    JsonSubset(serde_json::Value),
    /// Matches any value including null; the wildcard verb (`ignore`
    /// at the grammar layer, the Citrus `@ignore@` equivalent).
    Any,
}

/// The sql-target row-shape expectation: exactly one row shape is
/// populated at parse time — concrete row patterns or a row-count
/// bound (`rows` XOR `bound`).
///
/// `columns` names the projection the assertion applies to; it is
/// applied at the call site, which projects the observed rows by
/// column name before matching (ADR-0072 §3 — the algebra is
/// parameterized, observation is per-tier).
#[derive(Debug, Clone, PartialEq)]
pub struct RowsExpectation {
    /// Optional projection: the column names the observed rows are
    /// narrowed to before matching, applied by name at the call site.
    pub columns: Option<Vec<String>>,
    /// Whether the rows may match in any order; `false` matches
    /// positionally in declaration order.
    pub unordered: bool,
    /// Concrete row patterns, one expectation per projected cell.
    /// Populated exactly when `bound` is `None`.
    pub rows: Option<Vec<Vec<Expectation>>>,
    /// Row-count bound over the projected rows. Populated exactly
    /// when `rows` is `None`.
    pub bound: Option<CountBound>,
}

/// Whether one snapshot's filtered count satisfies the bound: the
/// decision predicate of the no-deadline read and of the final expiry
/// snapshot.
pub fn bound_holds(bound: &CountBound, actual: usize) -> bool {
    let actual = actual as u64;
    match bound {
        CountBound::Exact(n) => actual == *n,
        CountBound::AtLeast(n) => actual >= *n,
        CountBound::AtMost(n) => actual <= *n,
        CountBound::Range(min, max) => actual >= *min && actual <= *max,
    }
}

/// Whether the poll may settle early on this snapshot. `Exact`
/// settles at equality and `AtLeast` at its floor — both sound
/// because arrivals only add, so a reached count stays reached.
/// `AtMost` and a `Range` never settle early: a passing snapshot
/// cannot prove the count stays within bounds while the window is
/// open.
///
/// Early-settle soundness assumes a monotone subject (arrivals only
/// add). SQL row sets are NOT monotone — a DELETE shrinks them — so
/// SQL validation must not settle early; the final snapshot at
/// deadline decides (papal e_opus, bd rc-25lup.2, 2026-09-09).
pub fn settles_early(bound: &CountBound, actual: usize) -> bool {
    match bound {
        CountBound::Exact(_) | CountBound::AtLeast(_) => bound_holds(bound, actual),
        CountBound::AtMost(_) | CountBound::Range(..) => false,
    }
}

/// Whether this snapshot has already broken an upper bound beyond
/// recovery (`AtMost` above its ceiling, a `Range` above its
/// maximum): arrivals only add, so the claim fails on the first
/// observation instead of waiting the window out.
pub fn above_ceiling(bound: &CountBound, actual: usize) -> bool {
    let actual = actual as u64;
    match bound {
        CountBound::Exact(_) | CountBound::AtLeast(_) => false,
        CountBound::AtMost(n) => actual > *n,
        CountBound::Range(_, max) => actual > *max,
    }
}

/// The percent-decoded query pairs of a path-and-query: everything
/// after the first `?`, parsed with `form_urlencoded`, which
/// percent-decodes `%XX` and `+`. A path without a query yields no
/// pairs.
pub fn query_pairs(path_and_query: &str) -> Vec<(String, String)> {
    match path_and_query.split_once('?') {
        Some((_, query)) => form_urlencoded::parse(query.as_bytes())
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect(),
        None => Vec::new(),
    }
}

/// Counts the recorded requests that pass all filters: `method`
/// compares ASCII-case-insensitively (callers may project any
/// casing), the path filter matches
/// the recorded path-and-query — `Exact` byte-for-byte, `Contains`
/// by substring, `Matches` by regex — and the `query` subset requires
/// every declared pair to appear among the request's
/// percent-decoded query pairs, in any position order. A `None`
/// filter passes everything, and all filters combine conjunctively.
///
/// Each request is projected as its `(method, path_and_query)` pair.
/// The regex of a `Matches` filter compiles once per call, not once
/// per recorded request; an invalid pattern matches nothing, failing
/// closed.
pub fn matching_count<'a>(
    requests: impl IntoIterator<Item = (&'a str, &'a str)>,
    method: Option<&str>,
    path_filter: Option<&PathFilter>,
    query: Option<&BTreeMap<String, String>>,
) -> usize {
    // The regex of a `Matches` filter compiles once per call, not once
    // per recorded request. An invalid pattern (the parser rejects it
    // first) matches nothing, failing closed.
    let matches_regex = match path_filter {
        Some(PathFilter::Matches(pattern)) => regex::Regex::new(pattern).ok(),
        _ => None,
    };
    requests
        .into_iter()
        .filter(|(request_method, path)| {
            let path_matches = match path_filter {
                None => true,
                Some(PathFilter::Exact(p)) => p.as_str() == *path,
                Some(PathFilter::Contains(s)) => path.contains(s.as_str()),
                Some(PathFilter::Matches(_)) => {
                    matches_regex.as_ref().is_some_and(|re| re.is_match(path))
                }
            };
            let query_subset = query.is_none_or(|declared| {
                let pairs = query_pairs(path);
                declared
                    .iter()
                    .all(|(key, value)| pairs.iter().any(|(k, v)| k == key && v == value))
            });
            method.is_none_or(|m| m.eq_ignore_ascii_case(request_method))
                && path_matches
                && query_subset
        })
        .count()
}

/// Renders a count bound in its own grammar for mismatch details:
/// `Exact(3)` renders `expected 3` — the historical phrasing the
/// exact-count tests pin byte-for-byte — `AtLeast(3)` renders
/// `expected at least 3`, `AtMost(2)` renders `expected at most 2`,
/// and `Range(2, 4)` renders `expected between 2 and 4`.
pub fn render_bound(bound: &CountBound) -> String {
    match bound {
        CountBound::Exact(n) => format!("expected {n}"),
        CountBound::AtLeast(n) => format!("expected at least {n}"),
        CountBound::AtMost(n) => format!("expected at most {n}"),
        CountBound::Range(min, max) => format!("expected between {min} and {max}"),
    }
}

/// The pure per-form boolean of a validation expectation against a
/// value: `Equals` compares by equality; `Regex` failing to compile
/// matches nothing (fail closed), otherwise matching the stringified
/// value; `Contains`/`StartsWith`/`EndsWith` match the stringified
/// value; `Exists` holds for any non-null value; `Any` matches every
/// value including null; `JsonSubset` recursive-subset matches via
/// `json_subset`.
pub fn expectation_matches(expectation: &Expectation, value: &serde_json::Value) -> bool {
    match expectation {
        Expectation::Equals(expected) => value == expected,
        Expectation::Regex(pattern) => {
            regex::Regex::new(pattern).is_ok_and(|regex| regex.is_match(&stringify(value)))
        }
        Expectation::Contains(needle) => stringify(value).contains(needle),
        Expectation::StartsWith(prefix) => stringify(value).starts_with(prefix),
        Expectation::EndsWith(suffix) => stringify(value).ends_with(suffix),
        Expectation::Exists => value != &serde_json::Value::Null,
        Expectation::JsonSubset(pattern) => json_subset(pattern, value),
        Expectation::Any => true,
    }
}

/// Whether a set of row patterns matches observed rows. Ordered
/// (`unordered == false`): the row counts are equal and every pattern
/// row satisfies its expectations positionally against the actual row
/// at the same index. Unordered: the row counts are equal and a
/// perfect matching exists between pattern rows and actual rows —
/// decided with Kuhn's augmenting-path bipartite matching over the
/// cell-compatibility matrix, never factorial backtracking. Expected
/// rows are iterated in declaration order, so the decision is
/// deterministic.
pub fn rows_match(
    expected: &[Vec<Expectation>],
    actual: &[Vec<serde_json::Value>],
    unordered: bool,
) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    if !unordered {
        return expected
            .iter()
            .zip(actual)
            .all(|(pattern, row)| row_pattern_matches(pattern, row));
    }
    // Compatibility matrix: `compat[p][r]` — pattern row `p` matches
    // actual row `r`.
    let compat: Vec<Vec<bool>> = expected
        .iter()
        .map(|pattern| {
            actual
                .iter()
                .map(|row| row_pattern_matches(pattern, row))
                .collect()
        })
        .collect();
    // `match_of_row[r]` is the pattern row currently assigned to
    // actual row `r`.
    let mut match_of_row: Vec<Option<usize>> = vec![None; actual.len()];
    for pattern in 0..expected.len() {
        let mut visited = vec![false; actual.len()];
        if !try_augment(pattern, &compat, &mut match_of_row, &mut visited) {
            return false;
        }
    }
    true
}

/// Whether one pattern row matches one actual row: same length and
/// every cell satisfies its expectation.
fn row_pattern_matches(pattern: &[Expectation], row: &[serde_json::Value]) -> bool {
    pattern.len() == row.len()
        && pattern
            .iter()
            .zip(row)
            .all(|(expectation, value)| expectation_matches(expectation, value))
}

/// Kuhn's augmenting path: whether pattern row `pattern` can reach an
/// unmatched actual row by reassigning the patterns currently holding
/// the rows it is compatible with. `visited` marks the actual rows
/// probed on this path.
fn try_augment(
    pattern: usize,
    compat: &[Vec<bool>],
    match_of_row: &mut [Option<usize>],
    visited: &mut [bool],
) -> bool {
    for (row, &compatible) in compat[pattern].iter().enumerate() {
        if !compatible || visited[row] {
            continue;
        }
        visited[row] = true;
        match match_of_row[row] {
            None => {
                match_of_row[row] = Some(pattern);
                return true;
            }
            Some(holder) => {
                if try_augment(holder, compat, match_of_row, visited) {
                    match_of_row[row] = Some(pattern);
                    return true;
                }
            }
        }
    }
    false
}

/// Renders a value for string matchers: strings as-is, anything else
/// as its JSON form.
pub fn stringify(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Recursive-subset match: every key in `pattern` must exist in
/// `actual` with a recursively subset-matching value; values outside
/// `pattern` are ignored. Non-object patterns compare by equality.
fn json_subset(pattern: &serde_json::Value, actual: &serde_json::Value) -> bool {
    match (pattern, actual) {
        (serde_json::Value::Object(pattern_object), serde_json::Value::Object(actual_object)) => {
            pattern_object.iter().all(|(key, pattern_value)| {
                actual_object
                    .get(key)
                    .is_some_and(|actual_value| json_subset(pattern_value, actual_value))
            })
        }
        _ => pattern == actual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_holds_covers_every_form_at_edges() {
        let cases = [
            (CountBound::Exact(2), [false, false, true, false, false]),
            (CountBound::AtLeast(2), [false, false, true, true, true]),
            (CountBound::AtMost(2), [true, true, true, false, false]),
            (CountBound::Range(1, 3), [false, true, true, true, false]),
        ];
        for (bound, holds) in cases {
            for (count, expected) in holds.into_iter().enumerate() {
                assert_eq!(
                    bound_holds(&bound, count),
                    expected,
                    "{bound:?} at count {count}"
                );
            }
        }
    }

    #[test]
    fn settles_early_absence_claims_never_settle() {
        for count in 0..=5usize {
            assert!(
                !settles_early(&CountBound::AtMost(5), count),
                "AtMost(5) at count {count}"
            );
            assert!(
                !settles_early(&CountBound::Range(0, 5), count),
                "Range(0, 5) at count {count}"
            );
            assert_eq!(
                settles_early(&CountBound::Exact(2), count),
                count == 2,
                "Exact(2) at count {count}"
            );
            assert_eq!(
                settles_early(&CountBound::AtLeast(2), count),
                count >= 2,
                "AtLeast(2) at count {count}"
            );
        }
    }

    #[test]
    fn above_ceiling_only_upper_breaches() {
        assert!(above_ceiling(&CountBound::AtMost(2), 3));
        assert!(above_ceiling(&CountBound::Range(1, 2), 3));
        assert!(!above_ceiling(&CountBound::Exact(2), 99));
        assert!(!above_ceiling(&CountBound::AtLeast(2), 99));
        assert!(!above_ceiling(&CountBound::AtMost(2), 2));
    }

    #[test]
    fn matching_count_query_subset_order_and_encoding_independent() {
        let declared = BTreeMap::from([
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
        ]);
        let reordered_and_encoded = [("GET", "/x?b=2&a=1"), ("POST", "/x?a=%31&b=%32")];
        assert_eq!(
            matching_count(reordered_and_encoded, None, None, Some(&declared)),
            2
        );
        assert_eq!(
            matching_count([("GET", "/x?a=1")], None, None, Some(&declared)),
            0
        );

        let only_a = BTreeMap::from([("a".to_string(), "1".to_string())]);
        assert_eq!(
            matching_count([("GET", "/x?a=1")], None, None, Some(&only_a)),
            1
        );
        assert_eq!(
            matching_count([("GET", "/x?b=2")], None, None, Some(&declared)),
            0
        );
    }

    #[test]
    fn matching_count_invalid_regex_fails_closed() {
        let filter = PathFilter::Matches("(".to_string());
        assert_eq!(
            matching_count([("GET", "/anything")], None, Some(&filter), None),
            0
        );
    }

    #[test]
    fn matching_count_method_case_insensitive() {
        let requests = [("POST", "/o"), ("GET", "/o")];
        assert_eq!(matching_count(requests, Some("post"), None, None), 1);
    }

    #[test]
    fn matching_count_path_forms() {
        let requests = [("GET", "/o?a=1"), ("POST", "/o?a=1&x=2"), ("GET", "/diff")];
        let exact = PathFilter::Exact("/o?a=1".to_string());
        let contains = PathFilter::Contains("/o".to_string());
        let matches = PathFilter::Matches("^/o".to_string());
        assert_eq!(matching_count(requests, None, Some(&exact), None), 1);
        assert_eq!(matching_count(requests, None, Some(&contains), None), 2);
        assert_eq!(matching_count(requests, None, Some(&matches), None), 2);
    }

    #[test]
    fn query_pairs_no_question_mark() {
        assert!(query_pairs("/noquery").is_empty());
        assert_eq!(
            query_pairs("/q?a=1"),
            vec![("a".to_string(), "1".to_string())]
        );
    }

    #[test]
    fn query_pairs_plus_decoding() {
        assert_eq!(
            query_pairs("/x?a=1+2"),
            vec![("a".to_string(), "1 2".to_string())]
        );
    }

    #[test]
    fn expectation_matches_string_forms() {
        let value = serde_json::json!("hello world");
        assert!(expectation_matches(
            &Expectation::Contains("world".to_string()),
            &value
        ));
        assert!(expectation_matches(
            &Expectation::StartsWith("hello".to_string()),
            &value
        ));
        assert!(expectation_matches(
            &Expectation::EndsWith("world".to_string()),
            &value
        ));
        assert!(expectation_matches(&Expectation::Exists, &value));
        assert!(expectation_matches(
            &Expectation::Regex("^hello".to_string()),
            &value
        ));
        assert!(expectation_matches(
            &Expectation::Equals(serde_json::json!("hello world")),
            &value
        ));
        assert!(!expectation_matches(
            &Expectation::Contains("nope".to_string()),
            &value
        ));
    }

    #[test]
    fn expectation_matches_object_forms() {
        let value = serde_json::json!({"n": "café", "s": "hello world"});
        assert!(expectation_matches(
            &Expectation::Equals(serde_json::json!({"n": "café", "s": "hello world"})),
            &value
        ));
        assert!(expectation_matches(
            &Expectation::JsonSubset(serde_json::json!({"n": "café"})),
            &value
        ));
        assert!(expectation_matches(
            &Expectation::Regex("caf".to_string()),
            &value
        ));
        assert!(expectation_matches(&Expectation::Exists, &value));
        assert!(!expectation_matches(
            &Expectation::JsonSubset(serde_json::json!({"n": "other"})),
            &value
        ));
        assert!(!expectation_matches(
            &Expectation::Exists,
            &serde_json::Value::Null
        ));
        assert!(!expectation_matches(
            &Expectation::Regex("(".to_string()),
            &value
        ));
    }

    #[test]
    fn json_subset_recursive_objects() {
        let actual = serde_json::json!({"user": {"name": "María", "role": "admin"}, "extra": 1});
        assert!(expectation_matches(
            &Expectation::JsonSubset(serde_json::json!({"user": {"name": "María"}})),
            &actual
        ));
        assert!(!expectation_matches(
            &Expectation::JsonSubset(serde_json::json!({"user": {"name": "other"}})),
            &actual
        ));
    }

    #[test]
    fn any_matches_all_values_including_null() {
        for value in [
            serde_json::json!(null),
            serde_json::json!(0),
            serde_json::json!("x"),
            serde_json::json!([1, 2]),
            serde_json::json!({"k": "v"}),
        ] {
            assert!(
                expectation_matches(&Expectation::Any, &value),
                "Any vs {value}"
            );
        }
    }

    #[test]
    fn any_distinct_from_exists() {
        assert!(!expectation_matches(
            &Expectation::Exists,
            &serde_json::Value::Null
        ));
        assert!(expectation_matches(
            &Expectation::Any,
            &serde_json::Value::Null
        ));
    }

    fn two_row_pattern() -> Vec<Vec<Expectation>> {
        vec![
            vec![
                Expectation::Equals(serde_json::json!(1)),
                Expectation::Contains("li".to_string()),
            ],
            vec![Expectation::Equals(serde_json::json!(2)), Expectation::Any],
        ]
    }

    #[test]
    fn rows_match_ordered_positional() {
        let expected = two_row_pattern();
        let actual = vec![
            vec![serde_json::json!(1), serde_json::json!("alice")],
            vec![serde_json::json!(2), serde_json::json!("bob")],
        ];
        assert!(rows_match(&expected, &actual, false));
        let swapped = vec![actual[1].clone(), actual[0].clone()];
        assert!(!rows_match(&expected, &swapped, false));
    }

    #[test]
    fn rows_match_length_mismatch_fails() {
        let expected = two_row_pattern();
        let actual = vec![vec![serde_json::json!(1), serde_json::json!("alice")]];
        assert!(!rows_match(&expected, &actual, false));
        assert!(!rows_match(&expected, &actual, true));
    }

    #[test]
    fn rows_match_unordered_reorder() {
        let expected = two_row_pattern();
        let actual = vec![
            vec![serde_json::json!(2), serde_json::json!("bob")],
            vec![serde_json::json!(1), serde_json::json!("alice")],
        ];
        assert!(rows_match(&expected, &actual, true));
    }

    #[test]
    fn rows_match_unordered_duplicates() {
        let expected = vec![
            vec![Expectation::Equals(serde_json::json!(1))],
            vec![Expectation::Equals(serde_json::json!(1))],
        ];
        let same = vec![vec![serde_json::json!(1)], vec![serde_json::json!(1)]];
        assert!(rows_match(&expected, &same, true));
        let mixed = vec![vec![serde_json::json!(1)], vec![serde_json::json!(2)]];
        assert!(!rows_match(&expected, &mixed, true));
    }

    #[test]
    fn rows_match_kuhn_needs_augmenting() {
        let expected = vec![
            vec![Expectation::Any, Expectation::Any],
            vec![Expectation::Equals(serde_json::json!(1)), Expectation::Any],
        ];
        let actual = vec![
            vec![serde_json::json!(1), serde_json::json!("x")],
            vec![serde_json::json!(2), serde_json::json!("a")],
        ];
        // Pattern 0 matches both rows, pattern 1 only the first:
        // greedy first-fit strands pattern 1, the augmenting path
        // reassigns pattern 0 to the second row.
        assert!(rows_match(&expected, &actual, true));
        assert!(!rows_match(&expected, &actual, false));
    }

    #[test]
    fn rows_match_wildcard_including_null_cells() {
        let expected = vec![vec![Expectation::Any]];
        let actual = vec![vec![serde_json::Value::Null]];
        assert!(rows_match(&expected, &actual, false));
        assert!(rows_match(&expected, &actual, true));
    }

    #[test]
    fn render_bound_forms() {
        let bounds = [
            CountBound::Exact(3),
            CountBound::AtLeast(3),
            CountBound::AtMost(2),
            CountBound::Range(2, 4),
        ];
        let rendered: Vec<String> = bounds.iter().map(render_bound).collect();
        for text in &rendered {
            assert!(!text.is_empty(), "empty render for {text:?}");
        }
        for (index, left) in rendered.iter().enumerate() {
            for right in &rendered[index + 1..] {
                assert_ne!(left, right, "duplicate render `{left}`");
            }
        }
    }
}
