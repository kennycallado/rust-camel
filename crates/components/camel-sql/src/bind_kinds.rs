//! Bind-kind normalization for SQL bindings.
//!
//! Owns the bind domain: the default JSON binder, the per-position wire
//! encodings (`BindTarget`), the single- and batch-mode normalization loops,
//! and the described-mode eligibility matrix that decides how one statement
//! is bound. Bind-kind safety rationale lives with the batch execution code
//! in `producer`.

use camel_component_api::CamelError;
use sqlx::Any;
use sqlx::any::AnyArguments;
use sqlx::any::AnyTypeInfoKind;
use sqlx::query::Query;

/// Binds JSON values to a sqlx query.
///
/// Handles: Null, Bool, Number (i64/f64), String, Array/Object (serialized to JSON string).
pub(crate) fn bind_json_values<'q>(
    mut query: Query<'q, Any, AnyArguments>,
    values: &'q [serde_json::Value],
) -> Query<'q, Any, AnyArguments> {
    for value in values {
        query = bind_single_json_value(query, value);
    }
    query
}

/// Binds one JSON value through the default JSON path.
///
/// Handles: Null, Bool, Number (i64/f64), String, Array/Object (serialized to JSON string).
fn bind_single_json_value<'q>(
    query: Query<'q, Any, AnyArguments>,
    value: &'q serde_json::Value,
) -> Query<'q, Any, AnyArguments> {
    match value {
        serde_json::Value::Null => query.bind(Option::<String>::None),
        serde_json::Value::Bool(b) => query.bind(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                query.bind(i)
            } else if let Some(f) = n.as_f64() {
                query.bind(f)
            } else {
                query.bind(n.to_string())
            }
        }
        serde_json::Value::String(s) => query.bind(s.as_str()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => query.bind(value.to_string()),
    }
}

// ── Batch bind-kind normalization (bd rc-fef4z) ──────────────────────

/// Conservative exactness limit for integers bound as f64 (owner-specified
/// policy): integers with magnitude above 2^53 fail with a typed error
/// instead of silently rounding.
const F64_EXACT_INT_LIMIT: i128 = 9_007_199_254_740_992;

/// Exactly representable `i64::MIN` as f64 (`-2^63`).
const I64_MIN_AS_F64: f64 = -9_223_372_036_854_775_808.0;

/// The first f64 above every `i64` value (`2^63`; `i64::MAX as f64` rounds
/// up to this, so the upper bound is exclusive).
const I64_BOUND_EXCLUSIVE_AS_F64: f64 = 9_223_372_036_854_775_808.0;

/// Wire bind encoding for a described numeric parameter position.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BindTarget {
    I16,
    I32,
    I64,
    F32,
    F64,
}

impl BindTarget {
    fn name(self) -> &'static str {
        match self {
            BindTarget::I16 => "SMALLINT",
            BindTarget::I32 => "INTEGER",
            BindTarget::I64 => "BIGINT",
            BindTarget::F32 => "REAL",
            BindTarget::F64 => "DOUBLE PRECISION",
        }
    }
}

/// Maps a described parameter type to its exact wire bind encoding.
///
/// Broad `i64`/`f64` categories are not used: an `i64` payload under a
/// described `int4`/`int2` OID fails the server's binary width check.
pub(crate) fn bind_target_from_any_kind(kind: AnyTypeInfoKind) -> Option<BindTarget> {
    match kind {
        AnyTypeInfoKind::SmallInt => Some(BindTarget::I16),
        AnyTypeInfoKind::Integer => Some(BindTarget::I32),
        AnyTypeInfoKind::BigInt => Some(BindTarget::I64),
        AnyTypeInfoKind::Real => Some(BindTarget::F32),
        AnyTypeInfoKind::Double => Some(BindTarget::F64),
        _ => None,
    }
}

