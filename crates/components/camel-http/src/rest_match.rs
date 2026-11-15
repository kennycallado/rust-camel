//! Path template matcher for method-aware REST dispatch.
//!
//! This module lets the HTTP consumer register endpoints with a
//! `(method, path template)` triple instead of a single path string. The
//! `match_endpoint` function picks the most specific match for an incoming
//! request; an exact specificity tie is reported as `Ambiguous` instead of
//! being resolved silently.
//!
//! Specificity rule: templates are ranked by `(L, C)`, where `L` is the
//! number of segments carrying at least one literal part and `C` is the
//! total number of literal characters. The pair is compared
//! lexicographically, higher wins. So `GET /users/me` is preferred over
//! `GET /users/{id}`, and `GET /{q}xy` over `GET /{p}y`, when both are
//! registered.
//!
//! Matching a segment is a bounded dynamic program over part index and
//! `char`-boundary offset (never exponential backtracking), with captures
//! recovered leftmost-shortest.
//!
//! Grammar: a template is `/`-separated segments (empty segments and a
//! leading `/` are dropped). A segment is a sequence of literal text and
//! `{name}` placeholders; a placeholder may appear anywhere in a segment
//! (`/files/{id}.xml`), and a malformed brace fails the whole parse.
//!
//! Note: `camel-http` does NOT depend on `camel-dsl` (verified at plan
//! review), so the segment types are defined locally rather than
//! re-exported. The DSL keeps a byte-for-byte duplicate of this grammar,
//! pinned together by the cross-crate corpus test.

use std::collections::HashMap;
use std::fmt;

/// One piece of a path-template segment: literal text or a `{name}`
/// placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentPart {
    /// Literal text that must match exactly, e.g. `users`.
    Lit(String),
    /// A `{name}` placeholder; the matching request text is captured under
    /// `name`.
    Param(String),
}

/// One parsed segment of a path template, made of ordered parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentPattern {
    /// Ordered literal and placeholder parts of the segment.
    pub parts: Vec<SegmentPart>,
}

/// Why a path template could not be parsed.
///
/// Every variant carries the offending `segment` text so the caller can
/// name the malformation in its startup or route-load diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum TemplateParseError {
    /// A `{` placeholder was not terminated by `}`.
    UnclosedPlaceholder {
        /// The segment containing the unclosed placeholder.
        segment: String,
    },
    /// A `}` appeared outside a placeholder.
    StrayCloseBrace {
        /// The segment containing the stray `}`.
        segment: String,
    },
    /// A placeholder had an empty name (`{}`).
    EmptyPlaceholder {
        /// The segment containing the empty placeholder.
        segment: String,
    },
}

impl fmt::Display for TemplateParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnclosedPlaceholder { segment } => write!(
                f,
                "unclosed placeholder in segment '{segment}': a '{{' must be closed by '}}'"
            ),
            Self::StrayCloseBrace { segment } => write!(
                f,
                "stray '}}' in segment '{segment}': '}}' outside a placeholder is not allowed"
            ),
            Self::EmptyPlaceholder { segment } => {
                write!(f, "empty placeholder name in segment '{segment}'")
            }
        }
    }
}

impl std::error::Error for TemplateParseError {}

/// A registered REST endpoint with method, parsed path template, and the
/// caller-supplied payload (typically the mpsc sender for the consumer's
/// receive loop — see `HttpRouteRegistryInner::rest_endpoints`).
#[derive(Debug, Clone)]
pub struct RestEndpoint<T> {
    pub method: String,
    pub segments: Vec<SegmentPattern>,
    pub payload: T,
}

/// Result of matching a request path against registered endpoints.
#[derive(Debug, Clone)]
pub struct MatchResult<T> {
    pub payload: T,
    pub path_params: HashMap<String, String>,
}

/// Outcome of matching an incoming `(method, path)` against the registered
/// REST endpoints (review C3).
#[derive(Debug, Clone)]
pub enum MatchOutcome<T> {
    /// A unique best match was found.
    Found(MatchResult<T>),
    /// No endpoint matched the (method, path).
    NotFound,
    /// Two or more equal-specificity templates matched the same request.
    /// This is an ambiguous registration that should have been rejected at
    /// lowering time; the caller surfaces a loud error rather than silently
    /// falling through to a 404.
    Ambiguous,
}

