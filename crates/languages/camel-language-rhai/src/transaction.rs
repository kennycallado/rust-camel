//! Mutating-script transaction: change detection, validated conversion and
//! commit (change `language-value-boundary`, task 2.3, boundary B2 + B3).
//!
//! A mutating script is a transaction. The evaluator snapshots the native
//! `body`/`headers`/`properties` values before evaluation and compares them
//! with the post-eval scope values using a RECURSIVE, TYPE-SENSITIVE native
//! comparison ([`rhai_values_differ`]). An int-to-float change is a change
//! even though rhai's `==` treats `1 == 1.0` as true. Only added, changed and
//! removed entries are converted — with GENERIC entry targets, never runtime
//! key names — so untouched entries keep their original value and variant and
//! a conversion failure commits nothing.

use camel_language_api::{Body, LanguageError, Value};
use rhai::Dynamic;

use crate::converter::rhai_map_to_value_map;
use crate::json::JsonNumber;

/// A single header/property entry change.
pub(crate) enum EntryDelta {
    /// Added or changed entry, already converted to a JSON value.
    Set(Value),
    /// Entry removed by the script.
    Remove,
}

/// Recursive, type-sensitive, native value comparison.
///
/// Returns `true` when the two values differ. Different scalar types always
/// differ (this is what catches an int-to-float change); same-type scalars are
/// compared natively per [`Kind`] — bool, int, float, char and string — with
/// no script evaluation. Float comparison uses `PartialEq`, so NaN differs
/// from itself (the same result the engine's `==` produced). A failed
/// downcast is conservatively reported as a change. Maps recurse over the
/// union of keys and arrays recurse pairwise by position. An exotic type is
/// conservatively reported as a change.
pub(crate) fn rhai_values_differ(pre: &Dynamic, post: &Dynamic) -> bool {
    let (pre_kind, post_kind) = (kind(pre), kind(post));
    if pre_kind != post_kind {
        return true;
    }
    match pre_kind {
        Kind::Unit => false,
        Kind::Bool | Kind::Int | Kind::Float | Kind::Char | Kind::String => {
            !scalars_equal(pre, post)
        }
        Kind::Map => maps_differ(pre, post),
        Kind::Array => arrays_differ(pre, post),
        Kind::Blob => !blobs_equal(pre, post),
        Kind::JsonNumber => !json_numbers_equal(pre, post),
        // Exotic / remaining custom types have no defined comparison —
        // conservative.
        Kind::Other => true,
    }
}

/// Compute the per-key deltas for one rhai map against its pre snapshot.
///
/// Only added/changed entries are collected into a temporary map and run
/// through the single boundary converter; removed keys become `Remove` deltas.
/// An error propagates with no delta applied. Runtime keys never enter
/// `target`.
pub(crate) fn map_deltas(
    pre: &rhai::Map,
    post: &rhai::Map,
    target: &str,
) -> Result<Vec<(String, EntryDelta)>, LanguageError> {
    let mut changed = rhai::Map::new();
    let mut removed = Vec::new();
    for (k, pre_v) in pre {
        match post.get(k) {
            None => removed.push(k.to_string()),
            Some(post_v) if rhai_values_differ(pre_v, post_v) => {
                changed.insert(k.clone(), post_v.clone());
            }
            Some(_) => {}
        }
    }
    for (k, post_v) in post {
        if !pre.contains_key(k) {
            changed.insert(k.clone(), post_v.clone());
        }
    }

    // Conversion happens for every pending change before any of them is
    // committed; a failure returns `Err` with nothing applied.
    let converted = rhai_map_to_value_map(&changed, target)?;
    let mut deltas: Vec<(String, EntryDelta)> = removed
        .into_iter()
        .map(|k| (k, EntryDelta::Remove))
        .collect();
    deltas.extend(converted.into_iter().map(|(k, v)| (k, EntryDelta::Set(v))));
    Ok(deltas)
}

/// Map a converted body value back to the native [`Body`] variant.
///
/// Only the assigned body is mapped: `Null` becomes `Empty`, a string becomes
/// `Text`, and any structured value becomes `Json`.
pub(crate) fn value_to_body(v: Value) -> Body {
    match v {
        Value::Null => Body::Empty,
        Value::String(s) => Body::Text(s),
        other => Body::Json(other),
    }
}

/// Scalar type discriminant. Containers have their own variants so callers can
/// recurse; anything unrecognized is conservative (`Other`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Unit,
    Bool,
    Int,
    Float,
    Char,
    String,
    Map,
    Array,
    Blob,
    JsonNumber,
    Other,
}