/// Fallback scan when the server description is unavailable: a parameter
/// position binds as the float encoding once any row carries a float JSON
/// number there; positions carrying only integers bind as the integer
/// encoding; positions without any JSON number keep the default binding.
pub(crate) fn scan_bind_targets(rows: &[&[serde_json::Value]]) -> Vec<Option<BindTarget>> {
    let width = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    let mut targets = vec![None; width];

    for row in rows {
        for (position, value) in row.iter().enumerate() {
            let serde_json::Value::Number(n) = value else {
                continue;
            };
            // serde_json never converts an f64-backed number in `as_i64`,
            // so a float literal is exactly: no i64, but an f64.
            if n.as_i64().is_none() && n.as_f64().is_some() {
                targets[position] = Some(BindTarget::F64);
            } else if targets[position].is_none() && n.as_i64().is_some() {
                targets[position] = Some(BindTarget::I64);
            }
        }
    }

    targets
}

/// The default JSON binder's wire family for one payload (bd rc-mn139).
///
/// This is what `bind_single_json_value` would put on the wire: strings,
/// arrays and objects bind as raw UTF-8 JSON text; numbers as int8/f64
/// bytes; bools as one byte; null as a typed NULL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PayloadWire {
    Null,
    Bool,
    Numeric,
    Text,
}

/// The default JSON binder's wire family for one JSON payload.
pub(crate) fn payload_wire_family(payload: &serde_json::Value) -> PayloadWire {
    match payload {
        serde_json::Value::Null => PayloadWire::Null,
        serde_json::Value::Bool(_) => PayloadWire::Bool,
        serde_json::Value::Number(_) => PayloadWire::Numeric,
        serde_json::Value::String(_)
        | serde_json::Value::Array(_)
        | serde_json::Value::Object(_) => PayloadWire::Text,
    }
}

/// The wire family a described (server-reported) parameter kind accepts.
///
/// Returns `None` for kinds outside the closed set the default binder can
/// encode faithfully; such a position makes the statement ineligible for
/// described mode. `Blob` maps to `Text`: bytea carries raw bytes, the same
/// family as a string payload's raw UTF-8 bytes.
pub(crate) fn described_wire_family(kind: AnyTypeInfoKind) -> Option<PayloadWire> {
    match kind {
        AnyTypeInfoKind::Bool => Some(PayloadWire::Bool),
        AnyTypeInfoKind::Text => Some(PayloadWire::Text),
        AnyTypeInfoKind::Blob => Some(PayloadWire::Text),
        AnyTypeInfoKind::SmallInt
        | AnyTypeInfoKind::Integer
        | AnyTypeInfoKind::BigInt
        | AnyTypeInfoKind::Real
        | AnyTypeInfoKind::Double => Some(PayloadWire::Numeric),
        AnyTypeInfoKind::Null => Some(PayloadWire::Null),
    }
}

/// Whether a statement described as `kinds` may execute in described mode
/// with `bindings` (bd rc-mn139).
///
/// A position with a numeric bind target is always fine: numbers are
/// normalized to that encoding and non-numeric payloads fail with a typed
/// error at bind time. Any other position is compatible only when the
/// payload's default-binder wire family is NULL (all kinds accept it) or
/// matches the described kind's family. A payload the binder cannot encode
/// faithfully under the described OID — a bool under TEXT, a number under
/// TEXT — makes the whole statement ineligible.
pub(crate) fn described_mode_eligible(
    kinds: &[AnyTypeInfoKind],
    bindings: &[serde_json::Value],
) -> bool {
    if kinds.len() < bindings.len() {
        return false;
    }

    kinds.iter().zip(bindings.iter()).all(|(kind, payload)| {
        if bind_target_from_any_kind(*kind).is_some() {
            return true;
        }
        let payload_family = payload_wire_family(payload);
        if payload_family == PayloadWire::Null {
            return true;
        }
        described_wire_family(*kind) == Some(payload_family)
    })
}

