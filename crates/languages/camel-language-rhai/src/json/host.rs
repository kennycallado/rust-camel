//! Process-wide Rhai host module for the bounded JSON wrapper (change
//! `rhai-json-fidelity`, task 1.4).
//!
//! The module is built once and shared by every sandbox engine (task 1.5
//! registers it globally after the standard package, so its functions shadow
//! the stock `parse_json`/`to_json`). It owns:
//!
//! - the custom type registrations (`json value`, `json number`) that drive
//!   `type_of`/`map_type_name` and the stable error labels;
//! - `parse_json` (strict, bounded, redacted);
//! - `to_json` exact-typed overloads plus the `to_string`/`to_debug`/`to_float`
//!   helpers;
//! - the `JsonValue` surface: string/integer index get/set, `len`, `contains`,
//!   `keys`, `remove`, `push`;
//! - the six refused comparison operators over wrapper/scalar pairs.
//!
//! All bounds are read from the calling engine, never captured. Mutations
//! validate a candidate copy before committing, so a refused set/push leaves
//! the wrapper unchanged.

use super::parse::{self, JsonLimits};
use super::serialize;
use super::{
    JSON_NUMBER_TYPE_NAME, JSON_VALUE_TYPE_NAME, JsonHostError, JsonKind, JsonNumber, JsonValue,
    MAX_JSON_DEPTH, metrics, project_dynamic,
};
use indexmap::IndexMap;
use rhai::{
    Array, Dynamic, EvalAltResult, ImmutableString, LexError, Module, NativeCallContext,
    ParseErrorType, Position, Shared, Variant,
};
use std::sync::OnceLock;

/// Crate-local alias for rhai's raw result shape (the rhai alias is private).
type RhaiResultOf<T> = Result<T, Box<EvalAltResult>>;

/// Process-wide host module, built on first use.
static HOST: OnceLock<Shared<Module>> = OnceLock::new();

/// The registered comparison operators; all refuse with `type-mismatch`.
const COMPARISON_OPS: [&str; 6] = ["==", "!=", "<", ">", "<=", ">="];

/// Return the shared host module, building it once.
pub(crate) fn host_module() -> Shared<Module> {
    HOST.get_or_init(|| Shared::new(build_module())).clone()
}

/// Build the module: custom types plus every registered function.
fn build_module() -> Module {
    let mut module = Module::new();
    module.set_custom_type::<JsonValue>(JSON_VALUE_TYPE_NAME);
    module.set_custom_type::<JsonNumber>(JSON_NUMBER_TYPE_NAME);
    register_parse(&mut module);
    register_to_json(&mut module);
    register_surface(&mut module);
    register_comparisons(&mut module);
    module
}

/// Register `parse_json`.
fn register_parse(module: &mut Module) {
    module.set_native_fn(
        "parse_json",
        |ctx: NativeCallContext, s: ImmutableString| -> RhaiResultOf<Dynamic> {
            let limits = limits_from(&ctx);
            let value = parse::parse(&s, &limits)
                .map_err(|e| host_err(e, "json input", ctx.call_position()))?;
            Ok(project_dynamic(&value))
        },
    );
}

/// Register the exact-typed `to_json` overloads.
///
/// The native-`Map` overload is exact (not `Dynamic`) because Rhai prefers the
/// stock exact `Map` method over a `Dynamic` one.
fn register_to_json(module: &mut Module) {
    module.set_native_fn(
        "to_json",
        |ctx: NativeCallContext, m: &mut rhai::Map| -> RhaiResultOf<String> {
            serialize_dynamic(&ctx, Dynamic::from(m.clone()))
        },
    );
    module.set_native_fn(
        "to_json",
        |ctx: NativeCallContext, a: Array| -> RhaiResultOf<String> {
            serialize_dynamic(&ctx, Dynamic::from(a))
        },
    );
    module.set_native_fn(
        "to_json",
        |ctx: NativeCallContext, v: JsonValue| -> RhaiResultOf<String> {
            serialize_dynamic(&ctx, Dynamic::from(v))
        },
    );
    module.set_native_fn(
        "to_json",
        |ctx: NativeCallContext, n: JsonNumber| -> RhaiResultOf<String> {
            serialize_dynamic(&ctx, Dynamic::from(n))
        },
    );
    module.set_native_fn(
        "to_json",
        |ctx: NativeCallContext, s: ImmutableString| -> RhaiResultOf<String> {
            serialize_dynamic(&ctx, Dynamic::from(s))
        },
    );
    module.set_native_fn(
        "to_json",
        |ctx: NativeCallContext, b: bool| -> RhaiResultOf<String> {
            serialize_dynamic(&ctx, Dynamic::from(b))
        },
    );
    module.set_native_fn(
        "to_json",
        |ctx: NativeCallContext, i: i64| -> RhaiResultOf<String> {
            serialize_dynamic(&ctx, Dynamic::from(i))
        },
    );
    module.set_native_fn(
        "to_json",
        |ctx: NativeCallContext, f: f64| -> RhaiResultOf<String> {
            serialize_dynamic(&ctx, Dynamic::from(f))
        },
    );
    module.set_native_fn(
        "to_json",
        |ctx: NativeCallContext, _v: ()| -> RhaiResultOf<String> {
            serialize_dynamic(&ctx, Dynamic::UNIT)
        },
    );
}

