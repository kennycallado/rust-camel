//! Mutating-script transaction: change detection, validated conversion and
//! commit (change `language-value-boundary`, task 2.3, boundary B2 + B3).
//!
//! A mutating script is a transaction. The evaluator snapshots the native
//! `body`/`headers`/`properties` values before evaluation and compares them
//! with the post-eval scope values using a RECURSIVE, TYPE-SENSITIVE
//! comparison ([`rhai_values_differ`]). An int-to-float change is a change
//! even though rhai's `==` treats `1 == 1.0` as true. Only added, changed and
//! removed entries are converted — with GENERIC entry targets, never runtime
//! key names — so untouched entries keep their original value and variant and
//! a conversion failure commits nothing.

use camel_language_api::{Body, LanguageError, Value};
use rhai::{Dynamic, Engine, Scope};

use crate::converter::rhai_map_to_value_map;

/// A single header/property entry change.
pub(crate) enum EntryDelta {
    /// Added or changed entry, already converted to a JSON value.
    Set(Value),
    /// Entry removed by the script.
    Remove,
}

/// Recursive, type-sensitive value comparison.
///
/// Returns `true` when the two values differ. Different scalar types always
/// differ (this is what catches an int-to-float change); same-type scalars are
/// compared through the frozen comparison [`Engine`] evaluating `a == b`
/// (`Dynamic` has no `PartialEq`). Maps recurse over the union of keys and
/// arrays recurse pairwise by position. A comparison failure or an exotic type
/// is conservatively reported as a change.
pub(crate) fn rhai_values_differ(engine: &Engine, pre: &Dynamic, post: &Dynamic) -> bool {
    let (pre_kind, post_kind) = (kind(pre), kind(post));
    if pre_kind != post_kind {
        return true;
    }
    match pre_kind {
        Kind::Unit => false,
        Kind::Bool | Kind::Int | Kind::Float | Kind::Char | Kind::String => {
            !scalars_equal(engine, pre, post)
        }
        Kind::Map => maps_differ(engine, pre, post),
        Kind::Array => arrays_differ(engine, pre, post),
        Kind::Blob => !blobs_equal(pre, post),
        // Exotic / custom types have no defined comparison — conservative.
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
    engine: &Engine,
    pre: &rhai::Map,
    post: &rhai::Map,
    target: &str,
) -> Result<Vec<(String, EntryDelta)>, LanguageError> {
    let mut changed = rhai::Map::new();
    let mut removed = Vec::new();
    for (k, pre_v) in pre {
        match post.get(k) {
            None => removed.push(k.to_string()),
            Some(post_v) if rhai_values_differ(engine, pre_v, post_v) => {
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
    } else {
        Kind::Other
    }
}

/// Compare two same-type scalars through the frozen engine. A comparison
/// failure (for example an exotic scalar) is conservatively "not equal".
fn scalars_equal(engine: &Engine, a: &Dynamic, b: &Dynamic) -> bool {
    let mut scope = Scope::new();
    scope.push_dynamic("a", a.clone());
    scope.push_dynamic("b", b.clone());
    engine
        .eval_expression_with_scope::<bool>(&mut scope, "a == b")
        .unwrap_or(false)
}

fn maps_differ(engine: &Engine, pre: &Dynamic, post: &Dynamic) -> bool {
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
            Some(post_v) if !rhai_values_differ(engine, pre_v, post_v) => {}
            _ => return true,
        }
    }
    false
}

fn arrays_differ(engine: &Engine, pre: &Dynamic, post: &Dynamic) -> bool {
    let pre = pre.read_lock::<rhai::Array>();
    let post = post.read_lock::<rhai::Array>();
    let (Some(pre), Some(post)) = (pre, post) else {
        return true;
    };
    if pre.len() != post.len() {
        return true;
    }
    for (a, b) in pre.iter().zip(post.iter()) {
        if rhai_values_differ(engine, a, b) {
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
