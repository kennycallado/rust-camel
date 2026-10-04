//! Registers the `camel` global object and `console` into a Boa `Context`.

use std::collections::HashMap;

use boa_engine::{
    Context, JsValue, js_string,
    native_function::NativeFunction,
    object::{JsObject, builtins::JsArray},
    property::PropertyKey,
};
use serde_json::Value;

use crate::{
    engine::JsExchange,
    error::JsLanguageError,
    readonly::{ReadOnlySentinel, deep_readonly_wrap, readonly_proxy, throwing_mutator},
    value::{ArrayDetector, js_to_value_with_detector, value_to_js},
};

/// Map a Boa error from host-object construction into the typed language error.
fn host_error(e: boa_engine::JsError) -> JsLanguageError {
    JsLanguageError::Execution {
        message: e.to_string(),
    }
}

macro_rules! make_console_fn {
    ($level:ident, $ctx:expr) => {{
        let f = NativeFunction::from_copy_closure(move |_this, args, ctx| {
            let msg = args
                .iter()
                .map(|a| {
                    a.to_string(ctx)
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" ");
            tracing::$level!(target: "camel_language_js::console", "{}", msg);
            Ok(JsValue::undefined())
        });
        f.to_js_function($ctx.realm())
    }};
}

/// Replaces the default `console` with a tracing-backed version and
/// returns the installed console object.
///
/// NOTE: `Context::default()` already registers a built-in `console`,
/// so we overwrite via `global_object().set()` instead of `register_global_property`.
///
/// The global set uses `throw = true`: a rejected set (for example a
/// spoofed non-writable `console` binding) surfaces as an error instead of
/// a silently ignored `Ok(false)` — callers treat it as realm poison and
/// recycle.
pub fn register_console(ctx: &mut Context) -> boa_engine::JsResult<JsObject> {
    let console = JsObject::with_null_proto();

    let log_js = make_console_fn!(info, ctx);
    let _ = console.set(js_string!("log"), JsValue::from(log_js), false, ctx);

    // Also wire warn/error/debug to tracing levels.
    let warn_js = make_console_fn!(warn, ctx);
    let _ = console.set(js_string!("warn"), JsValue::from(warn_js), false, ctx);

    let error_js = make_console_fn!(error, ctx);
    let _ = console.set(js_string!("error"), JsValue::from(error_js), false, ctx);

    let debug_js = make_console_fn!(debug, ctx);
    let _ = console.set(js_string!("debug"), JsValue::from(debug_js), false, ctx);

    ctx.global_object().set(
        js_string!("console"),
        JsValue::from(console.clone()),
        true,
        ctx,
    )?;
    Ok(console)
}

/// Build the `camel` global object with `headers`, `properties`, and `body`.
///
/// When `read_only` is `Some`, every snapshot object is wrapped in a throwing
/// `Proxy`, the mutator functions throw the sentinel, and the returned object
/// is itself wrapped — see [`crate::readonly`]. The writable path
/// (`read_only == None`) is unchanged.
pub fn build_camel_global(
    exchange: &JsExchange,
    read_only: Option<&ReadOnlySentinel>,
    ctx: &mut Context,
) -> Result<JsObject, JsLanguageError> {
    let camel = JsObject::with_null_proto();

    let headers = build_map_object(&exchange.headers, read_only, ctx)?;
    camel
        .set(js_string!("headers"), JsValue::from(headers), false, ctx)
        .map_err(host_error)?;

    let properties = build_map_object(&exchange.properties, read_only, ctx)?;
    camel
        .set(
            js_string!("properties"),
            JsValue::from(properties.clone()),
            false,
            ctx,
        )
        .map_err(host_error)?;

    // `property(name)` is a read-only accessor: it reads the (already
    // read-only-wrapped) backing data and returns the wrapped value. In
    // read-only mode the map it captures is a proxy, so `__data` resolves to
    // the proxied backing object.
    let properties_get = properties.clone();
    let property_fn = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, props, ctx| {
            let key = args
                .first()
                .unwrap_or(&JsValue::undefined())
                .to_string(ctx)?
                .to_std_string_escaped();
            let data = props.get(js_string!("__data"), ctx)?;
            let data_obj = data.as_object().ok_or_else(|| {
                boa_engine::JsNativeError::typ().with_message("properties.__data missing")
            })?;
            data_obj.get(js_string!(key.as_str()), ctx)
        },
        properties_get,
    );
    let property_js = property_fn.to_js_function(ctx.realm());
    camel
        .set(
            js_string!("property"),
            JsValue::from(property_js),
            false,
            ctx,
        )
        .map_err(host_error)?;

    // `set_property(name, value)`: a throwing mutator in read-only mode, the
    // real writer otherwise.
    let set_property_js = match read_only {
        Some(sentinel) => throwing_mutator(sentinel, ctx),
        None => {
            let properties_set = properties.clone();
            let set_property_fn = NativeFunction::from_copy_closure_with_captures(
                move |_this, args, props, ctx| {
                    let key = args
                        .first()
                        .unwrap_or(&JsValue::undefined())
                        .to_string(ctx)?
                        .to_std_string_escaped();
                    let val = args.get(1).cloned().unwrap_or(JsValue::undefined());
                    let data = props.get(js_string!("__data"), ctx)?;
                    let data_obj = data.as_object().ok_or_else(|| {
                        boa_engine::JsNativeError::typ().with_message("properties.__data missing")
                    })?;
                    data_obj.set(js_string!(key.as_str()), val, false, ctx)?;
                    Ok(JsValue::undefined())
                },
                properties_set,
            );
            JsValue::from(set_property_fn.to_js_function(ctx.realm()))
        }
    };
    camel
        .set(js_string!("set_property"), set_property_js, false, ctx)
        .map_err(host_error)?;

    let body_val = value_to_js(&exchange.body, ctx)?;
    let body_val = match read_only {
        Some(sentinel) => deep_readonly_wrap(body_val, sentinel, ctx).map_err(host_error)?,
        None => body_val,
    };
    camel
        .set(js_string!("body"), body_val, false, ctx)
        .map_err(host_error)?;

    match read_only {
        Some(sentinel) => readonly_proxy(camel, sentinel, ctx).map_err(host_error),
        None => Ok(camel),
    }
}

