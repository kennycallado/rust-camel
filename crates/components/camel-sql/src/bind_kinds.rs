//! Bind-kind normalization for SQL bindings.
//!
//! Owns the bind domain: the default JSON binder, the per-position wire
//! encodings (`BindTarget`), the single- and batch-mode normalization loops,
//! and the described-mode eligibility matrix that decides how one statement
//! is bound. Bind-kind safety rationale lives with the batch execution code
//! in `producer`.

use camel_component_api::CamelError;
use sqlx::Any;
use sqlx::AnyConnection;
use sqlx::Either;
use sqlx::Executor;
use sqlx::SqlSafeStr;
use sqlx::any::AnyArguments;
use sqlx::any::AnyTypeInfoKind;
use sqlx::query::Query;
use tracing::warn;

/// Binds JSON values to a sqlx query.
///
/// Handles: Null, Bool, Number (i64/f64), String, Array/Object (serialized to JSON string).
///
/// Test-only: production sites go through the bind planner (`plan_binds` +
/// `bind_single_item`/`bind_batch_item`), which keeps the raw binder behind
/// this gate so the compiler rejects any new production caller.
#[cfg(test)]
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

// ── Bind planner (bd rc-7e681, rc-0eiqm) ───────────────────────────────

/// Which writer owns the statement. The mode selects the probe and fallback
/// alias namespaces only (`/* {mode}-bind-kinds-probe */`,
/// `/* {mode}-bind-kinds-fallback */`); it never changes the bind policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindMode {
    Single,
    Consumer,
    Batch,
}

impl BindMode {
    /// Suffix shared by this mode's probe and fallback aliases.
    fn alias_suffix(self) -> &'static str {
        match self {
            BindMode::Single => "single",
            BindMode::Consumer => "consumer",
            BindMode::Batch => "batch",
        }
    }

    /// Log subject for the describe stages. The consumer sites reuse the
    /// single wording: they bound raw JSON before the planner migration.
    fn log_subject(self) -> &'static str {
        match self {
            BindMode::Single | BindMode::Consumer => "Single",
            BindMode::Batch => "Batch",
        }
    }

    /// Error wrapper prefix for a failed prepare on this mode's text.
    fn prepare_error_prefix(self) -> &'static str {
        match self {
            BindMode::Single | BindMode::Consumer => "SQL query prepare failed",
            BindMode::Batch => "Batch query prepare failed",
        }
    }
}

/// One statement's planned binds: the SQL text to execute, whether the
/// statement may stay in the pool's persistent statement cache, and the
/// per-position wire encodings.
pub(crate) struct BindPlan {
    pub sql: String,
    pub persistent: bool,
    pub targets: Vec<Option<BindTarget>>,
}

/// Outcome of a describe probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProbeOutcome {
    /// The server described the statement with these parameter kinds.
    Described(Vec<AnyTypeInfoKind>),
    /// No usable description: driver metadata error (kinds outside the
    /// closed set, e.g. NUMERIC) or an empty parameter type list.
    Unusable,
}

/// Probes a statement's parameter kinds on this mode's probe alias.
///
/// Prepares with empty parameter types so the server infers the parameter
/// OIDs from the SQL text. The probe text is never bound against: when a
/// type is unmapped the probe seeds a cache entry no payload can satisfy,
/// and keeping it on its own text leaves the user's SQL namespace clean.
async fn probe_kinds(
    conn: &mut AnyConnection,
    sql: &str,
    mode: BindMode,
) -> Result<ProbeOutcome, CamelError> {
    let probe_sql = format!("{sql} /* {}-bind-kinds-probe */", mode.alias_suffix());
    match Executor::prepare_with(
        &mut *conn,
        sqlx::AssertSqlSafe(probe_sql).into_sql_str(),
        &[],
    )
    .await
    {
        Ok(statement) => match statement.parameters {
            // An empty type list cannot drive normalization.
            Some(Either::Left(types)) if !types.is_empty() => Ok(ProbeOutcome::Described(
                types.iter().map(|type_info| type_info.kind()).collect(),
            )),
            _ => Ok(ProbeOutcome::Unusable),
        },
        // Metadata unavailable (e.g. NUMERIC columns have no Any mapping).
        // Server and connection failures propagate unchanged.
        Err(sqlx::Error::AnyDriverError(_)) => Ok(ProbeOutcome::Unusable),
        Err(e) => {
            // log-policy: handler-owned
            // (category a: bind probe failure inside the bind planner)
            warn!("{} describe probe failed: {}", mode.log_subject(), e);
            Err(CamelError::ProcessorError(format!(
                "{}: {e}",
                mode.prepare_error_prefix()
            )))
        }
    }
}

/// Prepares the real SQL text with empty parameter types and reads back the
/// parameter kinds of the statement that actually executes.
///
/// On a cache hit the read-back kinds are the cached entry's actual OIDs —
/// which is why the planner compares them against the probe: a disagreement
/// means the entry was seeded by bind-derived OIDs (bd rc-0eiqm).
///
/// `batch_index` names the offending batch item in the prepare-failure log
/// (`Batch describe failed at index {index}: {e}`); the single-statement
/// callers pass `None` and keep `Single describe failed: {e}`.
pub(crate) async fn readback_kinds(
    conn: &mut AnyConnection,
    sql: &str,
    mode: BindMode,
    batch_index: Option<usize>,
) -> Result<Option<Vec<AnyTypeInfoKind>>, CamelError> {
    let statement = Executor::prepare_with(
        &mut *conn,
        sqlx::AssertSqlSafe(sql.to_string()).into_sql_str(),
        &[],
    )
    .await
    .map_err(|e| {
        // log-policy: handler-owned
        // (category a: described prepare failure inside the bind planner)
        match batch_index {
            Some(index) => warn!(
                "{} describe failed at index {}: {}",
                mode.log_subject(),
                index,
                e
            ),
            None => warn!("{} describe failed: {}", mode.log_subject(), e),
        }
        CamelError::ProcessorError(format!("{}: {e}", mode.prepare_error_prefix()))
    })?;

    Ok(match statement.parameters {
        Some(Either::Left(types)) if !types.is_empty() => {
            Some(types.iter().map(|type_info| type_info.kind()).collect())
        }
        _ => None,
    })
}