/// Register the wrapper surface: indexers, container helpers, serializers.
fn register_surface(module: &mut Module) {
    module.set_indexer_get_fn(
        |ctx: NativeCallContext,
         obj: &mut JsonValue,
         key: ImmutableString|
         -> RhaiResultOf<Dynamic> {
            get_dynamic(obj, JsonIndex::Key(key), ctx.call_position())
        },
    );
    module.set_indexer_get_fn(
        |ctx: NativeCallContext, obj: &mut JsonValue, idx: i64| -> RhaiResultOf<Dynamic> {
            get_dynamic(obj, JsonIndex::Index(idx), ctx.call_position())
        },
    );
    module.set_indexer_set_fn(
        |ctx: NativeCallContext,
         obj: &mut JsonValue,
         key: ImmutableString,
         val: Dynamic|
         -> RhaiResultOf<()> {
            let limits = limits_from(&ctx);
            set_at(obj, JsonIndex::Key(key), &val, &limits)
                .map_err(|e| host_err(e, JSON_VALUE_TYPE_NAME, ctx.call_position()))
        },
    );
    module.set_indexer_set_fn(
        |ctx: NativeCallContext, obj: &mut JsonValue, idx: i64, val: Dynamic| -> RhaiResultOf<()> {
            let limits = limits_from(&ctx);
            set_at(obj, JsonIndex::Index(idx), &val, &limits)
                .map_err(|e| host_err(e, JSON_VALUE_TYPE_NAME, ctx.call_position()))
        },
    );

    module.set_native_fn("len", |obj: &mut JsonValue| -> RhaiResultOf<i64> {
        match obj.kind() {
            JsonKind::Array(items) => Ok(items.len() as i64),
            JsonKind::Object(entries) => Ok(entries.len() as i64),
            _ => Err(host_err(
                JsonHostError::TypeMismatch,
                JSON_VALUE_TYPE_NAME,
                Position::NONE,
            )),
        }
    });
    module.set_native_fn(
        "contains",
        |obj: &mut JsonValue, key: ImmutableString| -> RhaiResultOf<bool> {
            match obj.kind() {
                JsonKind::Object(entries) => Ok(entries.contains_key(key.as_str())),
                _ => Err(host_err(
                    JsonHostError::TypeMismatch,
                    JSON_VALUE_TYPE_NAME,
                    Position::NONE,
                )),
            }
        },
    );
    module.set_native_fn("keys", |obj: &mut JsonValue| -> RhaiResultOf<Array> {
        match obj.kind() {
            JsonKind::Object(entries) => {
                Ok(entries.keys().map(|k| Dynamic::from(k.clone())).collect())
            }
            _ => Err(host_err(
                JsonHostError::TypeMismatch,
                JSON_VALUE_TYPE_NAME,
                Position::NONE,
            )),
        }
    });
    module.set_native_fn(
        "remove",
        |ctx: NativeCallContext, obj: &mut JsonValue, idx: Dynamic| -> RhaiResultOf<Dynamic> {
            let index = dynamic_index(&idx).ok_or_else(|| {
                host_err(
                    JsonHostError::TypeMismatch,
                    JSON_VALUE_TYPE_NAME,
                    ctx.call_position(),
                )
            })?;
            remove_at(obj, index)
                .map_err(|e| host_err(e, JSON_VALUE_TYPE_NAME, ctx.call_position()))
        },
    );
    module.set_native_fn(
        "push",
        |ctx: NativeCallContext, obj: &mut JsonValue, val: Dynamic| -> RhaiResultOf<()> {
            let limits = limits_from(&ctx);
            push_to(obj, &val, &limits)
                .map_err(|e| host_err(e, JSON_VALUE_TYPE_NAME, ctx.call_position()))
        },
    );

    module.set_native_fn(
        "to_string",
        |ctx: NativeCallContext, v: &mut JsonValue| -> RhaiResultOf<String> {
            serialize::to_json_string(v, ctx.engine().max_string_size())
                .map_err(|e| host_err(e, JSON_VALUE_TYPE_NAME, ctx.call_position()))
        },
    );
    module.set_native_fn(
        "to_debug",
        |ctx: NativeCallContext, v: &mut JsonValue| -> RhaiResultOf<String> {
            serialize::to_json_string(v, ctx.engine().max_string_size())
                .map_err(|e| host_err(e, JSON_VALUE_TYPE_NAME, ctx.call_position()))
        },
    );
    module.set_native_fn(
        "to_string",
        |ctx: NativeCallContext, n: &mut JsonNumber| -> RhaiResultOf<String> {
            number_text(&ctx, n)
        },
    );
    module.set_native_fn(
        "to_debug",
        |ctx: NativeCallContext, n: &mut JsonNumber| -> RhaiResultOf<String> {
            number_text(&ctx, n)
        },
    );
    module.set_native_fn("to_float", |n: &mut JsonNumber| -> RhaiResultOf<f64> {
        n.to_f64()
            .map_err(|e| host_err(e, JSON_NUMBER_TYPE_NAME, Position::NONE))
    });
}

