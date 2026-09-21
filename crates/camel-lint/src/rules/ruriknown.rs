//! R-URI-known rule — scheme + option validation against the catalog.
//!
//! For each endpoint URI, splits the scheme (text before the first `:`) and
//! consults [`ComponentMetadataCatalog`]:
//!
//! - **Absent scheme** → one informational `UnverifiedScheme` note on the
//!   scheme token; no option diagnostics (the catalog cannot verify options
//!   for a scheme it does not know).
//! - **Cross-source duplicate key** (an option key in both the URI query
//!   string and a `parameters:` map, or in config and step `parameters:`
//!   maps) → one `DuplicateKey` error on the redundant occurrence, before
//!   any catalog lookup — the collision fails lowering regardless of scheme
//!   knowledge.
//! - **Known-but-minimal scheme** (registered, no `uri_options`) → silent.
//! - **Known scheme with `uri_options`** → each provided option is resolved
//!   against `name`/`aliases`: an unresolved key yields an `UnknownOption`
//!   error on the key; a resolved option whose value does not parse as its
//!   declared `OptionKind` yields a `KindMismatch` error on the value
//!   (Bool/Int/Float/Duration/Enum/List are validated; String accepts any
//!   text); any catalog `UriOption` declared `required` that is absent
//!   yields a `MissingRequiredOption` error on the URI.
//!
//! Kind validation mirrors runtime parse semantics per kind: Bool uses the
//! `parse_bool_param` vocabulary (`true`/`false`/`1`/`0`/`yes`/`no`, any
//! case); Int/Float use the Rust integer/float `FromStr` grammars (the kind
//! carries no signedness or width — anything `i64` or `u64` accepts is an
//! integer, anything `f64` accepts is a float); Duration uses the
//! `humantime` grammar the workspace's duration strings share (`500ms`,
//! `2h30m`, `1m 30s`, fractional `1.5s`; bare integers are Int-kind
//! values, not durations); Enum is membership in the declared allowed
//! values under the two normalizations runtime `FromStr` impls apply —
//! ASCII-case folding and underscore stripping (`if_reply_expected`
//! matches `IfReplyExpected`); List is the
//! comma-separated convention, each element validated against the element
//! kind (no production component parses a List option today — the derive's
//! codegen requires `FromStr`, which bare `Vec<T>` fields lack — so the
//! convention is best-effort). Values carrying an interpolation marker
//! (`${...}` or `{{...}}`) resolve at boot, not at lint time; their
//! resolved type is unknowable, so kind validation skips them (mirrors
//! R-SECRET's reference treatment) — except a value that is exactly one
//! whole-scalar `${env:VAR:-default}` token: the default is the concrete
//! boot-time fallback, so it is validated against the kind (no-default,
//! escaped, and mid-string tokens stay exempt). String needs no
//! validation. The
//! `#[non_exhaustive]` attribute additionally requires `matches!`-style
//! non-exhaustive matching so future kinds stay non-erroring.

use camel_api::component_metadata::{ComponentMetadataCatalog, OptionKind};

use crate::diagnostic::{Diagnostic, DiagnosticCode, Severity, Span, UriKnownSubCode};
use crate::document::Document;
use crate::env_interpolation::{WholeScalarEnvToken, whole_scalar_env_token};
use crate::route_view::{Endpoint, OptionOrigin};
use crate::rule::Rule;

/// R-URI-known: validates endpoint schemes and options against the catalog.
pub struct RUriKnownRule;

impl Rule for RUriKnownRule {
    fn analyze(&self, doc: &Document, catalog: &dyn ComponentMetadataCatalog) -> Vec<Diagnostic> {
        // R-URI-known cannot run on a document that failed to parse — R-SYN
        // owns that case.
        if doc.parse_failure.is_some() {
            return Vec::new();
        }

        let mut diagnostics = Vec::new();
        for ep in doc.route_view.endpoints() {
            analyze_endpoint(&ep, catalog, &mut diagnostics);
        }
        diagnostics
    }

    fn code(&self) -> DiagnosticCode {
        DiagnosticCode::RUriKnown(UriKnownSubCode::UnverifiedScheme)
    }
}