/// Alias targets from a shape's described targets and the message-wide scan.
///
/// Every described numeric target is kept, so the checked conversions still
/// fire at positions the server described. Only positions without one
/// resolve from `message_scan`, which is always the scan over ALL binding
/// rows — never a per-shape scan: a per-shape scan would loosen the typed
/// 2^53 guard into a silent server-side round.
pub(crate) fn hybrid_fallback_targets_rows(
    described: &[Option<BindTarget>],
    message_scan: &[Option<BindTarget>],
) -> Vec<Option<BindTarget>> {
    described
        .iter()
        .enumerate()
        .map(|(position, target)| target.or_else(|| message_scan.get(position).copied().flatten()))
        .collect()
}

/// Whether the named statement's read-back kinds agree with the probe.
///
/// Compared on the RAW parameter kinds, position-wise: comparing mapped
/// targets would hide a TEXT-versus-BOOL disagreement (both map to no
/// target), which is exactly the poisoned-entry shape this check exists to
/// catch. An unusable read-back never agrees.
pub(crate) fn named_kinds_agree(
    probe: &[AnyTypeInfoKind],
    readback: Option<&[AnyTypeInfoKind]>,
) -> bool {
    readback.is_some_and(|readback| readback == probe)
}

/// Which bind path a planned statement takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BindPath {
    /// The real SQL text stays persistent; the read-back supplies targets.
    Named,
    /// The fallback alias text runs unnamed (`persistent(false)`).
    Alias,
}

/// Named only when the probe described the statement, every row passes the
/// eligibility matrix, and the named read-back agrees with the probe.
///
/// On `Alias`, targets are the probe-derived hybrid targets when the probe
/// is `Described`, or the message-wide scan when the probe is `Unusable`.
pub(crate) fn decide_path(
    probe: &ProbeOutcome,
    rows_eligible: bool,
    readback_agrees: bool,
) -> BindPath {
    if matches!(probe, ProbeOutcome::Described(_)) && rows_eligible && readback_agrees {
        BindPath::Named
    } else {
        BindPath::Alias
    }
}

/// Plans one statement's binds on the connection that will execute it.
///
/// All-empty bindings keep the default persistent path with no probe round
/// trip. Otherwise the probe decides the mode: the named path is taken only
/// when every row passes the eligibility matrix and the read-back agrees
/// with the probe; both alias paths keep the probe's described numeric
/// targets (hybrid) so the checked conversions still fire, and a statement
/// with no usable description keeps the legacy message-wide scan.
pub(crate) async fn plan_binds(
    conn: &mut AnyConnection,
    sql: &str,
    rows: &[&[serde_json::Value]],
    mode: BindMode,
) -> Result<BindPlan, CamelError> {
    // Nothing to reinterpret: keep the default persistent path, with no
    // probe round trip.
    if rows.iter().all(|row| row.is_empty()) {
        return Ok(BindPlan {
            sql: sql.to_string(),
            persistent: true,
            targets: vec![],
        });
    }

    let probe = probe_kinds(&mut *conn, sql, mode).await?;
    let message_scan = scan_bind_targets(rows);
    let fallback_sql = format!("{sql} /* {}-bind-kinds-fallback */", mode.alias_suffix());

    let ProbeOutcome::Described(kinds) = &probe else {
        // Metadata unavailable: legacy scan on the alias, no described
        // targets (an unusable probe never supplies guards).
        return Ok(BindPlan {
            sql: fallback_sql,
            persistent: false,
            targets: message_scan,
        });
    };

    let described_targets = kinds
        .iter()
        .map(|kind| bind_target_from_any_kind(*kind))
        .collect::<Vec<_>>();
    let rows_eligible = rows.iter().all(|row| described_mode_eligible(kinds, row));

    // An ineligible row goes straight to the alias, like the single path
    // before the planner: no named prepare is issued for a statement that
    // will not execute on its named text.
    let readback = if rows_eligible {
        readback_kinds(&mut *conn, sql, mode, None).await?
    } else {
        None
    };
    let readback_agrees = named_kinds_agree(kinds, readback.as_deref());

    match decide_path(&probe, rows_eligible, readback_agrees) {
        BindPath::Named => {
            let Some(readback) = readback else {
                // Unreachable by construction: `decide_path` selects `Named`
                // only when the read-back agreed with the probe, and an
                // unusable read-back never agrees. If the invariant ever
                // breaks, take the safe direction — the alias text with
                // own-OID binds — never probe targets on the named text.
                debug_assert!(false, "Named path selected without an agreeing read-back");
                return Ok(BindPlan {
                    sql: fallback_sql,
                    persistent: false,
                    targets: hybrid_fallback_targets_rows(&described_targets, &message_scan),
                });
            };
            let targets = readback
                .iter()
                .map(|kind| bind_target_from_any_kind(*kind))
                .collect();
            Ok(BindPlan {
                sql: sql.to_string(),
                persistent: true,
                targets,
            })
        }
        BindPath::Alias => Ok(BindPlan {
            sql: fallback_sql,
            persistent: false,
            targets: hybrid_fallback_targets_rows(&described_targets, &message_scan),
        }),
    }
}