/// Build a map-like JS object with get/set/has/remove/keys methods backed by a `__data` object.
///
/// In read-only mode the backing values and the `__data` object are wrapped in
/// throwing proxies and `set`/`remove` are throwing host functions (so an
/// alias captured before the call still refuses).
fn build_map_object(
    map: &HashMap<String, Value>,
    read_only: Option<&ReadOnlySentinel>,
    ctx: &mut Context,
) -> Result<JsObject, JsLanguageError> {
    // Build the backing __data object.
    let data = JsObject::with_null_proto();
    for (k, v) in map {
        let js_val = value_to_js(v, ctx)?;
        let js_val = match read_only {
            Some(sentinel) => deep_readonly_wrap(js_val, sentinel, ctx).map_err(host_error)?,
            None => js_val,
        };
        data.set(js_string!(k.as_str()), js_val, false, ctx)
            .map_err(host_error)?;
    }

    let map_obj = JsObject::with_null_proto();
    let data_value = match read_only {
        Some(sentinel) => {
            JsValue::from(readonly_proxy(data.clone(), sentinel, ctx).map_err(host_error)?)
        }
        None => JsValue::from(data.clone()),
    };
    map_obj
        .set(js_string!("__data"), data_value, false, ctx)
        .map_err(host_error)?;

    // get(key) -> value. The captured backing object already stores
    // read-only-wrapped values in read-only mode.
    let data_get = data.clone();
    let get_fn = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, data_obj, ctx| {
            let key = args
                .first()
                .unwrap_or(&JsValue::undefined())
                .to_string(ctx)?
                .to_std_string_escaped();
            data_obj.get(js_string!(key.as_str()), ctx)
        },
        data_get,
    );
    let get_js = get_fn.to_js_function(ctx.realm());
    map_obj
        .set(js_string!("get"), JsValue::from(get_js), false, ctx)
        .map_err(host_error)?;

    // set(key, value): throwing mutator in read-only mode, real writer otherwise.
    let set_js = match read_only {
        Some(sentinel) => throwing_mutator(sentinel, ctx),
        None => {
            let data_set = data.clone();
            let set_fn = NativeFunction::from_copy_closure_with_captures(
                move |_this, args, data_obj, ctx| {
                    let key = args
                        .first()
                        .unwrap_or(&JsValue::undefined())
                        .to_string(ctx)?
                        .to_std_string_escaped();
                    let val = args.get(1).cloned().unwrap_or(JsValue::undefined());
                    data_obj.set(js_string!(key.as_str()), val, false, ctx)?;
                    Ok(JsValue::undefined())
                },
                data_set,
            );
            JsValue::from(set_fn.to_js_function(ctx.realm()))
        }
    };
    map_obj
        .set(js_string!("set"), set_js, false, ctx)
        .map_err(host_error)?;

    // has(key) -> bool.
    let data_has = data.clone();
    let has_fn = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, data_obj, ctx| {
            let key = args
                .first()
                .unwrap_or(&JsValue::undefined())
                .to_string(ctx)?
                .to_std_string_escaped();
            let has = data_obj.has_own_property(js_string!(key.as_str()), ctx)?;
            Ok(JsValue::from(has))
        },
        data_has,
    );
    let has_js = has_fn.to_js_function(ctx.realm());
    map_obj
        .set(js_string!("has"), JsValue::from(has_js), false, ctx)
        .map_err(host_error)?;

    // remove(key): throwing mutator in read-only mode, real remover otherwise.
    let remove_js = match read_only {
        Some(sentinel) => throwing_mutator(sentinel, ctx),
        None => {
            let data_remove = data.clone();
            let remove_fn = NativeFunction::from_copy_closure_with_captures(
                move |_this, args, data_obj, ctx| {
                    let key = args
                        .first()
                        .unwrap_or(&JsValue::undefined())
                        .to_string(ctx)?
                        .to_std_string_escaped();
                    data_obj.delete_property_or_throw(js_string!(key.as_str()), ctx)?;
                    Ok(JsValue::undefined())
                },
                data_remove,
            );
            JsValue::from(remove_fn.to_js_function(ctx.realm()))
        }
    };
    map_obj
        .set(js_string!("remove"), remove_js, false, ctx)
        .map_err(host_error)?;

    // keys() -> string[].
    let data_keys = data.clone();
    let keys_fn = NativeFunction::from_copy_closure_with_captures(
        move |_this, _args, data_obj, ctx| {
            let keys = data_obj.own_property_keys(ctx)?;
            let js_keys: Vec<JsValue> = keys
                .iter()
                .filter_map(|k| match k {
                    PropertyKey::String(s) => Some(JsValue::from(s.clone())),
                    _ => None,
                })
                .collect();
            let arr = JsArray::from_iter(js_keys, ctx);
            Ok(JsValue::from(arr))
        },
        data_keys,
    );
    let keys_js = keys_fn.to_js_function(ctx.realm());
    map_obj
        .set(js_string!("keys"), JsValue::from(keys_js), false, ctx)
        .map_err(host_error)?;

    match read_only {
        Some(sentinel) => readonly_proxy(map_obj, sentinel, ctx).map_err(host_error),
        None => Ok(map_obj),
    }
}