/// Target assembly for the hybrid fallback (bd rc-mn139).
///
/// Every described numeric target is kept, so the checked conversion guards
/// still fire at positions the server described. Only positions without a
/// described numeric target resolve from this message's bind scan.
pub(crate) fn hybrid_fallback_targets(
    described: &[Option<BindTarget>],
    bindings: &[serde_json::Value],
) -> Vec<Option<BindTarget>> {
    let scanned = scan_bind_targets(&[bindings]);
    described
        .iter()
        .enumerate()
        .map(|(position, target)| target.or_else(|| scanned.get(position).copied().flatten()))
        .collect()
}

/// A JSON value normalized for one parameter position's wire encoding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum NormalizedBind {
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Null(BindTarget),
    /// No described numeric target — bind through the default JSON path.
    Default,
}

/// Which bind a normalization failure belongs to (bd rc-mn139).
///
/// `BatchItem` reproduces the batch wording byte-for-byte; `Single` names
/// only the parameter position of the one statement being bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindCtx {
    BatchItem(usize),
    Single,
}

impl BindCtx {
    /// Prefix naming the offending bind in a typed error.
    fn item_prefix(self) -> String {
        match self {
            BindCtx::BatchItem(item) => format!("Batch item {item}, "),
            BindCtx::Single => String::new(),
        }
    }

    /// Mode name used by the non-numeric rejection clause.
    fn mode_name(self) -> &'static str {
        match self {
            BindCtx::BatchItem(_) => "batch",
            BindCtx::Single => "single",
        }
    }
}

/// Normalizes one JSON value against its position's bind target.
///
/// Numeric values are converted to the target encoding with checked
/// conversions (ties-to-even rounding for float -> integer, the 2^53
/// conservative guard for integer -> float, f32 overflow/underflow checks).
/// Non-numeric values at described numeric positions fail: raw text or
/// bool payloads must never reach a numeric OID, because an 8-byte string
/// silently reinterprets as float8 bits.
pub(crate) fn normalize_for_target(
    value: &serde_json::Value,
    target: Option<BindTarget>,
    ctx: BindCtx,
    position: usize,
) -> Result<NormalizedBind, CamelError> {
    let Some(target) = target else {
        return Ok(NormalizedBind::Default);
    };

    let reject = |reason: String| Err(reject_number(ctx, position, reason));

    match value {
        serde_json::Value::Null => Ok(NormalizedBind::Null(target)),
        serde_json::Value::Number(n) => normalize_number(n, target, ctx, position),
        other => reject(format!(
            "{} value at a numeric parameter; {} mode requires numeric JSON at described numeric positions",
            json_kind_name(other),
            ctx.mode_name()
        )),
    }
}