/// One distinct resolved SQL shape and the item indices that use it.
pub(crate) struct ShapeGroup {
    pub sql: String,
    pub rows: Vec<usize>,
}

/// Groups item SQL texts into shapes, in first-appearance order.
///
/// IN-clause expansion rewrites `in (:list)` into one placeholder per
/// element, so items of one batch can resolve to different SQL texts and
/// every later parameter position shifts with the list length.
pub(crate) fn group_rows_by_sql(sqls: &[String]) -> Vec<ShapeGroup> {
    let mut groups: Vec<ShapeGroup> = Vec::new();
    for (index, sql) in sqls.iter().enumerate() {
        match groups.iter_mut().find(|group| &group.sql == sql) {
            Some(group) => group.rows.push(index),
            None => groups.push(ShapeGroup {
                sql: sql.clone(),
                rows: vec![index],
            }),
        }
    }
    groups
}

/// Probes every distinct shape once on its own batch probe alias.
pub(crate) async fn probe_batch_shapes(
    conn: &mut AnyConnection,
    groups: &[ShapeGroup],
) -> Result<Vec<ProbeOutcome>, CamelError> {
    let mut probes = Vec::with_capacity(groups.len());
    for group in groups {
        probes.push(probe_kinds(&mut *conn, &group.sql, BindMode::Batch).await?);
    }
    Ok(probes)
}

/// How a batch item binds once the whole batch is decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ItemBindSource {
    /// Named prepare plus read-back: the item's shape probe was described
    /// and every row of that shape passes the eligibility matrix.
    NamedReadBack,
    /// Alias execution on the precomputed `alias_targets`.
    AliasPlan,
}

/// One batch item's precomputed plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BatchItemPlan {
    pub source: ItemBindSource,
    pub alias_targets: Vec<Option<BindTarget>>,
}

/// Plans every batch item from its shape's probe and the message-wide scan.
///
/// The FIRST probe in `probes` selects the batch mode (today's semantics:
/// the batch loop probed `prepared_items.first()`). An unusable first probe
/// keeps the legacy message-wide scan for the whole batch — scan guards, no
/// described guards. Otherwise each item is matrix-checked against its own
/// shape's kinds: an eligible item stays `NamedReadBack`, while an
/// ineligible row or a later unusable shape becomes `AliasPlan`.
///
/// `alias_targets` is precomputed for every item so the whole-batch rule
/// (one `AliasPlan` item moves every item to its precomputed alias targets)
/// executes exactly what production planned. An ineligible row on a usable
/// probe keeps its shape's described numeric targets — mirroring the
/// single-path precedence — while a later unusable shape contributes the
/// message-wide scan alone, so neither order can erase an earlier shape's
/// described guard.
pub(crate) fn plan_batch_items(
    probes: &[(String, ProbeOutcome)],
    items: &[(String, Vec<serde_json::Value>)],
    message_scan: &[Option<BindTarget>],
) -> Vec<BatchItemPlan> {
    let scan_only = || BatchItemPlan {
        source: ItemBindSource::AliasPlan,
        alias_targets: message_scan.to_vec(),
    };

    let legacy_scan = probes
        .first()
        .is_none_or(|(_, probe)| matches!(probe, ProbeOutcome::Unusable));
    if legacy_scan {
        return items.iter().map(|_| scan_only()).collect();
    }

    items
        .iter()
        .map(|(sql, bindings)| {
            let Some((_, probe)) = probes.iter().find(|(shape, _)| shape == sql) else {
                return scan_only();
            };
            let ProbeOutcome::Described(kinds) = probe else {
                return scan_only();
            };
            let described_targets = kinds
                .iter()
                .map(|kind| bind_target_from_any_kind(*kind))
                .collect::<Vec<_>>();
            let alias_targets = hybrid_fallback_targets_rows(&described_targets, message_scan);
            let source = if described_mode_eligible(kinds, bindings) {
                ItemBindSource::NamedReadBack
            } else {
                ItemBindSource::AliasPlan
            };
            BatchItemPlan {
                source,
                alias_targets,
            }
        })
        .collect()
}

/// The FINAL bind decision for one whole batch (bd rc-7e681, rc-0eiqm).
///
/// The decision covers every item before item 0 executes, so a mismatch
/// discovered on a later shape still sends the whole batch to the aliases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BatchPlan {
    /// The whole batch runs on its uncached alias texts
    /// (`/* batch-bind-kinds-fallback */`, `persistent(false)`), each item
    /// with its own precomputed targets: the shape's described numeric
    /// targets where the shape had a usable description, the message-wide
    /// scan otherwise. The per-item distinction is the no-loosening rule —
    /// one shared scan would turn a typed 2^53 rejection into a silent
    /// server-side round.
    Aliased {
        /// Alias targets per item, indexed like the batch items.
        targets: Vec<Vec<Option<BindTarget>>>,
    },
    /// The whole batch runs on its named texts, each item with the targets
    /// its read-back agreed on.
    Named {
        /// Read-back-agreed targets per item, indexed like the batch items.
        per_item_targets: Vec<Vec<Option<BindTarget>>>,
    },
}

