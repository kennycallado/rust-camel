//! Permission value-source walker — the targeted exactly-one
//! diagnostics for `security_policy.permission` `resource`/`action`
//! value specs (rc-gddb2, rc-lkbqi).
//!
//! Extracted verbatim from `rschema.rs` (rc-2ov6p): the recursive walk
//! of collapsed Option-wrapper `anyOf` error contexts that recovers the
//! instance paths of exactly-one oneOf failures of
//! `RouteDslPermissionValueSource`, plus the field-context and
//! found-set renderers those diagnostics use. Non-permission oneOf
//! failures keep their generic collapsed diagnostics.

use jsonschema::error::{ValidationError, ValidationErrorKind};

/// Schema-path substring identifying the exactly-one oneOf failure of
/// `RouteDslPermissionValueSource` (probe-confirmed against jsonschema
/// 0.52.1 with the injected oneOf: the nested `OneOfNotValid` error's
/// schema path is `/$defs/RouteDslPermissionValueSource/oneOf`). The
/// targeted tests fail loudly if a future version changes the form.
pub(super) const PERMISSION_VALUE_SOURCE_ONEOF_MARKER: &str = "RouteDslPermissionValueSource/oneOf";

/// The `RouteDslPermissionPolicy` child key a validator instance path
/// ends with (`resource` or `action`), if any — the field context for a
/// targeted exactly-one permission value-source diagnostic.
///
/// Splits the raw validator instance path on `/` (dropping the empty
/// first segment) and matches the LAST segment. Only
/// `RouteDslPermissionPolicy` carries these child keys, so the tail IS
/// the field context regardless of the prefix (envelope depth, route
/// index, rest/mcp nesting).
pub(super) fn permission_value_source_field(instance_path: &str) -> Option<&'static str> {
    let mut last = "";
    for segment in instance_path.split('/').filter(|s| !s.is_empty()) {
        last = segment;
    }
    match last {
        "resource" => Some("resource"),
        "action" => Some("action"),
        _ => None,
    }
}

/// Render the found-set of a permission value-source instance: the keys
/// among `literal`, `header`, `property` whose value is non-null, joined
/// in that canonical order (regardless of authored order); `none set`
/// when the collection is empty (a non-object or missing instance
/// reports `none set` too — `Value::get` yields `None` there).
pub(super) fn found_sources(value: &serde_json::Value) -> String {
    let found = non_null_source_keys(value);
    if found.is_empty() {
        "none set".to_string()
    } else {
        found.join(", ")
    }
}

/// The keys among `literal`, `header`, `property` whose value is
/// non-null in a permission value-source instance, in that canonical
/// order (regardless of authored order). The count drives the
/// targeted-diagnostic gate: exactly one means the oneOf failed on the
/// value's TYPE, zero or several on the cardinality.
pub(super) fn non_null_source_keys(value: &serde_json::Value) -> Vec<&'static str> {
    let mut found: Vec<&'static str> = Vec::new();
    for key in ["literal", "header", "property"] {
        if value.get(key).is_some_and(|v| !v.is_null()) {
            found.push(key);
        }
    }
    found
}

/// Recursively collect the instance paths of every exactly-one oneOf
/// failure of `RouteDslPermissionValueSource` buried inside a collapsed
/// error tree (jsonschema 0.52 nests branch errors as owned
/// `ValidationError<'static>` inside the `AnyOf`/`OneOfNotValid`
/// contexts, so owned paths avoid borrowed-error gymnastics).
///
/// - a `OneOfNotValid` whose schema path contains
///   [`PERMISSION_VALUE_SOURCE_ONEOF_MARKER`] is a match: push its
///   instance path and stop descending (the branch errors below it are
///   per-branch `required` noise);
/// - any other `AnyOf`/`OneOfNotValid` may still bury a match deeper:
///   recurse into every nested error of every branch;
/// - every other kind cannot bury a permission oneOf failure: skip.
pub(super) fn collect_permission_oneof_paths(err: &ValidationError<'_>, out: &mut Vec<String>) {
    match err.kind() {
        ValidationErrorKind::OneOfNotValid { .. }
            if err
                .schema_path()
                .as_str()
                .contains(PERMISSION_VALUE_SOURCE_ONEOF_MARKER) =>
        {
            out.push(err.instance_path().as_str().to_owned());
        }
        ValidationErrorKind::AnyOf { context } | ValidationErrorKind::OneOfNotValid { context } => {
            for branch in context {
                for nested in branch {
                    collect_permission_oneof_paths(nested, out);
                }
            }
        }
        _ => {}
    }
}