/// Return a number's exact token through the bounded serializer so the calling
/// engine's string cap is enforced the same way as `to_json`.
fn number_text(ctx: &NativeCallContext, number: &JsonNumber) -> RhaiResultOf<String> {
    let tree = JsonValue::new(JsonKind::Number(number.clone()));
    serialize::to_json_string(&tree, ctx.engine().max_string_size())
        .map_err(|e| host_err(e, JSON_NUMBER_TYPE_NAME, ctx.call_position()))
}

/// Register the six refusal guards for one `(A, B)` operand pair.
fn guard_pair<A, B>(module: &mut Module)
where
    A: Variant + Clone,
    B: Variant + Clone,
{
    for op in COMPARISON_OPS {
        module.set_native_fn(
            op,
            |ctx: NativeCallContext, _a: A, _b: B| -> RhaiResultOf<bool> {
                Err(host_err(
                    JsonHostError::TypeMismatch,
                    JSON_VALUE_TYPE_NAME,
                    ctx.call_position(),
                ))
            },
        );
    }
}

/// Register every comparison pair in both operand orders. No comparison
/// engine exists; every registered pair refuses with `type-mismatch`.
fn register_comparisons(module: &mut Module) {
    guard_pair::<JsonValue, i64>(module);
    guard_pair::<i64, JsonValue>(module);
    guard_pair::<JsonValue, f64>(module);
    guard_pair::<f64, JsonValue>(module);
    guard_pair::<JsonValue, String>(module);
    guard_pair::<String, JsonValue>(module);
    guard_pair::<JsonValue, bool>(module);
    guard_pair::<bool, JsonValue>(module);

    guard_pair::<JsonNumber, i64>(module);
    guard_pair::<i64, JsonNumber>(module);
    guard_pair::<JsonNumber, f64>(module);
    guard_pair::<f64, JsonNumber>(module);
    guard_pair::<JsonNumber, String>(module);
    guard_pair::<String, JsonNumber>(module);
    guard_pair::<JsonNumber, bool>(module);
    guard_pair::<bool, JsonNumber>(module);

    guard_pair::<JsonValue, JsonNumber>(module);
    guard_pair::<JsonNumber, JsonValue>(module);

    guard_pair::<JsonValue, JsonValue>(module);
    guard_pair::<JsonNumber, JsonNumber>(module);
}

/// Read the three caps from the calling engine (never captured state).
fn limits_from(ctx: &NativeCallContext) -> JsonLimits {
    JsonLimits {
        max_string_size: ctx.engine().max_string_size(),
        max_array_size: ctx.engine().max_array_size(),
        max_map_size: ctx.engine().max_map_size(),
    }
}