/// The named targets for one batch item, or `None` when the item may not run
/// named.
///
/// `None` means the item's shape was not described, the read-back was
/// missing/unusable, or the read-back kinds disagree with the probe kinds —
/// the caller then flips the WHOLE batch to the aliases (the safe direction:
/// never probe-derived bytes on the named statement).
fn named_targets_for_item(
    shape_probes: &[(String, ProbeOutcome)],
    sql: &str,
    readback: Option<&[AnyTypeInfoKind]>,
) -> Option<Vec<Option<BindTarget>>> {
    let Some((_, ProbeOutcome::Described(kinds))) =
        shape_probes.iter().find(|(shape, _)| shape == sql)
    else {
        return None;
    };
    let readback = readback?;
    if !named_kinds_agree(kinds, Some(readback)) {
        return None;
    }
    Some(
        readback
            .iter()
            .map(|kind| bind_target_from_any_kind(*kind))
            .collect(),
    )
}

/// The final batch decision from the per-item plans and the read-back kinds
/// observed in item order.
///
/// `readbacks` is short-circuit-truncated: a batch that already failed on the
/// last observed item stops issuing read-backs, so a missing entry is a
/// failure, never an `Ok`.
fn decide_batch_plan(
    plans: &[BatchItemPlan],
    shape_probes: &[(String, ProbeOutcome)],
    items: &[(String, Vec<serde_json::Value>)],
    readbacks: &[Option<Vec<AnyTypeInfoKind>>],
) -> BatchPlan {
    let alias_plan = || BatchPlan::Aliased {
        targets: plans
            .iter()
            .map(|plan| plan.alias_targets.clone())
            .collect(),
    };

    // Whole-batch rule: one `AliasPlan` item (an ineligible row or a shape
    // with no usable description) sends every item to its precomputed alias
    // targets.
    if plans
        .iter()
        .any(|plan| plan.source == ItemBindSource::AliasPlan)
    {
        return alias_plan();
    }

    let mut per_item_targets = Vec::with_capacity(items.len());
    for (batch_idx, (sql, _)) in items.iter().enumerate() {
        let Some(targets) = readbacks
            .get(batch_idx)
            .and_then(|readback| named_targets_for_item(shape_probes, sql, readback.as_deref()))
        else {
            return alias_plan();
        };
        per_item_targets.push(targets);
    }

    BatchPlan::Named { per_item_targets }
}