fn normalize_number(
    n: &serde_json::Number,
    target: BindTarget,
    ctx: BindCtx,
    position: usize,
) -> Result<NormalizedBind, CamelError> {
    // Unsigned magnitudes beyond i64::MAX are detected through the u64
    // value before any lossy f64 conversion.
    if let Some(u) = n.as_u64()
        && n.as_i64().is_none()
    {
        return Err(reject_number(
            ctx,
            position,
            format!(
                "unsigned integer value {u} exceeds the described {} parameter range",
                target.name()
            ),
        ));
    }

    if let Some(i) = n.as_i64() {
        return match target {
            BindTarget::I64 => Ok(NormalizedBind::I64(i)),
            BindTarget::I32 => i32::try_from(i).map(NormalizedBind::I32).map_err(|_| {
                reject_number(
                    ctx,
                    position,
                    format!("integer value {i} does not fit in the described INTEGER parameter"),
                )
            }),
            BindTarget::I16 => i16::try_from(i).map(NormalizedBind::I16).map_err(|_| {
                reject_number(
                    ctx,
                    position,
                    format!("integer value {i} does not fit in the described SMALLINT parameter"),
                )
            }),
            BindTarget::F64 => {
                if i128::from(i).abs() > F64_EXACT_INT_LIMIT {
                    return Err(reject_number(
                        ctx,
                        position,
                        format!(
                            "integer value {i} exceeds 2^53 and would lose precision bound as f64"
                        ),
                    ));
                }
                Ok(NormalizedBind::F64(i as f64))
            }
            BindTarget::F32 => {
                if i128::from(i).abs() > F64_EXACT_INT_LIMIT {
                    return Err(reject_number(
                        ctx,
                        position,
                        format!(
                            "integer value {i} exceeds 2^53 and would lose precision bound as f32"
                        ),
                    ));
                }
                to_f32_checked(i as f64, ctx, position).map(NormalizedBind::F32)
            }
        };
    }

    let Some(f) = n.as_f64() else {
        return Err(reject_number(
            ctx,
            position,
            "number value cannot be represented as f64".to_string(),
        ));
    };

    match target {
        BindTarget::F64 => Ok(NormalizedBind::F64(f)),
        BindTarget::F32 => to_f32_checked(f, ctx, position).map(NormalizedBind::F32),
        BindTarget::I64 => f64_to_int(f, ctx, position, target).map(NormalizedBind::I64),
        BindTarget::I32 | BindTarget::I16 => {
            let rounded = f64_to_int(f, ctx, position, target)?;
            let reason = |v: i64| {
                format!(
                    "float value {f} rounds to {v}, outside the described {} parameter range",
                    target.name()
                )
            };
            if target == BindTarget::I32 {
                i32::try_from(rounded)
                    .map(NormalizedBind::I32)
                    .map_err(|_| reject_number(ctx, position, reason(rounded)))
            } else {
                i16::try_from(rounded)
                    .map(NormalizedBind::I16)
                    .map_err(|_| reject_number(ctx, position, reason(rounded)))
            }
        }
    }
}

/// Rounds with ties-to-even (Postgres `rint` semantics) and range-checks
/// against `i64` before the cast, so the conversion can never saturate.
fn f64_to_int(
    f: f64,
    ctx: BindCtx,
    position: usize,
    target: BindTarget,
) -> Result<i64, CamelError> {
    let rounded = f.round_ties_even();
    if !rounded.is_finite() || rounded < I64_MIN_AS_F64 || rounded >= I64_BOUND_EXCLUSIVE_AS_F64 {
        return Err(reject_number(
            ctx,
            position,
            format!(
                "float value {f} rounds outside the described {} parameter range",
                target.name()
            ),
        ));
    }
    Ok(rounded as i64)
}

/// Converts f64 to f32 with the same overflow/underflow rejection Postgres
/// applies in its `float8` -> `float4` cast: a finite value that would
/// become infinite, or a non-zero value that would become zero, fails.
fn to_f32_checked(f: f64, ctx: BindCtx, position: usize) -> Result<f32, CamelError> {
    let g = f as f32;
    if g.is_infinite() {
        return Err(reject_number(
            ctx,
            position,
            format!("float value {f} overflows the described REAL parameter"),
        ));
    }
    if f != 0.0 && g == 0.0 {
        return Err(reject_number(
            ctx,
            position,
            format!("float value {f} underflows the described REAL parameter"),
        ));
    }
    Ok(g)
}

fn reject_number(ctx: BindCtx, position: usize, reason: String) -> CamelError {
    CamelError::ProcessorError(format!(
        "{}parameter {position}: {reason}",
        ctx.item_prefix()
    ))
}

fn json_kind_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Binds one batch item's values against the position targets.
///
/// `targets[position]` drives normalization; positions without a target
/// bind through the default JSON path. On a normalization failure the
/// returned error names the 0-based batch item index and parameter
/// position.
pub(crate) fn bind_batch_item<'q>(
    query: Query<'q, Any, AnyArguments>,
    values: &'q [serde_json::Value],
    targets: &[Option<BindTarget>],
    item: usize,
) -> Result<Query<'q, Any, AnyArguments>, CamelError> {
    bind_values_for_targets(query, values, targets, BindCtx::BatchItem(item))
}