/// Map a redacted module error to a Rhai error carrying only `actual` (a static
/// Rhai type name) and the script call position.
fn host_err(err: JsonHostError, actual: &str, pos: Position) -> Box<EvalAltResult> {
    match err {
        JsonHostError::Parse => Box::new(EvalAltResult::ErrorParsing(
            ParseErrorType::BadInput(LexError::MalformedEscapeSequence(
                "invalid JSON".to_string(),
            )),
            pos,
        )),
        JsonHostError::Limit => Box::new(EvalAltResult::ErrorDataTooLarge(
            "json limit exceeded".to_string(),
            pos,
        )),
        JsonHostError::TypeMismatch => Box::new(EvalAltResult::ErrorMismatchDataType(
            actual.to_string(),
            "expected json".to_string(),
            pos,
        )),
        JsonHostError::Arithmetic => Box::new(EvalAltResult::ErrorArithmetic(
            "json number is not finite".to_string(),
            pos,
        )),
    }
}

/// Shared serialization path: convert `d` to a bounded tree, then serialize it
/// with the exact output bound (`max_string_size`, `0` = unlimited).
fn serialize_dynamic(ctx: &NativeCallContext, d: Dynamic) -> RhaiResultOf<String> {
    let limits = limits_from(ctx);
    let actual = d.type_name();
    let tree = dynamic_to_json(&d, &limits, MAX_JSON_DEPTH)
        .map_err(|e| host_err(e, actual, ctx.call_position()))?;
    serialize::to_json_string(&tree, ctx.engine().max_string_size())
        .map_err(|e| host_err(e, JSON_VALUE_TYPE_NAME, ctx.call_position()))
}

/// Convert a Rhai `Dynamic` into a bounded [`JsonValue`] tree.
///
/// Accepted: `JsonValue`, `JsonNumber`, string, bool, `i64`, finite `f64`,
/// unit, native `Map`, native `Array`. Everything else (non-finite float,
/// `FnPtr`, timestamp, custom types) is a payload-free `TypeMismatch` with no
/// `Debug` fallback. Native `Map` keys are emitted sorted (`rhai::Map` is a
/// `BTreeMap`).
fn dynamic_to_json(
    d: &Dynamic,
    limits: &JsonLimits,
    depth_budget: usize,
) -> Result<JsonValue, JsonHostError> {
    if d.is_unit() {
        return Ok(JsonValue::new(JsonKind::Null));
    }
    if d.is::<bool>() {
        return Ok(JsonValue::new(JsonKind::Bool(d.clone().cast::<bool>())));
    }
    if d.is_int() {
        let token = d.clone().cast::<i64>().to_string();
        return Ok(JsonValue::new(JsonKind::Number(JsonNumber::new(token))));
    }
    if d.is_float() {
        let value = d.clone().cast::<f64>();
        if !value.is_finite() {
            return Err(JsonHostError::TypeMismatch);
        }
        return Ok(JsonValue::new(JsonKind::Number(JsonNumber::new(format!(
            "{value}"
        )))));
    }
    if d.is_string() {
        return Ok(JsonValue::new(JsonKind::String(d.clone().cast::<String>())));
    }
    if d.is::<JsonValue>() {
        let value = d.clone().cast::<JsonValue>();
        return if value.depth() <= depth_budget {
            Ok(value)
        } else {
            Err(JsonHostError::Limit)
        };
    }
    if d.is::<JsonNumber>() {
        return Ok(JsonValue::new(JsonKind::Number(
            d.clone().cast::<JsonNumber>(),
        )));
    }
    if d.is::<rhai::Map>() {
        if depth_budget == 0 {
            return Err(JsonHostError::Limit);
        }
        let map = d.clone().cast::<rhai::Map>();
        if limits.max_map_size != 0 && map.len() > limits.max_map_size {
            return Err(JsonHostError::Limit);
        }
        let mut entries = IndexMap::with_capacity(map.len());
        for (key, value) in map.iter() {
            entries.insert(
                key.to_string(),
                dynamic_to_json(value, limits, depth_budget - 1)?,
            );
        }
        let value = JsonValue::new(JsonKind::Object(entries));
        check_size_bound(&value, limits)?;
        return Ok(value);
    }
    if d.is::<rhai::Array>() {
        if depth_budget == 0 {
            return Err(JsonHostError::Limit);
        }
        let array = d.clone().cast::<rhai::Array>();
        if limits.max_array_size != 0 && array.len() > limits.max_array_size {
            return Err(JsonHostError::Limit);
        }
        let mut items = Vec::with_capacity(array.len());
        for value in array.iter() {
            items.push(dynamic_to_json(value, limits, depth_budget - 1)?);
        }
        let value = JsonValue::new(JsonKind::Array(items));
        check_size_bound(&value, limits)?;
        return Ok(value);
    }
    Err(JsonHostError::TypeMismatch)
}