/// Plans every batch item's binds on the batch's in-flight transaction and
/// returns the FINAL whole-batch decision.
///
/// `items` carries each item's OWN resolved SQL text plus bindings: IN-clause
/// expansion rewrites `in (:list)` into one placeholder per element, so one
/// batch can resolve to distinct texts and every later parameter position
/// shifts with the list length. Metadata is therefore tracked per distinct
/// shape, never per batch: the texts are grouped by shape, every shape is
/// probed once, and the no-loosening bind scan runs over ALL binding rows.
///
/// The named read-back agreement loop follows: per item in order, the
/// read-back kinds on this same transaction must agree with the item shape's
/// probe kinds. The FIRST missing, unusable or disagreeing read-back flips
/// the WHOLE batch to the aliases before item 0 executes.
pub(crate) async fn plan_batch_binds(
    tx: &mut AnyConnection,
    items: &[(String, Vec<serde_json::Value>)],
) -> Result<BatchPlan, CamelError> {
    let sqls: Vec<String> = items.iter().map(|(sql, _)| sql.clone()).collect();
    let groups = group_rows_by_sql(&sqls);
    let binding_rows: Vec<&[serde_json::Value]> = items
        .iter()
        .map(|(_, bindings)| bindings.as_slice())
        .collect();
    // Scan fallback targets: one bind kind per position across the WHOLE
    // batch, float wins. The message-wide scan (never a per-shape scan) is
    // the no-loosening rule: a per-shape scan could turn a typed 2^53
    // rejection into a silent server-side round.
    let message_scan = scan_bind_targets(&binding_rows);

    // Probe every distinct shape once on the caller's in-flight batch
    // transaction. Each probe runs on its own alias text: preparing with
    // empty parameter types seeds a statement-cache entry under the probed
    // text, and for unmapped types (e.g. NUMERIC) that entry is one no
    // payload can satisfy.
    let probes = probe_batch_shapes(&mut *tx, &groups).await?;
    let shape_probes: Vec<(String, ProbeOutcome)> = groups
        .iter()
        .map(|group| group.sql.clone())
        .zip(probes)
        .collect();
    let plans = plan_batch_items(&shape_probes, items, &message_scan);

    // An aliased batch issues no named prepares at all: the whole-batch
    // decision already holds, so there is nothing to read back.
    if plans
        .iter()
        .any(|plan| plan.source == ItemBindSource::AliasPlan)
    {
        return Ok(decide_batch_plan(&plans, &shape_probes, items, &[]));
    }

    // Named read-back agreement, short-circuiting on the first item that
    // cannot agree: once the batch flips to the aliases no further read-back
    // is useful, so none is issued.
    let mut readbacks = Vec::with_capacity(items.len());
    for (batch_idx, (sql, _)) in items.iter().enumerate() {
        let readback = readback_kinds(&mut *tx, sql, BindMode::Batch, Some(batch_idx)).await?;
        let agrees = named_targets_for_item(&shape_probes, sql, readback.as_deref()).is_some();
        readbacks.push(readback);
        if !agrees {
            break;
        }
    }

    Ok(decide_batch_plan(&plans, &shape_probes, items, &readbacks))
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
        fn every_matrix_cell_is_decided() {
            use AnyTypeInfoKind::{
                BigInt, Blob, Bool, Double, Integer, Null, Real, SmallInt, Text,
            };

            // spec.md closed matrix: numeric kinds accept every payload
            // (typed errors fire at bind time, not at eligibility time);
            // BOOL only Bool and Null; TEXT/BLOB only Text and Null; an
            // unmapped Null kind only Null. No pair falls through to an
            // implicit default.
            let cases: &[(AnyTypeInfoKind, serde_json::Value, bool)] = &[
                (SmallInt, json!(true), true),
                (SmallInt, json!(1), true),
                (SmallInt, json!("x"), true),
                (SmallInt, json!(null), true),
                (Integer, json!(true), true),
                (Integer, json!(1), true),
                (Integer, json!("x"), true),
                (Integer, json!(null), true),
                (BigInt, json!(true), true),
                (BigInt, json!(1), true),
                (BigInt, json!("x"), true),
                (BigInt, json!(null), true),
                (Real, json!(true), true),
                (Real, json!(1), true),
                (Real, json!("x"), true),
                (Real, json!(null), true),
                (Double, json!(true), true),
                (Double, json!(1), true),
                (Double, json!("x"), true),
                (Double, json!(null), true),
                (Bool, json!(true), true),
                (Bool, json!(1), false),
                (Bool, json!("x"), false),
                (Bool, json!(null), true),
                (Text, json!(true), false),
                (Text, json!(1), false),
                (Text, json!("x"), true),
                (Text, json!(null), true),
                (Blob, json!(true), false),
                (Blob, json!(1), false),
                (Blob, json!("x"), true),
                (Blob, json!(null), true),
                (Null, json!(true), false),
                (Null, json!(1), false),
                (Null, json!("x"), false),
                (Null, json!(null), true),
            ];
            for (kind, payload, expected) in cases {
                assert_eq!(
                    described_mode_eligible(&[*kind], std::slice::from_ref(payload)),
                    *expected,
                    "eligibility for {kind:?} with payload {payload}"
                );
            }

            // The pinned attack cells: a bool or a number at a described
            // TEXT/BLOB position takes the hybrid alias, and a number or
            // text at a described BOOL position does the same.
            let attacks = [
                (Text, json!(3544952156018063160i64)),
                (Text, json!(true)),
                (Blob, json!(3544952156018063160i64)),
                (Blob, json!(true)),
                (Bool, json!(3544952156018063160i64)),
                (Bool, json!("x")),
            ];
            for (kind, payload) in attacks {
                assert!(
                    !described_mode_eligible(&[kind], std::slice::from_ref(&payload)),
                    "attack cell {kind:?} with payload {payload} must be ineligible"
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
            let scan = |bindings: &[serde_json::Value]| scan_bind_targets(&[bindings]);
            assert_eq!(
                hybrid_fallback_targets_rows(
                    &[Some(BindTarget::F64), None],
                    &scan(&[json!(1), json!(2)]),
                ),
                vec![Some(BindTarget::F64), Some(BindTarget::I64)]
            );
            assert_eq!(
                hybrid_fallback_targets_rows(&[None], &scan(&[json!("x")])),
                vec![None]
            );
            assert_eq!(
                hybrid_fallback_targets_rows(&[None], &scan(&[json!(6.5)])),
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

        #[test]
        fn hybrid_rows_keep_described_numeric_targets() {
            assert_eq!(
                hybrid_fallback_targets_rows(
                    &[Some(BindTarget::F64), None],
                    &[None, Some(BindTarget::F64)],
                ),
                vec![Some(BindTarget::F64), Some(BindTarget::F64)]
            );
        }
    }

    // ── Bind planner path decision (bd rc-7e681, rc-0eiqm) ───────────

    mod planner_path {
        use super::*;

        #[test]
        fn named_kinds_agree_requires_raw_equality() {
            // Mapped comparison would hide TEXT-versus-BOOL (both map to no
            // target) — the poisoned-entry shape this check exists to catch.
            assert!(!named_kinds_agree(
                &[AnyTypeInfoKind::Double, AnyTypeInfoKind::Text],
                Some(&[AnyTypeInfoKind::Double, AnyTypeInfoKind::Bool]),
            ));
            assert!(!named_kinds_agree(
                &[AnyTypeInfoKind::Double, AnyTypeInfoKind::Integer],
                Some(&[AnyTypeInfoKind::BigInt, AnyTypeInfoKind::Integer]),
            ));
            assert!(named_kinds_agree(
                &[AnyTypeInfoKind::Double, AnyTypeInfoKind::Integer],
                Some(&[AnyTypeInfoKind::Double, AnyTypeInfoKind::Integer]),
            ));
            assert!(!named_kinds_agree(&[AnyTypeInfoKind::Double], None));
        }

        #[test]
        fn decide_path_aliases_on_disagreement() {
            let described = ProbeOutcome::Described(vec![AnyTypeInfoKind::Double]);
            assert_eq!(decide_path(&described, true, false), BindPath::Alias);
            assert_eq!(decide_path(&described, true, true), BindPath::Named);
            assert_eq!(decide_path(&described, false, true), BindPath::Alias);
            assert_eq!(
                decide_path(&ProbeOutcome::Unusable, true, true),
                BindPath::Alias
            );
        }
    }

    // ── Batch shape planner (bd rc-7e681, rc-0eiqm) ──────────────────

    mod batch_planner {
        use super::*;
        use serde_json::json;

        fn probe_entries(entries: Vec<(&str, ProbeOutcome)>) -> Vec<(String, ProbeOutcome)> {
            entries
                .into_iter()
                .map(|(sql, probe)| (sql.to_string(), probe))
                .collect()
        }

        fn item_entries(
            entries: Vec<(&str, Vec<serde_json::Value>)>,
        ) -> Vec<(String, Vec<serde_json::Value>)> {
            entries
                .into_iter()
                .map(|(sql, values)| (sql.to_string(), values))
                .collect()
        }

        fn scan_only(message_scan: &[Option<BindTarget>]) -> BatchItemPlan {
            BatchItemPlan {
                source: ItemBindSource::AliasPlan,
                alias_targets: message_scan.to_vec(),
            }
        }

        #[test]
        fn group_rows_by_sql_preserves_first_appearance() {
            let sqls = ["A".to_string(), "B".to_string(), "A".to_string()];
            let groups = group_rows_by_sql(&sqls);
            assert_eq!(groups.len(), 2);
            assert_eq!(groups[0].sql, "A");
            assert_eq!(groups[0].rows, vec![0, 2]);
            assert_eq!(groups[1].sql, "B");
            assert_eq!(groups[1].rows, vec![1]);
        }

        #[test]
        fn message_wide_scan_regression_two_shapes_share_float_guard() {
            // Two shapes with no usable description: one carries a float at
            // position 0, the other an integer above 2^53 at the same index.
            let row_a = [json!(1.5)];
            let row_b = [json!(9007199254740993i64)];
            let rows: Vec<&[serde_json::Value]> = vec![row_a.as_slice(), row_b.as_slice()];
            let message_scan = scan_bind_targets(&rows);
            assert_eq!(message_scan, vec![Some(BindTarget::F64)]);

            let probes = probe_entries(vec![
                ("A", ProbeOutcome::Unusable),
                ("B", ProbeOutcome::Unusable),
            ]);
            let items = item_entries(vec![
                ("A", vec![json!(1.5)]),
                ("B", vec![json!(9007199254740993i64)]),
            ]);
            let plans = plan_batch_items(&probes, &items, &message_scan);
            assert_eq!(plans.len(), 2);
            // A per-shape scan would have given shape B `[Some(I64)]` and
            // rounded silently; the message-wide guard is F64 for both.
            for plan in &plans {
                assert_eq!(plan.source, ItemBindSource::AliasPlan);
                assert_eq!(plan.alias_targets, vec![Some(BindTarget::F64)]);
            }

            let err = normalize_for_target(
                &json!(9007199254740993i64),
                Some(BindTarget::F64),
                BindCtx::BatchItem(1),
                0,
            )
            .expect_err("the message-wide F64 guard must reject 2^53 + 1");
            assert!(processor_message(&err).contains("2^53"), "got: {err}");
        }

        #[test]
        fn plan_batch_items_mode_only() {
            let probes = probe_entries(vec![
                (
                    "A",
                    ProbeOutcome::Described(vec![
                        AnyTypeInfoKind::Double,
                        AnyTypeInfoKind::Integer,
                    ]),
                ),
                ("B", ProbeOutcome::Described(vec![AnyTypeInfoKind::Text])),
            ]);
            let items = item_entries(vec![
                ("A", vec![json!(2.5), json!(1)]),
                ("B", vec![json!(123)]),
            ]);
            let plans = plan_batch_items(&probes, &items, &[None, None]);
            assert_eq!(plans.len(), 2);
            assert_eq!(
                plans[0].source,
                ItemBindSource::NamedReadBack,
                "a described eligible shape stays on the named read-back path"
            );
            assert_eq!(
                plans[1].source,
                ItemBindSource::AliasPlan,
                "a number at a described TEXT position is ineligible"
            );
        }

        #[test]
        fn batch_plan_keeps_described_double_on_alias() {
            // Doubly dangerous: the shape's described DOUBLE position holds
            // an integer above 2^53, and the shape is ineligible anyway
            // (a number at its described TEXT position). The fallback to the
            // alias must keep the described F64 target so the checked
            // conversion fires; the message-wide scan supplies TEXT's
            // position only because the probe maps it to no target.
            let probes = probe_entries(vec![(
                "S",
                ProbeOutcome::Described(vec![AnyTypeInfoKind::Double, AnyTypeInfoKind::Text]),
            )]);
            let items = item_entries(vec![("S", vec![json!(9007199254740993i64), json!(123)])]);
            let rows: Vec<&[serde_json::Value]> =
                items.iter().map(|(_, values)| values.as_slice()).collect();
            let message_scan = scan_bind_targets(&rows);
            assert_eq!(
                message_scan,
                vec![Some(BindTarget::I64), Some(BindTarget::I64)]
            );

            let plans = plan_batch_items(&probes, &items, &message_scan);
            assert_eq!(
                plans,
                vec![BatchItemPlan {
                    source: ItemBindSource::AliasPlan,
                    alias_targets: vec![Some(BindTarget::F64), Some(BindTarget::I64)],
                }],
                "the described DOUBLE survives the alias fallback"
            );

            let err = match bind_batch_item(
                sqlx::query("SELECT ?, ?"),
                &items[0].1,
                &plans[0].alias_targets,
                0,
            ) {
                Ok(_) => panic!("the kept DOUBLE target must reject 2^53 + 1 on the alias"),
                Err(err) => err,
            };
            assert_eq!(
                processor_message(&err),
                "Batch item 0, parameter 0: integer value 9007199254740993 exceeds 2^53 and would lose precision bound as f64"
            );
        }

        #[test]
        fn plan_batch_items_unusable_probe_is_alias_plan() {
            let probes = probe_entries(vec![("N", ProbeOutcome::Unusable)]);
            let items = item_entries(vec![("N", vec![json!(1), json!("x")])]);
            let plans = plan_batch_items(&probes, &items, &[Some(BindTarget::I64), None]);
            assert_eq!(
                plans,
                vec![BatchItemPlan {
                    source: ItemBindSource::AliasPlan,
                    alias_targets: vec![Some(BindTarget::I64), None],
                }]
            );
        }

        #[test]
        fn first_unusable_probe_scans_whole_batch() {
            let probes = probe_entries(vec![
                ("A", ProbeOutcome::Unusable),
                (
                    "B",
                    ProbeOutcome::Described(vec![
                        AnyTypeInfoKind::Text,
                        AnyTypeInfoKind::BigInt,
                        AnyTypeInfoKind::Double,
                    ]),
                ),
            ]);
            let items = item_entries(vec![
                ("A", vec![json!("x"), json!(1.5), json!(1)]),
                (
                    "B",
                    vec![json!("y"), json!(9007199254740993i64), json!(2.5)],
                ),
            ]);
            let rows: Vec<&[serde_json::Value]> =
                items.iter().map(|(_, values)| values.as_slice()).collect();
            let message_scan = scan_bind_targets(&rows);
            assert_eq!(
                message_scan,
                vec![None, Some(BindTarget::F64), Some(BindTarget::F64)]
            );

            let plans = plan_batch_items(&probes, &items, &message_scan);
            assert_eq!(
                plans,
                vec![scan_only(&message_scan), scan_only(&message_scan)],
                "an unusable FIRST probe keeps the legacy scan for every item"
            );

            // The legacy rejection survives: a kept BIGINT target would have
            // silently accepted the integer.
            let err = normalize_for_target(
                &json!(9007199254740993i64),
                Some(BindTarget::F64),
                BindCtx::BatchItem(1),
                1,
            )
            .expect_err("the scan's F64 guard must reject 2^53 + 1");
            assert!(processor_message(&err).contains("2^53"), "got: {err}");
            assert!(
                normalize_for_target(
                    &json!(9007199254740993i64),
                    Some(BindTarget::I64),
                    BindCtx::BatchItem(1),
                    1,
                )
                .is_ok(),
                "a kept BIGINT target would mask the 2^53 rejection"
            );
        }

        #[test]
        fn later_unusable_shape_never_erases_described_guards() {
            // Reverse order: the usable shape is FIRST, the unusable NUMERIC
            // shape is second. The message-wide scan sees INTEGER-only values
            // at the guarded index, so scan-only selection would pick I64 and
            // silently accept the 2^53 integer.
            let probes = probe_entries(vec![
                (
                    "A",
                    ProbeOutcome::Described(vec![
                        AnyTypeInfoKind::Text,
                        AnyTypeInfoKind::Double,
                        AnyTypeInfoKind::Integer,
                    ]),
                ),
                ("B", ProbeOutcome::Unusable),
            ]);
            let items = item_entries(vec![
                ("A", vec![json!("x"), json!(9007199254740993i64), json!(1)]),
                ("B", vec![json!(true), json!(9007199254740993i64), json!(1)]),
            ]);
            let message_scan = vec![None, Some(BindTarget::I64), Some(BindTarget::I64)];

            let plans = plan_batch_items(&probes, &items, &message_scan);
            assert_eq!(plans.len(), 2);
            // The first shape's described DOUBLE survives in its precomputed
            // alias targets (the whole-batch rule then executes on them).
            assert_eq!(plans[0].source, ItemBindSource::NamedReadBack);
            assert_eq!(
                plans[0].alias_targets,
                vec![None, Some(BindTarget::F64), Some(BindTarget::I32)]
            );
            // The later unusable shape contributes scan-only alias targets.
            assert_eq!(plans[1], scan_only(&message_scan));

            let err = normalize_for_target(
                &json!(9007199254740993i64),
                Some(BindTarget::F64),
                BindCtx::BatchItem(0),
                1,
            )
            .expect_err("the kept DOUBLE target must reject 2^53 + 1");
            assert!(processor_message(&err).contains("2^53"), "got: {err}");
            assert!(
                normalize_for_target(
                    &json!(9007199254740993i64),
                    Some(BindTarget::I64),
                    BindCtx::BatchItem(0),
                    1,
                )
                .is_ok(),
                "scan-only I64 selection would silently accept the value"
            );
        }

        // ── planner contract: one decision for the whole batch ───────────

        #[test]
        fn batch_planner_accepts_distinct_expansions_together() {
            // IN-clause expansion resolves each item to its OWN text: two
            // `in (:list)` items with different list lengths plus a third
            // item of a different bind shape. Planning must carry the
            // metadata of each distinct text, not the batch's first text.
            let probes = probe_entries(vec![
                (
                    "select id from t where id in ($1, $2)",
                    ProbeOutcome::Described(vec![
                        AnyTypeInfoKind::Integer,
                        AnyTypeInfoKind::Integer,
                    ]),
                ),
                (
                    "select id from t where id in ($1, $2, $3)",
                    ProbeOutcome::Described(vec![
                        AnyTypeInfoKind::Integer,
                        AnyTypeInfoKind::Integer,
                        AnyTypeInfoKind::BigInt,
                    ]),
                ),
                (
                    "update t set n = $1 where id = $2",
                    ProbeOutcome::Described(vec![AnyTypeInfoKind::Text, AnyTypeInfoKind::BigInt]),
                ),
            ]);
            let items = item_entries(vec![
                (
                    "select id from t where id in ($1, $2)",
                    vec![json!(1), json!(2)],
                ),
                (
                    "select id from t where id in ($1, $2, $3)",
                    vec![json!(1), json!(2), json!(9007199254740993i64)],
                ),
                (
                    "update t set n = $1 where id = $2",
                    vec![json!("x"), json!(2)],
                ),
            ]);
            let rows: Vec<&[serde_json::Value]> =
                items.iter().map(|(_, values)| values.as_slice()).collect();
            let message_scan = scan_bind_targets(&rows);

            let plans = plan_batch_items(&probes, &items, &message_scan);
            assert_eq!(plans.len(), items.len(), "one plan per item");
            assert!(
                plans
                    .iter()
                    .all(|plan| plan.source == ItemBindSource::NamedReadBack),
                "every distinct text described an eligible bind shape"
            );
            // Per-shape metadata follows the item's own text: the IN-clause
            // placeholder count is preserved and the described kinds (not
            // the batch-wide scan) drive each position.
            assert_eq!(plans[0].alias_targets.len(), 2);
            assert_eq!(plans[1].alias_targets.len(), 3);
            assert_eq!(plans[2].alias_targets.len(), 2);
            assert_eq!(
                plans[1].alias_targets,
                vec![
                    Some(BindTarget::I32),
                    Some(BindTarget::I32),
                    Some(BindTarget::I64)
                ]
            );
            assert_eq!(
                plans[2].alias_targets,
                vec![Some(BindTarget::I64), Some(BindTarget::I64)],
                "a described TEXT position maps to no numeric target, so the scan supplies it"
            );

            // The whole-batch decision is ONE value computed before item 0:
            // the item-ordered read-backs agree, so the batch stays named
            // with per-item targets.
            let readbacks = vec![
                Some(vec![AnyTypeInfoKind::Integer, AnyTypeInfoKind::Integer]),
                Some(vec![
                    AnyTypeInfoKind::Integer,
                    AnyTypeInfoKind::Integer,
                    AnyTypeInfoKind::BigInt,
                ]),
                Some(vec![AnyTypeInfoKind::Text, AnyTypeInfoKind::BigInt]),
            ];
            match decide_batch_plan(&plans, &probes, &items, &readbacks) {
                BatchPlan::Named { per_item_targets } => {
                    assert_eq!(
                        per_item_targets.len(),
                        items.len(),
                        "the plan covers every item"
                    );
                    assert_eq!(
                        per_item_targets[0],
                        vec![Some(BindTarget::I32), Some(BindTarget::I32)]
                    );
                    assert_eq!(
                        per_item_targets[1],
                        vec![
                            Some(BindTarget::I32),
                            Some(BindTarget::I32),
                            Some(BindTarget::I64)
                        ],
                        "the three-placeholder expansion keeps its own position count"
                    );
                    assert_eq!(per_item_targets[2], vec![None, Some(BindTarget::I64)]);
                }
                BatchPlan::Aliased { .. } => {
                    panic!("agreeing read-backs must keep the whole batch named")
                }
            }
        }

        #[test]
        fn unusable_later_probe_still_sends_whole_batch_to_aliases() {
            // The usable shape is FIRST, the unusable shape later. One
            // `AliasPlan` item flips the WHOLE batch, and the later shape
            // contributes the message-wide scan targets.
            let probes = probe_entries(vec![
                ("A", ProbeOutcome::Described(vec![AnyTypeInfoKind::Text])),
                ("B", ProbeOutcome::Unusable),
            ]);
            let items = item_entries(vec![
                ("A", vec![json!("x")]),
                ("B", vec![json!(1.5), json!(2)]),
            ]);
            let rows: Vec<&[serde_json::Value]> =
                items.iter().map(|(_, values)| values.as_slice()).collect();
            let message_scan = scan_bind_targets(&rows);
            assert_eq!(
                message_scan,
                vec![Some(BindTarget::F64), Some(BindTarget::I64)],
                "float wins message-wide"
            );

            let plans = plan_batch_items(&probes, &items, &message_scan);
            assert_eq!(plans[0].source, ItemBindSource::NamedReadBack);
            assert_eq!(plans[1].source, ItemBindSource::AliasPlan);

            // No read-back is attempted at all: the alias decision already
            // holds for item 0.
            match decide_batch_plan(&plans, &probes, &items, &[]) {
                BatchPlan::Aliased { targets } => {
                    assert_eq!(targets.len(), items.len(), "the whole batch aliases");
                    assert_eq!(
                        targets[1], message_scan,
                        "the unusable shape runs on the message-wide scan"
                    );
                    assert_eq!(
                        targets[0],
                        vec![message_scan[0]],
                        "the usable shape keeps its scan-derived guard"
                    );
                }
                BatchPlan::Named { .. } => {
                    panic!("an unusable later shape must send the whole batch to aliases")
                }
            }
        }

        #[test]
        fn readback_disagreement_flips_whole_batch() {
            // Extends `decide_path_aliases_on_disagreement` (bd rc-0eiqm)
            // through the planner contract: the cached entry was seeded by
            // bind-derived OIDs, so its read-back disagrees with the probe.
            let probes = probe_entries(vec![(
                "S",
                ProbeOutcome::Described(vec![AnyTypeInfoKind::Double, AnyTypeInfoKind::Integer]),
            )]);
            let items = item_entries(vec![("S", vec![json!(1.5), json!(1)])]);
            let message_scan = scan_bind_targets(&[items[0].1.as_slice()]);
            let plans = plan_batch_items(&probes, &items, &message_scan);
            assert_eq!(plans[0].source, ItemBindSource::NamedReadBack);

            let readbacks = vec![Some(vec![AnyTypeInfoKind::Text, AnyTypeInfoKind::Integer])];
            assert_eq!(
                decide_batch_plan(&plans, &probes, &items, &readbacks),
                BatchPlan::Aliased {
                    targets: vec![vec![Some(BindTarget::F64), Some(BindTarget::I32)]],
                },
                "a disagreeing read-back must flip the whole batch to the aliases"
            );
        }
    }
}