/// Flag option keys declared in more than one source origin for `ep`.
///
/// One error per colliding key, on the span of the FIRST non-Query
/// occurrence: options arrive ordered query-first, then step-level
/// parameters, then config parameters, so that span is the step-level key
/// when both parameter sides collide and the parameter key for a
/// query/parameters overlap — the side the lowering's duplicate-key errors
/// name. Raw keys only; alias resolution plays no part. Repeated keys
/// within the raw query alone are legal (the lowering preserves them).
fn flag_cross_source_duplicates(ep: &Endpoint, diagnostics: &mut Vec<Diagnostic>) {
    struct Seen {
        query: bool,
        step: bool,
        config: bool,
        first_non_query: Option<Span>,
    }
    let mut by_key: std::collections::BTreeMap<&str, Seen> = std::collections::BTreeMap::new();
    for opt in &ep.options {
        let seen = by_key.entry(opt.key.value.as_str()).or_insert(Seen {
            query: false,
            step: false,
            config: false,
            first_non_query: None,
        });
        match opt.origin {
            OptionOrigin::Query => seen.query = true,
            OptionOrigin::StepParameters => {
                seen.step = true;
                if seen.first_non_query.is_none() {
                    seen.first_non_query = Some(opt.key.span.clone());
                }
            }
            OptionOrigin::ConfigParameters => {
                seen.config = true;
                if seen.first_non_query.is_none() {
                    seen.first_non_query = Some(opt.key.span.clone());
                }
            }
        }
    }
    for (key, seen) in by_key {
        // Fixed source vocabulary for the message, in lowering-check order.
        let mut sources: Vec<&str> = Vec::new();
        if seen.query {
            sources.push("the URI query string");
        }
        if seen.step {
            sources.push("step parameters");
        }
        if seen.config {
            sources.push("config parameters");
        }
        if sources.len() < 2 {
            continue;
        }
        // ≥2 sources always include a parameters-side occurrence, which
        // recorded its key span above; kept total rather than `expect` to
        // honor lint-unwrap on non-test code.
        let Some(span) = seen.first_non_query else {
            continue;
        };
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::RUriKnown(UriKnownSubCode::DuplicateKey),
            severity: Severity::Error,
            span,
            message: format!(
                "duplicate option key `{key}`: declared in {}",
                sources.join(" and ")
            ),
            fix: None,
        });
    }
}

/// Validate `raw` against the declared `OptionKind`, runtime-parse parity.
///
/// Returns `None` when the value parses as the kind (or the kind is not
/// validated: String, future `#[non_exhaustive]` kinds). Returns
/// `Some(expected)` — the human-readable expectation for the mismatch
/// message — when the value cannot parse.
fn validate_kind(kind: &OptionKind, raw: &str) -> Option<String> {
    match kind {
        OptionKind::String => None,
        OptionKind::Bool => {
            // Mirrors camel_endpoint::uri::parse_bool_param: the runtime
            // accepts 1/0/yes/no in any case, so the lint must too.
            let v = raw.to_ascii_lowercase();
            if matches!(v.as_str(), "true" | "false" | "1" | "0" | "yes" | "no") {
                None
            } else {
                Some("a boolean value (true/false)".to_string())
            }
        }
        OptionKind::Int => {
            // OptionKind carries no signedness or width; the runtime parses
            // with the field's integer FromStr, so accept the union of the
            // i64 and u64 grammars.
            if raw.parse::<i64>().is_ok() || raw.parse::<u64>().is_ok() {
                None
            } else {
                Some("an integer value".to_string())
            }
        }
        OptionKind::Float => {
            // Same grammar the runtime's f32/f64 FromStr uses.
            if raw.parse::<f64>().is_ok() {
                None
            } else {
                Some("a floating-point value".to_string())
            }
        }
        OptionKind::Duration => {
            // humantime grammar (the workspace's duration-string parser):
            // unit-suffixed values and concatenated groups. A bare integer
            // is an Int-kind value (the UriConfig `_ms` companion
            // convention), not a duration.
            if humantime::parse_duration(raw).is_ok() {
                None
            } else {
                Some("a duration value (e.g. 500ms, 2s, 1h30m)".to_string())
            }
        }
        OptionKind::Enum(variants) => {
            // Membership in the declared allowed values under the two
            // normalizations runtime FromStr impls apply: ASCII-case
            // folding (camel-log's level uppercases; camel-stream's frame
            // is exact) and underscore stripping (seda's
            // waitForTaskToComplete/exchangePattern lowercase + remove
            // `_`, accepting `if_reply_expected` / `in_only`). Comparing
            // with BOTH normalizations applied can only widen acceptance
            // — strict impls lose nothing (false negatives, never false
            // positives). A runtime parser accepting a token its
            // metadata does not list (an alias such as log's `WARNING`
            // or http's `none`) is a metadata gap the component must fix
            // by listing it. An empty variant list declares no
            // constraint — nothing to validate against.
            if variants.is_empty()
                || variants.iter().any(|v| {
                    // Case-fold + underscore-strip both sides (the plain
                    // case-compare is subsumed: stripping preserves it).
                    v.to_ascii_lowercase().replace('_', "")
                        == raw.to_ascii_lowercase().replace('_', "")
                })
            {
                None
            } else {
                Some(format!("one of: {}", variants.join(", ")))
            }
        }
        OptionKind::List(inner) => {
            // No production component parses a List option today (the
            // UriConfig codegen requires FromStr, which bare Vec<T> fields
            // lack), so the comma-separated convention is best-effort: an
            // empty value is an empty list; each element is validated
            // against the element kind (elements trimmed — a list author's
            // `1, 2` spacing is not a kind error).
            if raw.is_empty() {
                return None;
            }
            let mismatch = raw
                .split(',')
                .map(str::trim)
                .any(|el| validate_kind(inner, el).is_some());
            if mismatch {
                Some(format!(
                    "a comma-separated list of {} values",
                    kind_noun(inner)
                ))
            } else {
                None
            }
        }
        // #[non_exhaustive]: unknown future kinds stay non-erroring.
        _ => None,
    }
}