/// Refuse when the cached size estimate exceeds `max_string_size`.
fn check_size_bound(value: &JsonValue, limits: &JsonLimits) -> Result<(), JsonHostError> {
    if limits.max_string_size != 0 && value.size() > limits.max_string_size {
        Err(JsonHostError::Limit)
    } else {
        Ok(())
    }
}

/// A resolved wrapper index.
enum JsonIndex {
    Key(ImmutableString),
    Index(i64),
}

/// Convert a script-supplied `Dynamic` index to [`JsonIndex`].
fn dynamic_index(index: &Dynamic) -> Option<JsonIndex> {
    if index.is_int() {
        Some(JsonIndex::Index(index.clone().cast::<i64>()))
    } else if index.is_string() {
        Some(JsonIndex::Key(ImmutableString::from(
            index.clone().cast::<String>(),
        )))
    } else {
        None
    }
}

/// Resolve a possibly-negative integer index against `len`.
fn resolve_index(raw: i64, len: usize) -> Option<usize> {
    if raw >= 0 {
        let index = raw as usize;
        (index < len).then_some(index)
    } else {
        let from_end = raw.unsigned_abs() as usize;
        (from_end <= len).then(|| len - from_end)
    }
}

/// Read one wrapper element, projecting it. A missing key and a JSON null both
/// project to unit. A wrong-container target is a type mismatch; an
/// out-of-range integer index raises `ErrorArrayBounds`.
fn get_dynamic(obj: &JsonValue, index: JsonIndex, pos: Position) -> RhaiResultOf<Dynamic> {
    match (obj.kind(), index) {
        (JsonKind::Object(entries), JsonIndex::Key(key)) => Ok(entries
            .get(key.as_str())
            .map(project_dynamic)
            .unwrap_or(Dynamic::UNIT)),
        (JsonKind::Array(items), JsonIndex::Index(raw)) => match resolve_index(raw, items.len()) {
            Some(index) => Ok(project_dynamic(&items[index])),
            None => Err(Box::new(EvalAltResult::ErrorArrayBounds(
                items.len(),
                raw,
                pos,
            ))),
        },
        _ => Err(host_err(
            JsonHostError::TypeMismatch,
            JSON_VALUE_TYPE_NAME,
            pos,
        )),
    }
}

/// Set `index` on `obj`, validating a candidate copy first so a refusal leaves
/// `obj` untouched.
fn set_at(
    obj: &mut JsonValue,
    index: JsonIndex,
    value: &Dynamic,
    limits: &JsonLimits,
) -> Result<(), JsonHostError> {
    let inserted = dynamic_to_json(value, limits, MAX_JSON_DEPTH)?;

    let mut candidate = obj.kind().clone();
    match (&mut candidate, index) {
        (JsonKind::Object(entries), JsonIndex::Key(key)) => {
            entries.insert(key.to_string(), inserted);
        }
        (JsonKind::Array(items), JsonIndex::Index(raw)) => {
            let resolved = resolve_index(raw, items.len()).ok_or(JsonHostError::TypeMismatch)?;
            items[resolved] = inserted;
        }
        _ => return Err(JsonHostError::TypeMismatch),
    }

    check_candidate_bounds(&candidate, limits)?;
    commit(obj, candidate)
}

/// Append `value` to an array wrapper, validating a candidate copy first.
fn push_to(obj: &mut JsonValue, value: &Dynamic, limits: &JsonLimits) -> Result<(), JsonHostError> {
    let inserted = dynamic_to_json(value, limits, MAX_JSON_DEPTH)?;
    let JsonKind::Array(items) = obj.kind() else {
        return Err(JsonHostError::TypeMismatch);
    };

    let mut candidate = items.clone();
    candidate.push(inserted);
    let kind = JsonKind::Array(candidate);
    check_candidate_bounds(&kind, limits)?;
    commit(obj, kind)
}