/// Rank a parsed template's specificity as `(L, C)`.
///
/// `L` is the number of segments carrying at least one literal part and `C`
/// is the total number of literal characters across the whole template.
/// `match_endpoint` compares these pairs lexicographically, so a template
/// with more literal-bearing segments wins first, then one with more literal
/// text. `/users/me` is `(2, 7)` and beats `/users/{id}` `(1, 5)`; `/{q}xy`
/// `(1, 2)` beats `/{p}y` `(1, 1)`.
#[must_use]
pub fn template_specificity(segments: &[SegmentPattern]) -> (usize, usize) {
    let mut literal_segments = 0;
    let mut literal_chars = 0;
    for pattern in segments {
        let mut has_literal = false;
        for part in &pattern.parts {
            if let SegmentPart::Lit(text) = part {
                has_literal = true;
                literal_chars += text.chars().count();
            }
        }
        if has_literal {
            literal_segments += 1;
        }
    }
    (literal_segments, literal_chars)
}

/// Try to match `path` against one template's segments.
///
/// Returns the captured params on a hit, `None` on any mismatch
/// (segment count, literal mismatch, or empty parameter name).
pub fn try_match(request: &[&str], template: &[SegmentPattern]) -> Option<HashMap<String, String>> {
    if request.len() != template.len() {
        return None;
    }
    let mut params = HashMap::new();
    for (req, pattern) in request.iter().zip(template.iter()) {
        if !match_segment(req, pattern, &mut params) {
            return None;
        }
    }
    Some(params)
}

/// Match one request segment against one `SegmentPattern`, recording the
/// placeholder captures in `params`.
///
/// The match is a bounded dynamic program, never exponential backtracking:
/// `reachable[i][j]` means "the first `i` parts match the request prefix up
/// to `char`-boundary offset `j`". A `Lit` part advances deterministically
/// (and only when the next bytes equal the literal); a `Param` part reaches
/// every `char`-boundary offset at or after `j`, the empty capture included.
/// Worst case is `O(parts × len²)`, so the cost is polynomial in the request
/// segment length.
///
/// Captures are recovered leftmost-shortest using a backward
/// `can_finish[i][j]` table ("parts `i..` can match the request suffix from
/// `j`"): walking the parts forward, each `Param` takes the smallest offset
/// that both keeps the prefix reachable and lets the remaining parts finish.
fn match_segment(
    request: &str,
    pattern: &SegmentPattern,
    params: &mut HashMap<String, String>,
) -> bool {
    let parts = pattern.parts.as_slice();
    // Empty names are illegal — reject so an empty `{}` segment does not
    // silently match anything.
    for part in parts {
        if matches!(part, SegmentPart::Param(name) if name.is_empty()) {
            return false;
        }
    }

    let offsets = char_boundaries(request);
    let width = offsets.len();
    let last = width - 1;

    // reachable[i][j]: first `i` parts match the request prefix `..offsets[j]`.
    let mut reachable = vec![vec![false; width]; parts.len() + 1];
    reachable[0][0] = true;
    for (i, part) in parts.iter().enumerate() {
        for j in 0..width {
            if !reachable[i][j] {
                continue;
            }
            match part {
                SegmentPart::Lit(text) => {
                    if let Some(next) = advance_literal(request, &offsets, j, text) {
                        reachable[i + 1][next] = true;
                    }
                }
                SegmentPart::Param(_) => {
                    for slot in reachable[i + 1].iter_mut().skip(j) {
                        *slot = true;
                    }
                }
            }
        }
    }
    if !reachable[parts.len()][last] {
        return false;
    }

    // can_finish[i][j]: parts `i..` can match the request suffix from `j`.
    let mut can_finish = vec![vec![false; width]; parts.len() + 1];
    can_finish[parts.len()][last] = true;
    for i in (0..parts.len()).rev() {
        // suffix_any[j] = any(can_finish[i + 1][j..]); a Param may consume
        // any run, including none, before the rest of the parts finish.
        let mut suffix_any = vec![false; width];
        let mut any = false;
        for j in (0..width).rev() {
            any |= can_finish[i + 1][j];
            suffix_any[j] = any;
        }
        for j in 0..width {
            can_finish[i][j] = match &parts[i] {
                SegmentPart::Lit(text) => advance_literal(request, &offsets, j, text)
                    .is_some_and(|next| can_finish[i + 1][next]),
                SegmentPart::Param(_) => suffix_any[j],
            };
        }
    }

    // Leftmost-shortest recovery: start at offset 0 and take the smallest
    // feasible offset for every placeholder.
    let mut j = 0;
    for (i, part) in parts.iter().enumerate() {
        match part {
            SegmentPart::Lit(text) => {
                let Some(next) = advance_literal(request, &offsets, j, text) else {
                    return false; // unreachable: reachable[i][j] held
                };
                j = next;
            }
            SegmentPart::Param(name) => {
                let Some(next) = (j..width)
                    .find(|&candidate| reachable[i + 1][candidate] && can_finish[i + 1][candidate])
                else {
                    return false; // unreachable: reachable[parts.len()][last] held
                };
                params.insert(name.clone(), request[offsets[j]..offsets[next]].to_string());
                j = next;
            }
        }
    }
    j == last
}