/// Extract the (possibly mutated) exchange state from the `camel` global.
///
/// `detector` is the privately-retained pristine `Array.isArray` used to
/// identify read-only proxy arrays during conversion.
pub fn extract_camel_state(
    ctx: &mut Context,
    detector: Option<&ArrayDetector>,
) -> Result<JsExchange, JsLanguageError> {
    let camel_val = ctx
        .global_object()
        .get(js_string!("camel"), ctx)
        .map_err(|e| JsLanguageError::ExchangeAccess {
            message: e.to_string(),
        })?;

    let camel = match camel_val.as_object() {
        Some(obj) => obj,
        None => {
            return Err(JsLanguageError::ExchangeAccess {
                message: "camel global is not an object".to_string(),
            });
        }
    };

    let headers = extract_map(
        &camel
            .get(js_string!("headers"), ctx)
            .map_err(|e| JsLanguageError::ExchangeAccess {
                message: e.to_string(),
            })?,
        ctx,
        detector,
    )?;

    let properties = extract_map(
        &camel
            .get(js_string!("properties"), ctx)
            .map_err(|e| JsLanguageError::ExchangeAccess {
                message: e.to_string(),
            })?,
        ctx,
        detector,
    )?;

    let body_js =
        camel
            .get(js_string!("body"), ctx)
            .map_err(|e| JsLanguageError::ExchangeAccess {
                message: e.to_string(),
            })?;
    let body = js_to_value_with_detector(&body_js, ctx, detector)?;

    Ok(JsExchange {
        headers,
        body,
        properties,
    })
}

/// Extract `HashMap<String, Value>` from a map object (reads its `__data` property).
fn extract_map(
    map_val: &JsValue,
    ctx: &mut Context,
    detector: Option<&ArrayDetector>,
) -> Result<HashMap<String, Value>, JsLanguageError> {
    let map_obj = match map_val.as_object() {
        Some(obj) => obj,
        None => return Ok(HashMap::new()),
    };

    let data_val =
        map_obj
            .get(js_string!("__data"), ctx)
            .map_err(|e| JsLanguageError::ExchangeAccess {
                message: e.to_string(),
            })?;

    let data_obj = match data_val.as_object() {
        Some(obj) => obj,
        None => return Ok(HashMap::new()),
    };

    let keys = data_obj
        .own_property_keys(ctx)
        .map_err(|e| JsLanguageError::ExchangeAccess {
            message: e.to_string(),
        })?;

    let mut result = HashMap::new();
    for key in &keys {
        if let PropertyKey::String(s) = key {
            let k = s.to_std_string_escaped();
            let v = data_obj.get(js_string!(k.as_str()), ctx).map_err(|e| {
                JsLanguageError::ExchangeAccess {
                    message: e.to_string(),
                }
            })?;
            // Preserve the typed conversion refusal so the boundary classifies
            // it as `conversion`, not `runtime`. Only a genuinely unrelated
            // error degrades to an exchange-access failure.
            let val = match js_to_value_with_detector(&v, ctx, detector) {
                Ok(val) => val,
                Err(err @ JsLanguageError::TypeConversion { .. }) => return Err(err),
                Err(e) => {
                    return Err(JsLanguageError::ExchangeAccess {
                        message: format!("value extraction failed for key '{k}': {e}"),
                    });
                }
            };
            result.insert(k, val);
        }
    }

    Ok(result)
}
