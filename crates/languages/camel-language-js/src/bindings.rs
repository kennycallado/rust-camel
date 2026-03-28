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
    value::{js_to_value, value_to_js},
};

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

/// Replaces the default `console` with a tracing-backed version.
///
/// NOTE: `Context::default()` already registers a built-in `console`,
/// so we overwrite via `global_object().set()` instead of `register_global_property`.
pub fn register_console(ctx: &mut Context) {
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

    let _ = ctx
        .global_object()
        .set(js_string!("console"), JsValue::from(console), false, ctx);
}

/// Build the `camel` global object with `headers`, `properties`, and `body`.
pub fn build_camel_global(
    exchange: &JsExchange,
    ctx: &mut Context,
) -> Result<JsObject, JsLanguageError> {
    let camel = JsObject::with_null_proto();

    let headers = build_map_object(&exchange.headers, ctx)?;
    camel
        .set(js_string!("headers"), JsValue::from(headers), false, ctx)
        .map_err(|e| JsLanguageError::Execution {
            message: e.to_string(),
        })?;

    let properties = build_map_object(&exchange.properties, ctx)?;
    camel
        .set(
            js_string!("properties"),
            JsValue::from(properties),
            false,
            ctx,
        )
        .map_err(|e| JsLanguageError::Execution {
            message: e.to_string(),
        })?;

    let body_val = value_to_js(&exchange.body, ctx)?;
    camel
        .set(js_string!("body"), body_val, false, ctx)
        .map_err(|e| JsLanguageError::Execution {
            message: e.to_string(),
        })?;

    Ok(camel)
}

/// Build a map-like JS object with get/set/has/remove/keys methods backed by a `__data` object.
fn build_map_object(
    map: &HashMap<String, Value>,
    ctx: &mut Context,
) -> Result<JsObject, JsLanguageError> {
    // Build the backing __data object.
    let data = JsObject::with_null_proto();
    for (k, v) in map {
        let js_val = value_to_js(v, ctx)?;
        data.set(js_string!(k.as_str()), js_val, false, ctx)
            .map_err(|e| JsLanguageError::Execution {
                message: e.to_string(),
            })?;
    }

    let map_obj = JsObject::with_null_proto();
    map_obj
        .set(
            js_string!("__data"),
            JsValue::from(data.clone()),
            false,
            ctx,
        )
        .map_err(|e| JsLanguageError::Execution {
            message: e.to_string(),
        })?;

    // get(key) -> value.
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
        .map_err(|e| JsLanguageError::Execution {
            message: e.to_string(),
        })?;

    // set(key, value).
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
    let set_js = set_fn.to_js_function(ctx.realm());
    map_obj
        .set(js_string!("set"), JsValue::from(set_js), false, ctx)
        .map_err(|e| JsLanguageError::Execution {
            message: e.to_string(),
        })?;

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
            Ok(JsValue::Boolean(has))
        },
        data_has,
    );
    let has_js = has_fn.to_js_function(ctx.realm());
    map_obj
        .set(js_string!("has"), JsValue::from(has_js), false, ctx)
        .map_err(|e| JsLanguageError::Execution {
            message: e.to_string(),
        })?;

    // remove(key).
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
    let remove_js = remove_fn.to_js_function(ctx.realm());
    map_obj
        .set(js_string!("remove"), JsValue::from(remove_js), false, ctx)
        .map_err(|e| JsLanguageError::Execution {
            message: e.to_string(),
        })?;

    // keys() -> string[].
    let data_keys = data.clone();
    let keys_fn = NativeFunction::from_copy_closure_with_captures(
        move |_this, _args, data_obj, ctx| {
            let keys = data_obj.own_property_keys(ctx)?;
            let js_keys: Vec<JsValue> = keys
                .iter()
                .filter_map(|k| match k {
                    PropertyKey::String(s) => Some(JsValue::String(s.clone())),
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
        .map_err(|e| JsLanguageError::Execution {
            message: e.to_string(),
        })?;

    Ok(map_obj)
}

/// Extract the (possibly mutated) exchange state from the `camel` global.
pub fn extract_camel_state(ctx: &mut Context) -> Result<JsExchange, JsLanguageError> {
    let camel_val = ctx
        .global_object()
        .get(js_string!("camel"), ctx)
        .map_err(|e| JsLanguageError::ExchangeAccess {
            message: e.to_string(),
        })?;

    let camel = match camel_val {
        JsValue::Object(ref obj) => obj.clone(),
        _ => {
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
    )?;

    let properties = extract_map(
        &camel
            .get(js_string!("properties"), ctx)
            .map_err(|e| JsLanguageError::ExchangeAccess {
                message: e.to_string(),
            })?,
        ctx,
    )?;

    let body_js =
        camel
            .get(js_string!("body"), ctx)
            .map_err(|e| JsLanguageError::ExchangeAccess {
                message: e.to_string(),
            })?;
    let body = js_to_value(&body_js, ctx)?;

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
) -> Result<HashMap<String, Value>, JsLanguageError> {
    let map_obj = match map_val {
        JsValue::Object(obj) => obj.clone(),
        _ => return Ok(HashMap::new()),
    };

    let data_val =
        map_obj
            .get(js_string!("__data"), ctx)
            .map_err(|e| JsLanguageError::ExchangeAccess {
                message: e.to_string(),
            })?;

    let data_obj = match data_val {
        JsValue::Object(obj) => obj,
        _ => return Ok(HashMap::new()),
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
            let val = js_to_value(&v, ctx).map_err(|e| JsLanguageError::ExchangeAccess {
                message: format!("value extraction failed for key '{k}': {e}"),
            })?;
            result.insert(k, val);
        }
    }

    Ok(result)
}