/// Byte offsets of every `char` boundary in `segment`, plus `segment.len()`.
fn char_boundaries(segment: &str) -> Vec<usize> {
    let mut offsets: Vec<usize> = segment.char_indices().map(|(idx, _)| idx).collect();
    offsets.push(segment.len());
    offsets
}

/// Advance from `char`-boundary index `from` by `text`, returning the index
/// of the resulting boundary, or `None` when the literal does not match.
fn advance_literal(request: &str, offsets: &[usize], from: usize, text: &str) -> Option<usize> {
    let start = *offsets.get(from)?;
    if !request.get(start..)?.starts_with(text) {
        return None;
    }
    offsets.binary_search(&(start + text.len())).ok()
}

/// Match an incoming `(method, path)` against the registered endpoints.
///
/// Precedence:
/// 1. Method match: only endpoints registered for the request method are
///    considered at all.
/// 2. Specificity: templates are ranked by `(L, C)` — literal-bearing
///    segment count, then total literal characters — and the higher pair
///    wins. So `GET /users/me` beats `GET /users/{id}` for `/users/me`, and
///    `GET /{q}xy` beats `GET /{p}y` for `/1xy`.
/// 3. Ties: two or more matching templates with the exact same `(L, C)`
///    pair are NOT resolved by registration order — the result is
///    `MatchOutcome::Ambiguous`. Registration order never breaks a tie.
///
/// Returns `NotFound` if no endpoint matches the method, or `Ambiguous` if
/// two or more equal-specificity templates match the same request (the
/// caller should reject ambiguous registrations at lowering time — review C3).
pub fn match_endpoint<T: Clone>(
    method: &str,
    path: &str,
    endpoints: &[RestEndpoint<T>],
) -> MatchOutcome<T> {
    let request_segments: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    let mut best_specificity: Option<(usize, usize)> = None;
    let mut best_index: Option<usize> = None;
    let mut best_params: Option<HashMap<String, String>> = None;
    let mut ambiguous = false;

    for (idx, ep) in endpoints.iter().enumerate() {
        if ep.method != method {
            continue;
        }
        let Some(params) = try_match(&request_segments, &ep.segments) else {
            continue;
        };
        let specificity = template_specificity(&ep.segments);

        match best_specificity {
            None => {
                best_specificity = Some(specificity);
                best_index = Some(idx);
                best_params = Some(params);
                ambiguous = false;
            }
            Some(prev) if specificity > prev => {
                best_specificity = Some(specificity);
                best_index = Some(idx);
                best_params = Some(params);
                ambiguous = false;
            }
            Some(prev) if specificity == prev => {
                // Tie: if the index is the same as best, it's a re-match
                // (shouldn't happen, but safe). Otherwise ambiguous.
                if best_index == Some(idx) {
                    continue;
                }
                ambiguous = true;
            }
            _ => {}
        }
    }

    if ambiguous {
        return MatchOutcome::Ambiguous;
    }
    let (idx, params) = match (best_index, best_params) {
        (Some(i), Some(p)) => (i, p),
        _ => return MatchOutcome::NotFound,
    };
    MatchOutcome::Found(MatchResult {
        payload: endpoints[idx].payload.clone(),
        path_params: params,
    })
}