/// Shift-remove one element, returning it (or unit when absent).
fn remove_at(obj: &mut JsonValue, index: JsonIndex) -> Result<Dynamic, JsonHostError> {
    let removed = match (obj.kind(), &index) {
        (JsonKind::Object(entries), JsonIndex::Key(key)) => {
            entries.get(key.as_str()).map(project_dynamic)
        }
        (JsonKind::Array(items), JsonIndex::Index(raw)) => {
            resolve_index(*raw, items.len()).map(|resolved| project_dynamic(&items[resolved]))
        }
        _ => return Err(JsonHostError::TypeMismatch),
    };
    let Some(removed) = removed else {
        return Ok(Dynamic::UNIT);
    };

    obj.mutate(|kind| match (kind, &index) {
        (JsonKind::Object(entries), JsonIndex::Key(key)) => {
            entries.shift_remove(key.as_str());
            Ok(())
        }
        (JsonKind::Array(items), JsonIndex::Index(raw)) => {
            if let Some(resolved) = resolve_index(*raw, items.len()) {
                items.remove(resolved);
            }
            Ok(())
        }
        _ => Err(JsonHostError::TypeMismatch),
    })?;
    Ok(removed)
}

/// Recheck the exact candidate depth, the array/map caps and the cached size
/// estimate before committing a candidate kind.
///
/// The depth is the candidate's own `metrics` depth (not the existing subtree
/// depth plus the inserted value depth): unrelated siblings do not deepen the
/// insertion path, and a self-assignment at the cap still exceeds `1 + depth`.
///
/// The cached size estimate deliberately undercounts quotes and escape
/// expansion, so it is a lower bound only: it can never let a mutation through
/// that the exact bounded serializer would accept, and the serializer remains
/// the authoritative output bound for `to_json`/`to_string`/`to_debug`.
fn check_candidate_bounds(kind: &JsonKind, limits: &JsonLimits) -> Result<(), JsonHostError> {
    match kind {
        JsonKind::Array(items) => {
            if limits.max_array_size != 0 && items.len() > limits.max_array_size {
                return Err(JsonHostError::Limit);
            }
        }
        JsonKind::Object(entries) => {
            if limits.max_map_size != 0 && entries.len() > limits.max_map_size {
                return Err(JsonHostError::Limit);
            }
        }
        _ => return Ok(()),
    }
    let (depth, size) = metrics(kind);
    if depth > MAX_JSON_DEPTH {
        return Err(JsonHostError::Limit);
    }
    if limits.max_string_size != 0 && size > limits.max_string_size {
        return Err(JsonHostError::Limit);
    }
    Ok(())
}