/// Human-readable noun for an element kind (used by List messages).
///
/// KEEP IN SYNC with [`validate_kind`]: both match every `OptionKind`
/// variant independently (the scalar branches there carry richer hints —
/// `(true/false)`, duration examples, allowed-value lists — so they are
/// not derived from the nouns). A future kind added to camel-api falls
/// through both `_` arms: non-erroring, noun `valid`.
fn kind_noun(kind: &OptionKind) -> String {
    match kind {
        OptionKind::String => "string".to_string(),
        OptionKind::Int => "integer".to_string(),
        OptionKind::Bool => "boolean".to_string(),
        OptionKind::Float => "floating-point".to_string(),
        OptionKind::Duration => "duration".to_string(),
        OptionKind::Enum(variants) => format!("enum ({})", variants.join("/")),
        OptionKind::List(inner) => format!("{} list", kind_noun(inner)),
        _ => "valid".to_string(),
    }
}

/// Analyze a single endpoint against the catalog, appending diagnostics.
fn analyze_endpoint(
    ep: &Endpoint,
    catalog: &dyn ComponentMetadataCatalog,
    diagnostics: &mut Vec<Diagnostic>,
) {
    // Cross-source duplicate keys FIRST: the collision fails lowering
    // regardless of catalog knowledge or a parseable scheme, so this pass
    // runs before the scheme split and the catalog early-returns. It mirrors
    // the lowering's two fail-closed paths (`EndpointUriError::DuplicateKey`
    // for query/parameters overlap; `combine_params` for config/step
    // parameters overlap). Raw keys only — alias resolution plays no part,
    // and repeated keys within the raw query alone stay legal (the lowering
    // preserves them in order).
    flag_cross_source_duplicates(ep, diagnostics);

    let uri = &ep.uri.value;
    // Split the scheme: text before the first `:`. A URI without a colon has
    // no parseable scheme; skip it (structural issues are R-SCHEMA's domain).
    let Some(colon) = uri.find(':') else {
        return;
    };
    let scheme = &uri[..colon];
    let scheme_span = Span::new(ep.uri.span.start, ep.uri.span.start + scheme.len());

    let Some(meta) = catalog.get_metadata(scheme) else {
        // Absent scheme: one informational note on the scheme token, and NO
        // option diagnostics for this endpoint.
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::RUriKnown(UriKnownSubCode::UnverifiedScheme),
            severity: Severity::Info,
            span: scheme_span,
            message: "scheme not registered in catalog; cannot verify options".to_string(),
            fix: None,
        });
        return;
    };

    // Known-but-minimal scheme: nothing to validate.
    if meta.uri_options.is_empty() {
        return;
    }

    // Validate each provided option against the catalog's uri_options.
    for opt in &ep.options {
        let Some(canon) = crate::route_view::resolve_option(opt, &meta.uri_options) else {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::RUriKnown(UriKnownSubCode::UnknownOption),
                severity: Severity::Error,
                span: opt.key.span.clone(),
                message: format!("unknown option `{}` for scheme `{}`", opt.key.value, scheme),
                fix: None,
            });
            continue;
        };
        // Kind validation for every validated kind. Values carrying an
        // interpolation marker (`${...}` / `{{...}}`) resolve at boot, not
        // at lint time; their resolved type is unknowable, so they are
        // exempt (mirrors R-SECRET's reference treatment) — with one
        // carve-out (rc-w4otz): a value that is EXACTLY one whole-scalar
        // `${env:VAR:-default}` token carries its concrete boot-time
        // fallback in the default (what the runtime parses when the
        // variable is unset), so the default is validated against the
        // kind. No-default tokens, `$${...}` escapes, and tokens nested
        // in a larger string stay exempt.
        if let Some(val) = &opt.value {
            let mismatch = if !val.value.contains("${") && !val.value.contains("{{") {
                validate_kind(&canon.kind, &val.value)
            } else if let Some(WholeScalarEnvToken::WithDefault { default }) =
                whole_scalar_env_token(&val.value)
            {
                validate_kind(&canon.kind, &default)
            } else {
                // Interpolation-bearing but not a whole-scalar defaulted
                // env token: exempt.
                None
            };
            if let Some(expected) = mismatch {
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::RUriKnown(UriKnownSubCode::KindMismatch),
                    severity: Severity::Error,
                    span: val.span.clone(),
                    message: format!("option `{}` expects {}", canon.name, expected),
                    fix: None,
                });
            }
        }
    }

    // Report required catalog options that are absent from the endpoint.
    for canon in &meta.uri_options {
        if canon.required
            && !crate::route_view::option_present(&canon.name, &canon.aliases, &ep.options)
        {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::RUriKnown(UriKnownSubCode::MissingRequiredOption),
                severity: Severity::Error,
                span: ep.uri.span.clone(),
                message: format!("missing required option `{}`", canon.name),
                fix: None,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "ruriknown_tests.rs"]
mod tests;