/// Parse a path template string like `/users/{id}/orders` into a
/// `Vec<SegmentPattern>`. Used at registration time so the per-request
/// match can avoid parsing on the hot path.
///
/// The template is split on `/`, dropping empty segments and the leading
/// `/`. Each segment is scanned left to right: `{` starts a placeholder
/// whose name is the maximal run of characters excluding `{` and `}` and
/// which must be terminated by `}`; a `}` outside a placeholder is an
/// error, as is an empty name or an unclosed placeholder. Literal text
/// between placeholders becomes `Lit` parts, and a segment without
/// placeholders is one `Lit` part.
#[must_use = "a malformed template must be surfaced, not ignored"]
pub fn parse_path_template(template: &str) -> Result<Vec<SegmentPattern>, TemplateParseError> {
    template
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(parse_segment)
        .collect()
}

/// Parse one `/`-delimited segment into its ordered parts.
///
/// The scan is byte-wise, but only the ASCII braces `{` / `}` are
/// significant, so every slice boundary is a `char` boundary.
fn parse_segment(segment: &str) -> Result<SegmentPattern, TemplateParseError> {
    let bytes = segment.as_bytes();
    let mut parts = Vec::new();
    let mut cursor = 0;
    let mut literal_start = 0;

    while cursor < bytes.len() {
        match bytes[cursor] {
            b'{' => {
                let name_start = cursor + 1;
                let mut name_end = name_start;
                while name_end < bytes.len() && bytes[name_end] != b'{' && bytes[name_end] != b'}' {
                    name_end += 1;
                }
                if name_end >= bytes.len() || bytes[name_end] != b'}' {
                    return Err(TemplateParseError::UnclosedPlaceholder {
                        segment: segment.to_string(),
                    });
                }
                if name_end == name_start {
                    return Err(TemplateParseError::EmptyPlaceholder {
                        segment: segment.to_string(),
                    });
                }
                if literal_start < cursor {
                    parts.push(SegmentPart::Lit(segment[literal_start..cursor].to_string()));
                }
                parts.push(SegmentPart::Param(
                    segment[name_start..name_end].to_string(),
                ));
                cursor = name_end + 1;
                literal_start = cursor;
            }
            b'}' => {
                return Err(TemplateParseError::StrayCloseBrace {
                    segment: segment.to_string(),
                });
            }
            _ => cursor += 1,
        }
    }

    if literal_start < bytes.len() {
        parts.push(SegmentPart::Lit(segment[literal_start..].to_string()));
    }
    Ok(SegmentPattern { parts })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(method: &str, template: &str) -> RestEndpoint<String> {
        RestEndpoint {
            method: method.to_string(),
            segments: parse_path_template(template).expect("test template is well formed"),
            payload: template.to_string(), // route_id for tests
        }
    }

    #[test]
    fn exact_match_wins_over_template() {
        let endpoints = vec![ep("GET", "/users/{id}"), ep("GET", "/users/me")];
        let result = match match_endpoint("GET", "/users/me", &endpoints) {
            MatchOutcome::Found(m) => m,
            other => panic!("expected Found, got {:?}", other),
        };
        assert_eq!(result.payload, "/users/me".to_string());
        // No params extracted from a literal-only template.
        assert!(result.path_params.is_empty());
    }

    #[test]
    fn template_match_extracts_params() {
        let endpoints = vec![ep("GET", "/users/{id}")];
        let result = match match_endpoint("GET", "/users/42", &endpoints) {
            MatchOutcome::Found(m) => m,
            other => panic!("expected Found, got {:?}", other),
        };
        assert_eq!(result.payload, "/users/{id}".to_string());
        assert_eq!(result.path_params.get("id"), Some(&"42".to_string()));
    }

    #[test]
    fn different_methods_same_path() {
        let endpoints = vec![ep("GET", "/users"), ep("POST", "/users")];
        let get = match match_endpoint("GET", "/users", &endpoints) {
            MatchOutcome::Found(m) => m,
            other => panic!("expected Found, got {:?}", other),
        };
        let post = match match_endpoint("POST", "/users", &endpoints) {
            MatchOutcome::Found(m) => m,
            other => panic!("expected Found, got {:?}", other),
        };
        assert_eq!(get.payload, "/users".to_string());
        assert_eq!(post.payload, "/users".to_string());
    }

    #[test]
    fn no_match_returns_notfound() {
        let endpoints = vec![ep("GET", "/users")];
        assert!(matches!(
            match_endpoint("DELETE", "/users", &endpoints),
            MatchOutcome::NotFound
        ));
        assert!(matches!(
            match_endpoint("GET", "/orders", &endpoints),
            MatchOutcome::NotFound
        ));
        assert!(matches!(
            match_endpoint("GET", "/users/42", &endpoints),
            MatchOutcome::NotFound
        ));
    }

    #[test]
    fn trailing_slash_is_normalized() {
        let endpoints = vec![ep("GET", "/users/{id}")];
        let result = match match_endpoint("GET", "/users/42/", &endpoints) {
            MatchOutcome::Found(m) => m,
            other => panic!("expected Found, got {:?}", other),
        };
        assert_eq!(result.path_params.get("id"), Some(&"42".to_string()));
    }

    #[test]
    fn ambiguous_two_templates_same_specificity_is_reported() {
        // Both templates have one literal + one param (specificity 1 == 1)
        // AND both can match the SAME request `/users/42` (the literal
        // position `users` agrees; the param position accepts anything).
        // This is the genuine ambiguity branch — review C3.
        let endpoints = vec![ep("GET", "/users/{id}"), ep("GET", "/users/{name}")];
        assert!(matches!(
            match_endpoint("GET", "/users/42", &endpoints),
            MatchOutcome::Ambiguous
        ));
    }

    #[test]
    fn non_overlapping_literals_are_not_ambiguous() {
        // Same specificity, but the literal positions disagree (`users` vs
        // `admins`), so no single request matches both — NOT ambiguous.
        let endpoints = vec![ep("GET", "/users/{id}"), ep("GET", "/admins/{id}")];
        let result = match match_endpoint("GET", "/users/42", &endpoints) {
            MatchOutcome::Found(m) => m,
            other => panic!("expected Found, got {:?}", other),
        };
        assert_eq!(result.payload, "/users/{id}".to_string());
    }

    #[test]
    fn try_match_length_mismatch() {
        let template = vec![pattern(vec![lit("users")])];
        assert!(try_match(&["users", "42"], &template).is_none());
        assert!(try_match(&[], &template).is_none());
    }

    #[test]
    fn try_match_literal_mismatch() {
        let template = vec![pattern(vec![lit("users")]), pattern(vec![param("id")])];
        assert!(try_match(&["orders", "42"], &template).is_none());
    }

    #[test]
    fn try_match_param_captures_value() {
        let template = vec![pattern(vec![lit("users")]), pattern(vec![param("id")])];
        let params =
            try_match(&["users", "abc"], &template).expect("literal+param template matches");
        assert_eq!(params.get("id"), Some(&"abc".to_string()));
    }

    // -------------------------------------------------------------------
    // Task 1.1: segment-pattern grammar
    // -------------------------------------------------------------------

    fn lit(text: &str) -> SegmentPart {
        SegmentPart::Lit(text.to_string())
    }

    fn param(name: &str) -> SegmentPart {
        SegmentPart::Param(name.to_string())
    }

    fn pattern(parts: Vec<SegmentPart>) -> SegmentPattern {
        SegmentPattern { parts }
    }

    #[test]
    fn parse_bare_placeholder_segment() {
        // Arrange
        let template = "/users/{id}";

        // Act
        let parsed = parse_path_template(template).expect("well-formed template parses");

        // Assert
        assert_eq!(
            parsed,
            vec![pattern(vec![lit("users")]), pattern(vec![param("id")])]
        );
    }

    #[test]
    fn parse_mixed_segment_parts() {
        assert_eq!(
            parse_path_template("/files/{id}.xml").expect("mixed template parses"),
            vec![
                pattern(vec![lit("files")]),
                pattern(vec![param("id"), lit(".xml")]),
            ]
        );
        assert_eq!(
            parse_path_template("/report-{date}.json").expect("mixed template parses"),
            vec![pattern(vec![lit("report-"), param("date"), lit(".json")])]
        );
        assert_eq!(
            parse_path_template("/a{y}bc").expect("mixed template parses"),
            vec![pattern(vec![lit("a"), param("y"), lit("bc")])]
        );
    }

    #[test]
    fn parse_multi_placeholder_segment() {
        assert_eq!(
            parse_path_template("/{a}-{b}").expect("multi-placeholder template parses"),
            vec![pattern(vec![param("a"), lit("-"), param("b")])]
        );
        assert_eq!(
            parse_path_template("/{a}{b}").expect("adjacent-placeholder template parses"),
            vec![pattern(vec![param("a"), param("b")])]
        );
    }

    #[test]
    fn parse_legacy_names_stay_valid() {
        assert_eq!(
            parse_path_template("/users/{user-id}").expect("legacy name parses"),
            vec![pattern(vec![lit("users")]), pattern(vec![param("user-id")]),]
        );
        assert_eq!(
            parse_path_template("/f/{x.y}.dat").expect("dotted name parses"),
            vec![
                pattern(vec![lit("f")]),
                pattern(vec![param("x.y"), lit(".dat")]),
            ]
        );
    }

    #[test]
    fn parse_placeholder_like_literal_text_ok() {
        let parsed = parse_path_template("/files/%7Bid%7D").expect("escaped braces stay literal");
        assert_eq!(
            parsed,
            vec![pattern(vec![lit("files")]), pattern(vec![lit("%7Bid%7D")])]
        );
        assert!(
            parsed.iter().all(|segment| segment
                .parts
                .iter()
                .all(|part| matches!(part, SegmentPart::Lit(_)))),
            "escaped-brace template must not produce a Param part"
        );
    }

    #[test]
    fn parse_rejects_unclosed_placeholder() {
        assert_eq!(
            parse_path_template("/files/{id.xml"),
            Err(TemplateParseError::UnclosedPlaceholder {
                segment: "{id.xml".to_string(),
            })
        );
    }

    #[test]
    fn parse_rejects_stray_close_brace() {
        assert_eq!(
            parse_path_template("/files/id}.xml"),
            Err(TemplateParseError::StrayCloseBrace {
                segment: "id}.xml".to_string(),
            })
        );
    }

    #[test]
    fn parse_rejects_empty_placeholder_name() {
        assert_eq!(
            parse_path_template("/files/{}.xml"),
            Err(TemplateParseError::EmptyPlaceholder {
                segment: "{}.xml".to_string(),
            })
        );
    }

    #[test]
    fn parse_rejects_nested_braces() {
        let err = parse_path_template("/{{id}}").expect_err("nested braces are malformed");
        assert!(
            matches!(
                err,
                TemplateParseError::UnclosedPlaceholder { .. }
                    | TemplateParseError::StrayCloseBrace { .. }
            ),
            "expected unclosed or stray-close error, got {err:?}"
        );
    }

    #[test]
    fn parse_path_template_segments() {
        let segs =
            parse_path_template("/users/{id}/orders/{orderId}").expect("template is well formed");
        assert_eq!(
            segs,
            vec![
                pattern(vec![lit("users")]),
                pattern(vec![param("id")]),
                pattern(vec![lit("orders")]),
                pattern(vec![param("orderId")]),
            ]
        );
    }

    #[test]
    fn parse_path_template_literal_only() {
        let segs = parse_path_template("/users/me").expect("template is well formed");
        assert_eq!(
            segs,
            vec![pattern(vec![lit("users")]), pattern(vec![lit("me")])]
        );
    }

    #[test]
    fn specificity_picks_literals_over_params() {
        // Specificity of "/users/me" = 2, "/users/{id}" = 1.
        let endpoints = vec![ep("GET", "/users/{id}"), ep("GET", "/users/me")];
        // /users/me → specificity 2 (literal template) wins.
        let r = match match_endpoint("GET", "/users/me", &endpoints) {
            MatchOutcome::Found(m) => m,
            other => panic!("expected Found, got {:?}", other),
        };
        assert_eq!(r.payload, "/users/me");
        // /users/42 → only the template matches.
        let r = match match_endpoint("GET", "/users/42", &endpoints) {
            MatchOutcome::Found(m) => m,
            other => panic!("expected Found, got {:?}", other),
        };
        assert_eq!(r.payload, "/users/{id}".to_string());
        assert_eq!(r.path_params.get("id"), Some(&"42".to_string()));
    }

    // -------------------------------------------------------------------
    // Task 1.2: part-wise matching, leftmost-shortest capture, (L, C)
    // -------------------------------------------------------------------

    fn found<T: std::fmt::Debug>(outcome: MatchOutcome<T>) -> MatchResult<T> {
        match outcome {
            MatchOutcome::Found(result) => result,
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[test]
    fn mixed_segment_matches_extension_request() {
        // Arrange
        let endpoints = vec![ep("GET", "/files/{id}.xml")];

        // Act
        let result = found(match_endpoint("GET", "/files/42.xml", &endpoints));

        // Assert
        assert_eq!(result.path_params.get("id"), Some(&"42".to_string()));
    }

    #[test]
    fn trailing_literal_not_captured() {
        // Arrange
        let endpoints = vec![ep("GET", "/report-{date}.json")];

        // Act
        let result = found(match_endpoint("GET", "/report-2026-11-15.json", &endpoints));

        // Assert
        assert_eq!(
            result.path_params.get("date"),
            Some(&"2026-11-15".to_string()),
            "the trailing '.json' literal must not be captured by the placeholder"
        );
    }

    #[test]
    fn two_placeholders_leftmost_shortest() {
        // Arrange
        let endpoints = vec![ep("GET", "/{a}-{b}")];

        // Act
        let result = found(match_endpoint("GET", "/1-2-3", &endpoints));

        // Assert
        assert_eq!(result.path_params.get("a"), Some(&"1".to_string()));
        assert_eq!(result.path_params.get("b"), Some(&"2-3".to_string()));
    }

    #[test]
    fn three_placeholders_split() {
        // Arrange
        let endpoints = vec![ep("GET", "/{a}-{b}-{c}")];

        // Act
        let result = found(match_endpoint("GET", "/1-2-3-4", &endpoints));

        // Assert
        assert_eq!(result.path_params.get("a"), Some(&"1".to_string()));
        assert_eq!(result.path_params.get("b"), Some(&"2".to_string()));
        assert_eq!(result.path_params.get("c"), Some(&"3-4".to_string()));
    }

    #[test]
    fn adjacent_placeholders_first_takes_empty() {
        // Arrange
        let endpoints = vec![ep("GET", "/{a}{b}")];

        // Act
        let result = found(match_endpoint("GET", "/123", &endpoints));

        // Assert
        assert_eq!(result.path_params.get("a"), Some(&String::new()));
        assert_eq!(result.path_params.get("b"), Some(&"123".to_string()));
    }

    #[test]
    fn empty_mixed_capture_permitted() {
        // Arrange
        let endpoints = vec![ep("GET", "/a{y}bc")];

        // Act
        let result = found(match_endpoint("GET", "/abc", &endpoints));

        // Assert
        assert_eq!(result.path_params.get("y"), Some(&String::new()));
    }

    #[test]
    fn braces_in_request_value_are_data() {
        // Arrange
        let endpoints = vec![ep("GET", "/files/{id}.xml")];

        // Act
        let result = found(match_endpoint("GET", "/files/{oops}.xml", &endpoints));

        // Assert
        assert_eq!(result.path_params.get("id"), Some(&"{oops}".to_string()));
    }

    #[test]
    fn literal_placeholder_like_text_matches_literally() {
        // Arrange
        let endpoints = vec![ep("GET", "/files/%7Bid%7D")];

        // Act
        let result = found(match_endpoint("GET", "/files/%7Bid%7D", &endpoints));

        // Assert
        assert!(result.path_params.is_empty());
    }

    #[test]
    fn literal_beats_mixed_specificity() {
        // Arrange
        let endpoints = vec![ep("GET", "/fixed.xml"), ep("GET", "/{id}.xml")];

        // Act
        let result = found(match_endpoint("GET", "/fixed.xml", &endpoints));

        // Assert
        assert_eq!(result.payload, "/fixed.xml");
        assert!(result.path_params.is_empty());
    }

    #[test]
    fn longer_mixed_literal_wins() {
        // Arrange
        let endpoints = vec![ep("GET", "/{p}y"), ep("GET", "/{q}xy")];

        // Act
        let result = found(match_endpoint("GET", "/1xy", &endpoints));

        // Assert
        assert_eq!(result.payload, "/{q}xy");
        assert_eq!(result.path_params.get("q"), Some(&"1".to_string()));
    }

    #[test]
    fn legacy_multi_segment_precedence_preserved() {
        // Arrange
        let endpoints = vec![ep("GET", "/a/{x}/{y}"), ep("GET", "/{u}/b/c")];

        // Act
        let result = found(match_endpoint("GET", "/a/b/c", &endpoints));

        // Assert
        assert_eq!(result.payload, "/{u}/b/c");
        assert_eq!(result.path_params.get("u"), Some(&"a".to_string()));
    }

    #[test]
    fn specificity_pair_shape() {
        // Arrange
        let mixed = parse_path_template("/files/{id}.xml").expect("template parses");
        let literal = parse_path_template("/fixed.xml").expect("template parses");
        let bare_param = parse_path_template("/{a}").expect("template parses");
        let literal_plus_param = parse_path_template("/x/{id}").expect("template parses");

        // Act / Assert
        assert_eq!(template_specificity(&mixed), (2, 9));
        assert_eq!(template_specificity(&literal), (1, 9));
        assert_eq!(template_specificity(&bare_param), (0, 0));
        assert_eq!(template_specificity(&literal_plus_param), (1, 1));
    }

    #[test]
    fn adversarial_many_placeholders_no_match_bounded() {
        // Arrange
        let endpoints = vec![ep("GET", "/{a}{b}{c}{d}{e}{f}{g}{h}z")];
        let request = format!("/{}", "y".repeat(64));

        // Act
        let started = std::time::Instant::now();
        let outcome = match_endpoint("GET", &request, &endpoints);
        let elapsed = started.elapsed();

        // Assert
        assert!(matches!(outcome, MatchOutcome::NotFound));
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "matching must stay bounded, took {elapsed:?}"
        );
    }

    #[test]
    fn adversarial_many_placeholders_match_leftmost_shortest() {
        // Arrange
        let endpoints = vec![ep("GET", "/{a}{b}{c}{d}{e}{f}{g}{h}z")];
        let request = format!("/{}z", "y".repeat(64));

        // Act
        let result = found(match_endpoint("GET", &request, &endpoints));

        // Assert
        for name in ["a", "b", "c", "d", "e", "f", "g"] {
            assert_eq!(
                result.path_params.get(name),
                Some(&String::new()),
                "param {name} must capture the empty string"
            );
        }
        assert_eq!(
            result.path_params.get("h"),
            Some(&"y".repeat(64)),
            "the last placeholder must absorb the 64-char run"
        );
    }

    #[test]
    fn utf8_capture_boundaries() {
        // Arrange
        let endpoints = vec![ep("GET", "/{a}.txt")];

        // Act
        let result = found(match_endpoint("GET", "/café.txt", &endpoints));

        // Assert
        assert_eq!(result.path_params.get("a"), Some(&"café".to_string()));
    }
}