fn kind(d: &Dynamic) -> Kind {
    if d.is_unit() {
        Kind::Unit
    } else if d.is_bool() {
        Kind::Bool
    } else if d.is_int() {
        Kind::Int
    } else if d.is_float() {
        Kind::Float
    } else if d.is_char() {
        Kind::Char
    } else if d.is_string() {
        Kind::String
    } else if d.is::<rhai::Map>() {
        Kind::Map
    } else if d.is::<rhai::Array>() {
        Kind::Array
    } else if d.is::<rhai::Blob>() {
        Kind::Blob
    } else if d.is::<JsonNumber>() {
        Kind::JsonNumber
    } else {
        Kind::Other
    }
}

/// Compare two same-kind scalars natively. A failed downcast (for example an
/// exotic scalar) is conservatively "not equal".
fn scalars_equal(a: &Dynamic, b: &Dynamic) -> bool {
    match kind(a) {
        Kind::Bool => match (a.as_bool(), b.as_bool()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        },
        Kind::Int => match (a.as_int(), b.as_int()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        },
        Kind::Float => match (a.as_float(), b.as_float()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        },
        Kind::Char => match (a.as_char(), b.as_char()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        },
        Kind::String => match (a.as_immutable_string_ref(), b.as_immutable_string_ref()) {
            (Ok(a), Ok(b)) => a.as_str() == b.as_str(),
            _ => false,
        },
        _ => false,
    }
}

fn maps_differ(pre: &Dynamic, post: &Dynamic) -> bool {
    let pre = pre.read_lock::<rhai::Map>();
    let post = post.read_lock::<rhai::Map>();
    let (Some(pre), Some(post)) = (pre, post) else {
        return true;
    };
    if pre.len() != post.len() {
        return true;
    }
    for (k, pre_v) in pre.iter() {
        match post.get(k) {
            Some(post_v) if !rhai_values_differ(pre_v, post_v) => {}
            _ => return true,
        }
    }
    false
}

fn arrays_differ(pre: &Dynamic, post: &Dynamic) -> bool {
    let pre = pre.read_lock::<rhai::Array>();
    let post = post.read_lock::<rhai::Array>();
    let (Some(pre), Some(post)) = (pre, post) else {
        return true;
    };
    if pre.len() != post.len() {
        return true;
    }
    for (a, b) in pre.iter().zip(post.iter()) {
        if rhai_values_differ(a, b) {
            return true;
        }
    }
    false
}

fn blobs_equal(pre: &Dynamic, post: &Dynamic) -> bool {
    let pre = pre.read_lock::<rhai::Blob>();
    let post = post.read_lock::<rhai::Blob>();
    match (pre, post) {
        (Some(a), Some(b)) => *a == *b,
        _ => false,
    }
}

/// Compare two JSON numbers by their exact source token.
///
/// A `JsonNumber` is an opaque exact decimal image of a JSON integer that does
/// not fit `i64`. Two images are equal iff their tokens are identical; any
/// other shape (a failed downcast) is conservatively "not equal".
fn json_numbers_equal(pre: &Dynamic, post: &Dynamic) -> bool {
    let pre = pre.read_lock::<JsonNumber>();
    let post = post.read_lock::<JsonNumber>();
    match (pre, post) {
        (Some(a), Some(b)) => a.as_str() == b.as_str(),
        _ => false,
    }
}

#[cfg(test)]
thread_local! {
    /// Number of script-level evaluations performed by the write-back diff.
    ///
    /// Spec-mandated instrumentation for the native-comparison design: the
    /// diff compares leaves natively, so nothing in this module can increment
    /// the counter. Its `0` assertion pins that design, while the compile-time
    /// absence of an `Engine` parameter on [`rhai_values_differ`] is the
    /// primary structural guard.
    static DIFF_ENGINE_EVALS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Read the current thread's write-back-diff script-evaluation count.
#[cfg(test)]
pub(crate) fn diff_engine_evals() -> usize {
    DIFF_ENGINE_EVALS.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests {
    use super::rhai_values_differ;
    use crate::json::JsonNumber;
    use rhai::Dynamic;

    #[test]
    fn json_number_token_equality_diffs_by_token() {
        let a = Dynamic::from(JsonNumber::new("18446744073709551615"));
        let b = Dynamic::from(JsonNumber::new("18446744073709551615"));
        let c = Dynamic::from(JsonNumber::new("5"));
        let i = Dynamic::from(5_i64);

        assert!(
            !rhai_values_differ(&a, &b),
            "same token must not report a delta"
        );
        assert!(
            rhai_values_differ(&a, &c),
            "different tokens must report a delta"
        );
        assert!(
            rhai_values_differ(&a, &i),
            "json number vs int must report a delta"
        );
    }
}