/// Copy-on-write commit of a validated candidate kind.
fn commit(obj: &mut JsonValue, kind: JsonKind) -> Result<(), JsonHostError> {
    obj.mutate(|slot| {
        *slot = kind;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::{JsonIndex, host_module, push_to, set_at};
    use crate::json::parse::{JsonLimits, parse};
    use crate::json::serialize::to_json_string;
    use crate::json::{
        JSON_NUMBER_TYPE_NAME, JSON_VALUE_TYPE_NAME, JsonHostError, JsonKind, JsonValue,
    };
    use rhai::packages::{Package, StandardPackage};
    use rhai::{Dynamic, Engine, EvalAltResult, Scope};

    fn host_engine() -> Engine {
        let mut engine = Engine::new_raw();
        StandardPackage::new().register_into_engine(&mut engine);
        engine.register_global_module(host_module());
        engine.set_max_string_size(1_048_576);
        engine.set_max_array_size(10_000);
        engine.set_max_map_size(10_000);
        engine
    }

    fn unlimited() -> JsonLimits {
        JsonLimits {
            max_string_size: 0,
            max_array_size: 0,
            max_map_size: 0,
        }
    }

    fn parsed(input: &str) -> JsonValue {
        parse(input, &unlimited()).expect("test input parses")
    }

    fn rendered(value: &JsonValue) -> String {
        to_json_string(value, 0).expect("test value serializes")
    }

    #[test]
    fn host_module_is_single_instance() {
        assert!(rhai::Shared::ptr_eq(&host_module(), &host_module()));
    }

    #[test]
    fn host_module_registers_types() {
        let module = host_module();
        assert_eq!(
            module.get_custom_type_display::<JsonValue>(),
            Some(JSON_VALUE_TYPE_NAME)
        );
        assert_eq!(
            module.get_custom_type_display::<crate::json::JsonNumber>(),
            Some(JSON_NUMBER_TYPE_NAME)
        );
    }

    #[test]
    fn host_shadows_stock_parse_json() {
        let engine = host_engine();
        let out = engine
            .eval::<String>("type_of(parse_json(\"{}\"))")
            .expect("host parse_json runs");
        assert_eq!(out, JSON_VALUE_TYPE_NAME);
    }

    #[test]
    fn host_shadows_stock_to_json() {
        let engine = host_engine();
        let out = engine
            .eval::<String>("to_json(parse_json(\"[1,2]\"))")
            .expect("host to_json runs");
        assert_eq!(out, "[1,2]");
    }

    #[test]
    fn host_shadows_stock_to_json_map_nonfinite() {
        let engine = host_engine();
        let err = engine
            .eval::<String>("to_json(#{x: 0.0/0.0})")
            .expect_err("non-finite float must be refused");
        assert!(matches!(
            err.unwrap_inner(),
            EvalAltResult::ErrorMismatchDataType(..)
        ));
    }

    #[test]
    fn host_indexer_roundtrip_smoke() {
        let engine = host_engine();
        let out = engine
            .eval::<String>("let j = parse_json(\"{\\\"a\\\":1}\"); j[\"a\"] = 2; to_json(j)")
            .expect("index round-trip runs");
        assert_eq!(out, "{\"a\":2}");
    }

    #[test]
    fn host_get_out_of_range_error_array_bounds() {
        // Pins the exact inner variant that the `LanguageError::EvalFailure`
        // boundary erases: an out-of-range integer read raises
        // `ErrorArrayBounds(len, idx, pos)`.
        let engine = host_engine();
        let err = engine
            .eval::<Dynamic>(r#"let j = parse_json("[1]"); j[5]"#)
            .expect_err("out-of-range index must raise ErrorArrayBounds");
        assert!(matches!(
            err.unwrap_inner(),
            EvalAltResult::ErrorArrayBounds(1, 5, _)
        ));
    }

    #[test]
    fn host_compare_guard_smoke() {
        let engine = host_engine();
        let assert_refused = |script: &str| {
            let err = engine
                .eval::<bool>(script)
                .expect_err("registered comparison must refuse");
            assert!(
                matches!(err.unwrap_inner(), EvalAltResult::ErrorMismatchDataType(..)),
                "{script} -> {err:?}"
            );
        };

        let wrappers = [r#"parse_json("{}")"#, r#"parse_json("1.5")"#];
        let scalars = ["2", "2.5", "\"s\"", "true"];
        let mut cases = 0_usize;
        for op in ["==", "!=", "<", ">", "<=", ">="] {
            // 16 wrapper/scalar operand orders.
            for wrapper in wrappers {
                for scalar in scalars {
                    assert_refused(&format!("{wrapper} {op} {scalar}"));
                    assert_refused(&format!("{scalar} {op} {wrapper}"));
                    cases += 2;
                }
            }
            // Two cross-wrapper orders.
            for (a, b) in [(wrappers[0], wrappers[1]), (wrappers[1], wrappers[0])] {
                assert_refused(&format!("{a} {op} {b}"));
                cases += 1;
            }
            // Two same-wrapper pairs.
            for wrapper in wrappers {
                assert_refused(&format!("{wrapper} {op} {wrapper}"));
                cases += 1;
            }
        }
        assert_eq!(cases, 120);
    }

    #[test]
    fn host_run_with_scope_push_rollback() {
        let mut engine = host_engine();
        engine.set_max_array_size(2);
        let original = parsed("[1,2]");
        let mut scope = Scope::new();
        scope.push("j", original.clone());

        let err = engine
            .run_with_scope(&mut scope, "j.push(3)")
            .expect_err("cap violation must abort the run");
        assert!(matches!(
            err.unwrap_inner(),
            EvalAltResult::ErrorDataTooLarge(..)
        ));

        let after = scope.get_value::<JsonValue>("j").expect("j remains bound");
        assert_eq!(rendered(&after), rendered(&original));
    }

    #[test]
    fn host_set_at_depth_limit_unchanged() {
        // Valid object of depth 127 (its keyed subtree has depth 126).
        let deep126 = parsed(&format!("{}{}", "[".repeat(126), "]".repeat(126)));
        let entries: indexmap::IndexMap<String, JsonValue> =
            [("a".to_string(), deep126)].into_iter().collect();
        let mut obj = JsonValue::new(JsonKind::Object(entries));
        assert_eq!(obj.depth(), 127);
        let before = rendered(&obj);

        // A depth-128 array at a new root key makes the candidate depth 129.
        let deep128 = parsed(&format!("{}{}", "[".repeat(128), "]".repeat(128)));
        let err = set_at(
            &mut obj,
            JsonIndex::Key("b".into()),
            &Dynamic::from(deep128),
            &unlimited(),
        )
        .expect_err("candidate depth 129 must be refused");
        assert_eq!(err, JsonHostError::Limit);
        assert_eq!(rendered(&obj), before);
    }

    #[test]
    fn host_set_at_exact_depth_boundary() {
        // Parsed object containing a depth-6 subtree -> object depth 7.
        let deep6 = parsed(&format!("{}{}", "[".repeat(6), "]".repeat(6)));
        let entries: indexmap::IndexMap<String, JsonValue> =
            [("a".to_string(), deep6)].into_iter().collect();
        let mut obj = JsonValue::new(JsonKind::Object(entries));
        assert_eq!(obj.depth(), 7);

        // A depth-127 array at a different root key reaches exact depth 128.
        let deep127 = parsed(&format!("{}{}", "[".repeat(127), "]".repeat(127)));
        set_at(
            &mut obj,
            JsonIndex::Key("b".into()),
            &Dynamic::from(deep127),
            &unlimited(),
        )
        .expect("a depth-127 insert into a depth-7 object is legal");
        assert_eq!(obj.depth(), 128);

        // Replacing the old deep child with a depth-1 array keeps depth 128.
        set_at(
            &mut obj,
            JsonIndex::Key("a".into()),
            &Dynamic::from(parsed("[0]")),
            &unlimited(),
        )
        .expect("replacing the deep child keeps depth 128");
        assert_eq!(obj.depth(), 128);
    }

    #[test]
    fn host_string_methods_enforce_exact_output_limit() {
        let mut engine = host_engine();
        engine.set_max_string_size(9);
        let value = engine
            .eval::<JsonValue>("parse_json(\"[1,2,3,4]\")")
            .expect("a 9-byte array parses at the cap");
        assert_eq!(rendered(&value), "[1,2,3,4]");

        let mut scope = Scope::new();
        scope.push("j", value);
        engine.set_max_string_size(8);

        let err = engine
            .eval_with_scope::<String>(&mut scope, "j.to_string()")
            .expect_err("to_string must refuse above the cap");
        assert!(matches!(
            err.unwrap_inner(),
            EvalAltResult::ErrorDataTooLarge(..)
        ));
        let err = engine
            .eval_with_scope::<String>(&mut scope, "j.to_debug()")
            .expect_err("to_debug must refuse above the cap");
        assert!(matches!(
            err.unwrap_inner(),
            EvalAltResult::ErrorDataTooLarge(..)
        ));

        engine.set_max_string_size(9);
        assert_eq!(
            engine
                .eval_with_scope::<String>(&mut scope, "j.to_string()")
                .expect("9 bytes is exactly the cap"),
            "[1,2,3,4]"
        );
        assert_eq!(
            engine
                .eval_with_scope::<String>(&mut scope, "j.to_debug()")
                .expect("9 bytes is exactly the cap"),
            "[1,2,3,4]"
        );
    }

    #[test]
    fn host_set_at_size_limit_unchanged() {
        let mut obj = parsed("{}");
        let before = rendered(&obj);
        let limits = JsonLimits {
            max_string_size: 8,
            ..unlimited()
        };

        let err = set_at(
            &mut obj,
            JsonIndex::Key("key".into()),
            &Dynamic::from("value"),
            &limits,
        )
        .expect_err("oversize set must be refused");
        assert_eq!(err, JsonHostError::Limit);
        assert_eq!(rendered(&obj), before);
    }

    #[test]
    fn host_push_cap_limit_unchanged() {
        let mut arr = parsed("[1,2]");
        let limits = JsonLimits {
            max_array_size: 2,
            ..unlimited()
        };

        let err = push_to(&mut arr, &Dynamic::from(3_i64), &limits)
            .expect_err("array cap must refuse the push");
        assert_eq!(err, JsonHostError::Limit);
        assert_eq!(rendered(&arr), "[1,2]");
    }

    #[test]
    fn host_set_at_wrong_container_type_mismatch() {
        let mut obj = parsed("{}");
        let err = set_at(
            &mut obj,
            JsonIndex::Index(0),
            &Dynamic::from(1_i64),
            &unlimited(),
        )
        .expect_err("integer index on an object must be a type mismatch");
        assert_eq!(err, JsonHostError::TypeMismatch);
    }
}
