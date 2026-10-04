//! Read-only enforcement for `js:` expressions and predicates (change
//! `language-value-boundary`, B4).
//!
//! The compile-time AST walk in `expression.rs` rejects the statically
//! decidable mutation surface. Everything it cannot decide — dynamic dispatch
//! (`camel[expr]`), alias capture (`const m = camel.headers.set`), method
//! destructuring (`const { set } = camel.headers`), `Object.defineProperty`,
//! `Reflect.set`, and nested object writes — must still fail loudly at runtime
//! instead of being silently discarded with the snapshot.
//!
//! Mechanism, in two parts:
//!
//! - Every snapshot object exposed to the script (`camel`, the header and
//!   property maps, their `__data` backing objects, the body, and every nested
//!   object/array) is wrapped in a `Proxy` whose `set`, `deleteProperty`,
//!   `defineProperty`, `setPrototypeOf`, and `preventExtensions` traps throw a
//!   private sentinel. The `preventExtensions` trap makes
//!   `Object.preventExtensions`/`seal`/`freeze` on a snapshot fail loudly
//!   instead of silently locking it.
//! - The `camel` mutator functions (`set_property`, `headers.set`,
//!   `headers.remove`, `properties.set`, `properties.remove`) are installed as
//!   throwing host functions, so an alias captured before the call still
//!   refuses.
//!
//! The sentinel is a per-evaluation host-created null-prototype object that
//! script code cannot forge (it can only rethrow a caught instance). The
//! worker classifies an uncaught error by sentinel identity with
//! [`JsValue::strict_equals`], never by message text, so no exchange-derived
//! string can be mistaken for a refusal. `Object.freeze`, strict-mode writes,
//! and final-snapshot comparison are all insufficient here: they either
//! silently accept same-value writes or restore the previous value.

use boa_engine::{
    Context, JsError, JsObject, JsResult, JsValue, js_string, native_function::NativeFunction,
    object::builtins::JsArray,
};

/// A per-evaluation private marker thrown by every read-only trap.
///
/// The wrapped `JsObject` is what native closures capture (it is already
/// `Trace`); this handle keeps the identity anchor on the worker stack.
pub(crate) struct ReadOnlySentinel(JsObject);

impl ReadOnlySentinel {
    /// Create a fresh sentinel object on the active realm.
    pub(crate) fn new() -> Self {
        Self(JsObject::with_null_proto())
    }

    /// True when `err` is exactly this sentinel (identity, not message).
    pub(crate) fn matches(&self, err: &JsError) -> bool {
        err.as_opaque()
            .is_some_and(|value| value.strict_equals(&JsValue::from(self.0.clone())))
    }
}

/// Build a native function that always throws `sentinel`.
///
/// The closure captures the sentinel `JsObject` (already `Trace`), so no
/// crate-local GC wrapper is needed.
fn sentinel_function(sentinel: &ReadOnlySentinel, ctx: &mut Context) -> JsValue {
    let object = sentinel.0.clone();
    let function = NativeFunction::from_copy_closure_with_captures(
        |_this, _args, object: &JsObject, _ctx| {
            Err(JsError::from_opaque(JsValue::from(object.clone())))
        },
        object,
    );
    JsValue::from(function.to_js_function(ctx.realm()))
}

/// A native function that always throws the sentinel.
///
/// Used for every mutator installed in read-only mode, so alias capture
/// (`const m = camel.set_property; m(...)`) and destructuring
/// (`const { set } = camel.headers`) cannot bypass the proxy.
pub(crate) fn throwing_mutator(sentinel: &ReadOnlySentinel, ctx: &mut Context) -> JsValue {
    sentinel_function(sentinel, ctx)
}

/// Wrap `target` in a `Proxy` whose mutating traps throw the sentinel.
///
/// `get`, `has`, `ownKeys`, and the other read traps are left at their
/// default forwarding behavior: ordinary reads stay transparent.
///
/// `preventExtensions` is trapped too: `Object.preventExtensions`,
/// `Object.seal`, and `Object.freeze` on a snapshot object must fail loudly
/// rather than silently lock the snapshot (which would also make later
/// same-value writes throw a native `TypeError` instead of the private
/// sentinel, and would let a script permanently alter the snapshot's
/// extensibility).
pub(crate) fn readonly_proxy(
    target: JsObject,
    sentinel: &ReadOnlySentinel,
    ctx: &mut Context,
) -> JsResult<JsObject> {
    let handler = JsObject::with_null_proto();
    for trap in [
        "set",
        "deleteProperty",
        "defineProperty",
        "setPrototypeOf",
        "preventExtensions",
    ] {
        let function = sentinel_function(sentinel, ctx);
        handler.set(js_string!(trap), function, false, ctx)?;
    }
    let constructor = ctx.intrinsics().constructors().proxy().constructor();
    constructor.construct(&[JsValue::from(target), JsValue::from(handler)], None, ctx)
}

/// Recursively wrap `value` (and every object reachable from it) in read-only
/// proxies, returning the wrapped value.
///
/// Functions are returned unchanged: the snapshot exposes host functions
/// (`get`, `has`, `keys`, `property`) that are read-only by construction, and
/// wrapping a function would change its callability. JSON snapshot values are
/// acyclic, so the recursion terminates.
pub(crate) fn deep_readonly_wrap(
    value: JsValue,
    sentinel: &ReadOnlySentinel,
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let Some(object) = value.as_object() else {
        return Ok(value);
    };
    if object.is_callable() {
        return Ok(value);
    }
    if object.is_array() {
        let array = JsArray::from_object(object.clone())?;
        let length = array.length(ctx)?;
        for index in 0..length {
            let item = array.get(index, ctx)?;
            let wrapped = deep_readonly_wrap(item, sentinel, ctx)?;
            array.set(index, wrapped, false, ctx)?;
        }
    } else {
        for key in object.own_property_keys(ctx)? {
            let item = object.get(key.clone(), ctx)?;
            let wrapped = deep_readonly_wrap(item, sentinel, ctx)?;
            object.set(key, wrapped, false, ctx)?;
        }
    }
    Ok(JsValue::from(readonly_proxy(object, sentinel, ctx)?))
}
