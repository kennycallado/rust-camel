//! Path template matcher for method-aware REST dispatch.
//!
//! This module lets the HTTP consumer register endpoints with a
//! `(method, path template)` triple instead of a single path string. The
//! `match_endpoint` function picks the most specific match for an incoming
//! request, breaking ambiguity ties by registration order.
//!
//! Specificity rule: a path with more literal segments wins over one with
//! more `{param}` segments. So `GET /users/me` is preferred over
//! `GET /users/{id}` when both templates are registered.
//!
//! Note: `camel-http` does NOT depend on `camel-dsl` (verified at plan
//! review), so `PathSegment` is defined locally rather than re-exported.

use std::collections::HashMap;

/// One segment of a REST path template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    /// A literal segment that must match exactly, e.g. `users`.
    Literal(String),
    /// A `{name}` parameter segment; the request segment is captured under `name`.
    Param(String),
}

/// A registered REST endpoint with method, parsed path template, and the
/// caller-supplied payload (typically the mpsc sender for the consumer's
/// receive loop — see `HttpRouteRegistryInner::rest_endpoints`).
#[derive(Debug, Clone)]
pub struct RestEndpoint<T> {
    pub method: String,
    pub segments: Vec<PathSegment>,
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

/// Try to match `path` against one template's segments.
///
/// Returns the captured params on a hit, `None` on any mismatch
/// (length, literal mismatch, or empty parameter name).
pub fn try_match(request: &[&str], template: &[PathSegment]) -> Option<HashMap<String, String>> {
    if request.len() != template.len() {
        return None;
    }
    let mut params = HashMap::new();
    for (req, tmpl) in request.iter().zip(template.iter()) {
        match tmpl {
            PathSegment::Literal(lit) => {
                if req != &lit.as_str() {
                    return None;
                }
            }
            PathSegment::Param(name) => {
                // Empty names are illegal — reject so an empty `{}` template
                // doesn't silently match anything.
                if name.is_empty() {
                    return None;
                }
                params.insert(name.clone(), (*req).to_string());
            }
        }
    }
    Some(params)
}

/// Match an incoming `(method, path)` against the registered endpoints.
///
/// Precedence (highest to lowest):
/// 1. Method match: endpoints with the same method are considered first.
/// 2. Specificity: an endpoint with more literal segments wins. This means
///    `GET /users/me` beats `GET /users/{id}` for the path `/users/me`.
/// 3. Registration order: ties on specificity resolve to the
///    first-registered endpoint (stable for the lifetime of the registry).
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

    let mut best_specificity: Option<usize> = None;
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
        let specificity = ep
            .segments
            .iter()
            .filter(|s| matches!(s, PathSegment::Literal(_)))
            .count();

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

/// Parse a path template string like `/users/{id}/orders` into
/// a `Vec<PathSegment>`. Used at registration time so the per-request
/// match can avoid parsing on the hot path.
pub fn parse_path_template(template: &str) -> Vec<PathSegment> {
    template
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|seg| {
            if let Some(inner) = seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                PathSegment::Param(inner.to_string())
            } else {
                PathSegment::Literal(seg.to_string())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(method: &str, template: &str) -> RestEndpoint<String> {
        RestEndpoint {
            method: method.to_string(),
            segments: parse_path_template(template),
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
        let template = vec![PathSegment::Literal("users".into())];
        assert!(try_match(&["users", "42"], &template).is_none());
        assert!(try_match(&[], &template).is_none());
    }

    #[test]
    fn try_match_literal_mismatch() {
        let template = vec![
            PathSegment::Literal("users".into()),
            PathSegment::Param("id".into()),
        ];
        assert!(try_match(&["orders", "42"], &template).is_none());
    }

    #[test]
    fn try_match_param_captures_value() {
        let template = vec![
            PathSegment::Literal("users".into()),
            PathSegment::Param("id".into()),
        ];
        let params = try_match(&["users", "abc"], &template).unwrap();
        assert_eq!(params.get("id"), Some(&"abc".to_string()));
    }

    #[test]
    fn parse_path_template_segments() {
        let segs = parse_path_template("/users/{id}/orders/{orderId}");
        assert_eq!(
            segs,
            vec![
                PathSegment::Literal("users".into()),
                PathSegment::Param("id".into()),
                PathSegment::Literal("orders".into()),
                PathSegment::Param("orderId".into()),
            ]
        );
    }

    #[test]
    fn parse_path_template_literal_only() {
        let segs = parse_path_template("/users/me");
        assert_eq!(
            segs,
            vec![
                PathSegment::Literal("users".into()),
                PathSegment::Literal("me".into())
            ]
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
}