/// Binds one single-mode statement's values against the position targets.
///
/// Same normalization as `bind_batch_item`; a failure names only the
/// parameter position (`parameter {position}: ...`), since a single
/// statement has no batch item index.
pub(crate) fn bind_single_item<'q>(
    query: Query<'q, Any, AnyArguments>,
    values: &'q [serde_json::Value],
    targets: &[Option<BindTarget>],
) -> Result<Query<'q, Any, AnyArguments>, CamelError> {
    bind_values_for_targets(query, values, targets, BindCtx::Single)
}

/// Shared bind loop: normalizes every value for its position target and
/// binds it under the encoding that target describes.
fn bind_values_for_targets<'q>(
    mut query: Query<'q, Any, AnyArguments>,
    values: &'q [serde_json::Value],
    targets: &[Option<BindTarget>],
    ctx: BindCtx,
) -> Result<Query<'q, Any, AnyArguments>, CamelError> {
    for (position, value) in values.iter().enumerate() {
        query = match normalize_for_target(
            value,
            targets.get(position).copied().flatten(),
            ctx,
            position,
        )? {
            NormalizedBind::I16(v) => query.bind(v),
            NormalizedBind::I32(v) => query.bind(v),
            NormalizedBind::I64(v) => query.bind(v),
            NormalizedBind::F32(v) => query.bind(v),
            NormalizedBind::F64(v) => query.bind(v),
            NormalizedBind::Null(target) => match target {
                BindTarget::I16 => query.bind(Option::<i16>::None),
                BindTarget::I32 => query.bind(Option::<i32>::None),
                BindTarget::I64 => query.bind(Option::<i64>::None),
                BindTarget::F32 => query.bind(Option::<f32>::None),
                BindTarget::F64 => query.bind(Option::<f64>::None),
            },
            NormalizedBind::Default => bind_single_json_value(query, value),
        };
    }
    Ok(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Batch bind-kind normalization (bd rc-fef4z) ──────────────────

    mod batch_kinds {
        use super::*;
        use serde_json::json;

        fn norm_f64(v: serde_json::Value) -> Result<NormalizedBind, CamelError> {
            normalize_for_target(&v, Some(BindTarget::F64), BindCtx::BatchItem(1), 0)
        }

        fn norm_i64(v: serde_json::Value) -> Result<NormalizedBind, CamelError> {
            normalize_for_target(&v, Some(BindTarget::I64), BindCtx::BatchItem(1), 0)
        }

        fn err_text(result: Result<NormalizedBind, CamelError>) -> String {
            result.expect_err("expected typed error").to_string()
        }

        #[test]
        fn target_mapping_is_exact_per_wire_encoding() {
            let cases = [
                (AnyTypeInfoKind::SmallInt, Some(BindTarget::I16)),
                (AnyTypeInfoKind::Integer, Some(BindTarget::I32)),
                (AnyTypeInfoKind::BigInt, Some(BindTarget::I64)),
                (AnyTypeInfoKind::Real, Some(BindTarget::F32)),
                (AnyTypeInfoKind::Double, Some(BindTarget::F64)),
                (AnyTypeInfoKind::Text, None),
                (AnyTypeInfoKind::Bool, None),
                (AnyTypeInfoKind::Blob, None),
                (AnyTypeInfoKind::Null, None),
            ];
            for (kind, expected) in cases {
                assert_eq!(
                    bind_target_from_any_kind(kind),
                    expected,
                    "unexpected mapping for {kind:?}"
                );
            }
        }

        #[test]
        fn scan_lets_float_win_per_position() {
            let batch = [
                [json!(1), json!(2), json!("x")],
                [json!(3.5), json!(4), json!("y")],
            ];
            let rows: Vec<&[serde_json::Value]> = batch.iter().map(|r| r.as_slice()).collect();
            let targets = scan_bind_targets(&rows);
            assert_eq!(
                targets,
                vec![Some(BindTarget::F64), Some(BindTarget::I64), None]
            );
        }

        #[test]
        fn scan_without_numbers_defaults() {
            let batch = [[json!("a"), json!(true)], [json!("b"), json!(false)]];
            let rows: Vec<&[serde_json::Value]> = batch.iter().map(|r| r.as_slice()).collect();
            let targets = scan_bind_targets(&rows);
            assert_eq!(targets, vec![None, None]);
        }

        #[test]
        fn int_to_double_stays_exact_within_2_pow_53() {
            assert_eq!(
                norm_f64(json!(9007199254740992i64)).unwrap(),
                NormalizedBind::F64(9007199254740992.0)
            );
            assert_eq!(norm_f64(json!(20)).unwrap(), NormalizedBind::F64(20.0));
            assert_eq!(norm_f64(json!(-3)).unwrap(), NormalizedBind::F64(-3.0));
        }

        #[test]
        fn int_over_2_pow_53_is_rejected_for_float_targets() {
            let text = err_text(norm_f64(json!(9007199254740993i64)));
            assert!(text.contains("Batch item 1, parameter 0"), "got: {text}");
            assert!(text.contains("2^53"), "got: {text}");
            // i64::MIN exceeds 2^53 in magnitude and must be rejected too.
            let text = err_text(norm_f64(json!(i64::MIN)));
            assert!(text.contains("2^53"), "got: {text}");
        }

        #[test]
        fn int_to_smallint_integer_range_checks() {
            assert_eq!(
                normalize_for_target(
                    &json!(30000),
                    Some(BindTarget::I16),
                    BindCtx::BatchItem(0),
                    1,
                )
                .unwrap(),
                NormalizedBind::I16(30000)
            );
            assert_eq!(
                normalize_for_target(
                    &json!(300000),
                    Some(BindTarget::I32),
                    BindCtx::BatchItem(0),
                    1,
                )
                .unwrap(),
                NormalizedBind::I32(300000)
            );
            let text = err_text(normalize_for_target(
                &json!(300000),
                Some(BindTarget::I16),
                BindCtx::BatchItem(2),
                1,
            ));
            assert!(text.contains("SMALLINT"), "got: {text}");
            assert!(text.contains("Batch item 2, parameter 1"), "got: {text}");
            let text = err_text(normalize_for_target(
                &json!(i64::MAX),
                Some(BindTarget::I32),
                BindCtx::BatchItem(0),
                0,
            ));
            assert!(text.contains("INTEGER"), "got: {text}");
        }

        #[test]
        fn float_to_int_rounds_ties_even_like_postgres_rint() {
            assert_eq!(norm_i64(json!(2.5)).unwrap(), NormalizedBind::I64(2));
            assert_eq!(norm_i64(json!(3.5)).unwrap(), NormalizedBind::I64(4));
            assert_eq!(norm_i64(json!(-2.5)).unwrap(), NormalizedBind::I64(-2));
            assert_eq!(norm_i64(json!(0.5)).unwrap(), NormalizedBind::I64(0));
            // 6.5 ties to the even integer 6, like Postgres `rint`.
            assert_eq!(norm_i64(json!(6.5)).unwrap(), NormalizedBind::I64(6));
            assert_eq!(
                normalize_for_target(&json!(7.5), Some(BindTarget::I16), BindCtx::BatchItem(0), 0)
                    .unwrap(),
                NormalizedBind::I16(8)
            );
        }

        #[test]
        fn float_to_int_out_of_range_is_rejected() {
            let text = err_text(norm_i64(json!(1e30)));
            assert!(text.contains("outside the described BIGINT"), "got: {text}");
            let text = err_text(normalize_for_target(
                &json!(1e10),
                Some(BindTarget::I16),
                BindCtx::BatchItem(0),
                0,
            ));
            assert!(text.contains("SMALLINT"), "got: {text}");
        }

        #[test]
        fn f32_overflow_and_underflow_are_rejected() {
            let text = err_text(normalize_for_target(
                &json!(1e40),
                Some(BindTarget::F32),
                BindCtx::BatchItem(0),
                0,
            ));
            assert!(text.contains("overflows the described REAL"), "got: {text}");
            let text = err_text(normalize_for_target(
                &json!(1e-50),
                Some(BindTarget::F32),
                BindCtx::BatchItem(0),
                0,
            ));
            assert!(
                text.contains("underflows the described REAL"),
                "got: {text}"
            );
            assert_eq!(
                normalize_for_target(&json!(6.5), Some(BindTarget::F32), BindCtx::BatchItem(0), 0)
                    .unwrap(),
                NormalizedBind::F32(6.5)
            );
        }

        #[test]
        fn unsigned_beyond_i64_is_rejected_everywhere() {
            let huge = serde_json::Number::from(u64::MAX);
            for target in [
                BindTarget::I16,
                BindTarget::I32,
                BindTarget::I64,
                BindTarget::F32,
                BindTarget::F64,
            ] {
                let result = normalize_for_target(
                    &serde_json::Value::Number(huge.clone()),
                    Some(target),
                    BindCtx::BatchItem(3),
                    1,
                );
                let text = err_text(result);
                assert!(text.contains("Batch item 3, parameter 1"), "got: {text}");
            }
        }

        #[test]
        fn string_at_numeric_position_is_rejected() {
            let text = err_text(norm_f64(json!("12345678")));
            assert!(text.contains("string value"), "got: {text}");
            assert!(text.contains("numeric"), "got: {text}");
            let text = err_text(norm_i64(json!(true)));
            assert!(text.contains("bool value"), "got: {text}");
        }

        #[test]
        fn null_binds_typed_per_target() {
            assert_eq!(
                normalize_for_target(
                    &json!(null),
                    Some(BindTarget::I32),
                    BindCtx::BatchItem(0),
                    0
                )
                .unwrap(),
                NormalizedBind::Null(BindTarget::I32)
            );
            assert_eq!(
                normalize_for_target(
                    &json!(null),
                    Some(BindTarget::F32),
                    BindCtx::BatchItem(0),
                    0
                )
                .unwrap(),
                NormalizedBind::Null(BindTarget::F32)
            );
        }

        #[test]
        fn no_target_keeps_default_binding() {
            assert_eq!(
                normalize_for_target(&json!("abc"), None, BindCtx::BatchItem(0), 0).unwrap(),
                NormalizedBind::Default
            );
            assert_eq!(
                normalize_for_target(&json!(5), None, BindCtx::BatchItem(0), 0).unwrap(),
                NormalizedBind::Default
            );
        }

        #[test]
        fn bind_batch_item_uses_targets_per_position() {
            let values = vec![json!(20), json!(6.5), json!("x")];
            let targets = [Some(BindTarget::F64), Some(BindTarget::I64), None];
            let query = sqlx::query(sqlx::AssertSqlSafe("SELECT 1".to_string()));
            let bound = bind_batch_item(query, &values, &targets, 0);
            assert!(
                bound.is_ok(),
                "expected all values to bind, got {:?}",
                bound.err()
            );
        }
    }

    // ── Single-mode bind planning (bd rc-mn139) ──────────────────────

    fn processor_message(err: &CamelError) -> &str {
        match err {
            CamelError::ProcessorError(message) => message,
            other => panic!("expected ProcessorError, got {other:?}"),
        }
    }

    mod bind_ctx_wording {
        use super::*;
        use serde_json::json;

        #[test]
        fn batch_wording_is_byte_exact() {
            let err = normalize_for_target(
                &json!(9007199254740993i64),
                Some(BindTarget::F64),
                BindCtx::BatchItem(1),
                0,
            )
            .expect_err("integer above 2^53 must be rejected");
            assert_eq!(
                processor_message(&err),
                "Batch item 1, parameter 0: integer value 9007199254740993 exceeds 2^53 and would lose precision bound as f64"
            );

            let err =
                normalize_for_target(&json!("x"), Some(BindTarget::F64), BindCtx::BatchItem(2), 3)
                    .expect_err("text at a numeric parameter must be rejected");
            assert_eq!(
                processor_message(&err),
                "Batch item 2, parameter 3: string value at a numeric parameter; batch mode requires numeric JSON at described numeric positions"
            );
        }

        #[test]
        fn single_wording_names_only_the_position() {
            let err = normalize_for_target(
                &json!(9007199254740993i64),
                Some(BindTarget::F64),
                BindCtx::Single,
                0,
            )
            .expect_err("integer above 2^53 must be rejected");
            assert_eq!(
                processor_message(&err),
                "parameter 0: integer value 9007199254740993 exceeds 2^53 and would lose precision bound as f64"
            );

            let err = normalize_for_target(&json!("x"), Some(BindTarget::F64), BindCtx::Single, 3)
                .expect_err("text at a numeric parameter must be rejected");
            assert_eq!(
                processor_message(&err),
                "parameter 3: string value at a numeric parameter; single mode requires numeric JSON at described numeric positions"
            );
        }
    }

    mod described_eligibility {
        use super::*;
        use serde_json::json;

        #[test]
        fn eligibility_matrix_is_closed() {
            // Non-numeric described positions accept only a matching wire
            // family (or NULL); numeric positions always map to a target.
            let cases: &[(AnyTypeInfoKind, serde_json::Value, bool)] = &[
                (AnyTypeInfoKind::Text, json!(1), false),
                (AnyTypeInfoKind::Text, json!("x"), true),
                (AnyTypeInfoKind::Text, json!(true), false),
                (AnyTypeInfoKind::Bool, json!(true), true),
                (AnyTypeInfoKind::Bool, json!("x"), false),
                (AnyTypeInfoKind::Bool, json!(1), false),
                (AnyTypeInfoKind::Blob, json!("x"), true),
                (AnyTypeInfoKind::Blob, json!(1), false),
                (AnyTypeInfoKind::Blob, json!(true), false),
                (AnyTypeInfoKind::Double, json!(1), true),
                (AnyTypeInfoKind::Double, json!("x"), true),
                (AnyTypeInfoKind::Text, json!(null), true),
                (AnyTypeInfoKind::Bool, json!(null), true),
                (AnyTypeInfoKind::Blob, json!(null), true),
                (AnyTypeInfoKind::SmallInt, json!(null), true),
                (AnyTypeInfoKind::Null, json!(null), true),
                (AnyTypeInfoKind::Null, json!("x"), false),
            ];
            for (kind, payload, expected) in cases {
                assert_eq!(
                    described_mode_eligible(&[*kind], std::slice::from_ref(payload)),
                    *expected,
                    "eligibility for {kind:?} with payload {payload}"
                );
            }
        }

        #[test]
        fn missing_described_kind_is_ineligible() {
            assert!(!described_mode_eligible(
                &[AnyTypeInfoKind::Text],
                &[json!("x"), json!("y")]
            ));
        }
    }

    mod hybrid_targets {
        use super::*;
        use serde_json::json;

        #[test]
        fn described_numeric_targets_survive_the_fallback() {
            assert_eq!(
                hybrid_fallback_targets(&[Some(BindTarget::F64), None], &[json!(1), json!(2)]),
                vec![Some(BindTarget::F64), Some(BindTarget::I64)]
            );
            assert_eq!(hybrid_fallback_targets(&[None], &[json!("x")]), vec![None]);
            assert_eq!(
                hybrid_fallback_targets(&[None], &[json!(6.5)]),
                vec![Some(BindTarget::F64)]
            );
        }

        #[test]
        fn one_row_scan_lets_float_win() {
            assert_eq!(
                scan_bind_targets(&[&[json!(6.5)]]),
                vec![Some(BindTarget::F64)]
            );
        }
    }
}
