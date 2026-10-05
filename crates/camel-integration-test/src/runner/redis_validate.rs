//! Redis type-aware projection and value law for the `validate`
//! action's `redis` target (redis-state-tier task 1): the datasource
//! resolution through the catalog, the fail-closed RESP-to-cell law,
//! the coherent atomic-snapshot decoder, the non-monotone poll lattice,
//! and the cell-free mismatch renderers. Split out of the runner core
//! like [`super::sql_validate`] and [`super::surreal_validate`] so the
//! redis executor stays separately navigable; the runner dispatches the
//! `redis` target here.

use std::sync::Arc;
use std::time::Duration;

use camel_matchers::RowsExpectation;

use super::ScenarioFailure;
use crate::document::RedisTarget;

#[cfg(feature = "redis")]
use std::future::Future;

#[cfg(feature = "redis")]
use camel_api::Value;

#[cfg(feature = "redis")]
use camel_matchers::CountBound;

#[cfg(feature = "redis")]
use crate::document::RedisType;

/// The poll interval of a redis validate with a deadline (feature
/// `redis`): one atomic key snapshot every 100 ms until the deadline
/// passes. The snapshot is one read-only `EVAL` round-trip; the sleep
/// between snapshots means the poll never busy-waits.
#[cfg(feature = "redis")]
const REDIS_VALIDATE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The validated TTL status of a snapshot (feature `redis`): the raw
/// `PTTL` is decoded and range-checked at [`decode_snapshot`] time, so
/// `decide` only ever sees one of these three legitimate states.
#[cfg(feature = "redis")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RedisTtlStatus {
    /// `PTTL` replied `-2`: the key does not exist.
    Missing,
    /// `PTTL` replied `-1`: the key exists with no expiry.
    Persistent,
    /// `PTTL` replied a nonnegative whole-millisecond remaining TTL.
    Remaining(usize),
}

/// One coherent snapshot of a key (feature `redis`): the observed
/// type, the projected matcher tuples, the effective projection in
/// projection order, and the validated TTL status. `decode_snapshot`
/// is the single construction site; no raw `i64` PTTL leaks past it.
#[cfg(feature = "redis")]
struct Snapshot {
    observed: Option<RedisType>,
    tuples: Vec<Vec<Value>>,
    columns: Vec<String>,
    ttl: RedisTtlStatus,
}

#[cfg(feature = "redis")]
impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Snapshot")
            .field("observed", &self.observed)
            .field("row_count", &self.tuples.len())
            .field("columns", &self.columns)
            .field("ttl", &self.ttl)
            .finish()
    }
}

/// Asserts the rows/ttl expectation against one key's atomic snapshot
/// (feature `redis`): one immediate snapshot decides without a
/// deadline; with one, the poll re-snapshots at
/// [`REDIS_VALIDATE_POLL_INTERVAL`] and the FINAL snapshot at expiry
/// decides — never an early settle, because Redis key contents are
/// non-monotone (a concurrent mutation shrinks or retypes them). The
/// only mid-window exit is a count bound's ceiling breach.
///
/// Every apparatus failure carries the datasource NAME only — driver
/// errors pass through [`crate::steering::sanitize_db_error`]
/// (ADR-0051) — while the assertion outcome is a verdict-class
/// [`ScenarioFailure::ValidationMismatch`] whose detail renders the
/// expectation shape and the actual row count, never a value, member,
/// field identifier, or the `db_url`.
#[cfg(feature = "redis")]
pub(crate) async fn redis_validate_action(
    index: usize,
    target: &RedisTarget,
    expected: &RowsExpectation,
    deadline: Option<Duration>,
    catalog: Option<&Arc<dyn camel_api::datasource::DatasourceCatalog>>,
) -> Result<(), ScenarioFailure> {
    // Apparatus class (exit 2), mirroring the sql/surreal validate
    // arms: without a catalog the boot-owning caller never passed the
    // cascade's datasources — the scenario cannot reach its subject.
    let Some(catalog) = catalog else {
        return Err(ScenarioFailure::ActionTransport {
            action: index,
            source: crate::adapters::TransportError::Other {
                message: "redis validation: no datasource catalog is available; the \
                          boot-owning caller must pass the cascade's catalog"
                    .to_string(),
            },
        });
    };
    // Datasource resolution through the steering seam: the datasource
    // NAME obeys the identifier law (never a URL), every driver error
    // string is sanitized against the config's `db_url` before the
    // failure (ADR-0051), and the handle must be the driver's own
    // `MultiplexedConnection`. The resolver's message is already
    // complete, so it maps straight into the transport class.
    let name = &target.datasource;
    let (conn, db_url) = crate::steering::resolve_datasource::<redis::aio::MultiplexedConnection>(
        catalog,
        name,
        "redis validation",
    )
    .await
    .map_err(|message| ScenarioFailure::ActionTransport {
        action: index,
        source: crate::adapters::TransportError::Other { message },
    })?;
    // The shared poll driver owns the deadline discipline; the only
    // mid-window exit is a count-bound ceiling breach. Production
    // calls the driver ONCE with the real `eval_raw` source; the
    // colocated tests call it with an injected source.
    poll_with_source(index, target, expected, deadline, move || {
        let conn = conn.clone();
        let key = target.key.clone();
        let script = snapshot_script(target.r#type);
        let db_url = db_url.clone();
        let name = name.clone();
        async move { eval_raw(&conn, &key, script, index, &name, &db_url).await }
    })
    .await
}

/// The feature-off twin (the sql/surreal no-feature precedent): the
/// grammar is ungated, so a well-formed redis validate target parses
/// in every build — only a directly-constructed call reaches this arm,
/// and it fails with the verdict-class mismatch naming the gate
/// instead of passing silently.
#[cfg(not(feature = "redis"))]
pub(crate) async fn redis_validate_action(
    index: usize,
    target: &RedisTarget,
    expected: &RowsExpectation,
    deadline: Option<Duration>,
    catalog: Option<&Arc<dyn camel_api::datasource::DatasourceCatalog>>,
) -> Result<(), ScenarioFailure> {
    let _ = (target, expected, deadline, catalog);
    Err(ScenarioFailure::ValidationMismatch {
        action: index,
        detail: "redis validation requires the `redis` feature".to_string(),
    })
}

/// One apparatus failure (feature `redis`): the transport class
/// carrying the datasource NAME and the pre-sanitized detail — the
/// `db_url` never renders (ADR-0051).
#[cfg(feature = "redis")]
fn apparatus(index: usize, name: &str, text: String) -> ScenarioFailure {
    ScenarioFailure::ActionTransport {
        action: index,
        source: crate::adapters::TransportError::Other {
            message: format!("redis validation: datasource '{name}': {text}"),
        },
    }
}

/// The ONE mapping for every projection/value-law failure (feature
/// `redis`): it delegates to [`apparatus`], so a fail-closed cell
/// classification is apparatus-class, never verdict-class.
#[cfg(feature = "redis")]
fn projection_error(index: usize, name: &str, detail: String) -> ScenarioFailure {
    apparatus(index, name, detail)
}

/// Executes the fixed snapshot script as ONE server-side call (feature
/// `redis`): a spec-pinned script string, `numkeys` 1, and the
/// document-authored key via `redis::cmd("EVAL")` (not
/// `redis::Script`, which needs the non-default `script` feature). A
/// driver/connection error is apparatus with the text sanitized
/// against `db_url`.
#[cfg(feature = "redis")]
async fn eval_raw(
    conn: &redis::aio::MultiplexedConnection,
    key: &str,
    script: &str,
    index: usize,
    name: &str,
    db_url: &str,
) -> Result<redis::Value, ScenarioFailure> {
    redis::cmd("EVAL")
        .arg(script)
        .arg(1)
        .arg(key)
        .query_async::<redis::Value>(&mut conn.clone())
        .await
        .map_err(|e| {
            apparatus(
                index,
                name,
                crate::steering::sanitize_db_error(&e.to_string(), db_url),
            )
        })
}

/// Polls [`eval_raw`] snapshots through the shared driver (feature
/// `redis`). `source` is the deterministic seam: production binds the
/// real `eval_raw`, and the tests inject a scripted reply source that
/// repeats its last reply. The only early judgment is the count-bound
/// ceiling breach; Redis contents are not monotone, so nothing else
/// settles early.
#[cfg(feature = "redis")]
async fn poll_with_source<F, Fut>(
    index: usize,
    target: &RedisTarget,
    expected: &RowsExpectation,
    deadline: Option<Duration>,
    mut source: F,
) -> Result<(), ScenarioFailure>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<redis::Value, ScenarioFailure>>,
{
    super::poll::poll_until(
        deadline,
        REDIS_VALIDATE_POLL_INTERVAL,
        move || {
            let fut = source();
            async move {
                let raw = fut.await?;
                decode_snapshot(index, target, expected, raw)
            }
        },
        move |snapshot: &Snapshot| {
            expected
                .bound
                .as_ref()
                .is_some_and(|bound| camel_matchers::above_ceiling(bound, snapshot.tuples.len()))
                .then(|| Err(mismatch(index, target, expected, snapshot)))
        },
        move |snapshot: &Snapshot| decide(index, target, expected, snapshot),
    )
    .await
}

/// Decodes one raw script reply into a [`Snapshot`] (feature `redis`):
/// the SINGLE decode site. A reply that is not the expected
/// three-element array, an unsupported observed type, and an invalid
/// `PTTL` are apparatus failures. A missing key or a differing
/// observed type is verdict DATA (empty tuples), never an `Err`, so an
/// initially missing or transiently wrong-typed key recovers by the
/// deadline. When the observed type equals the declared type, the
/// payload projects through [`redis_rows_to_tuples`].
#[cfg(feature = "redis")]
fn decode_snapshot(
    index: usize,
    target: &RedisTarget,
    expected: &RowsExpectation,
    raw: redis::Value,
) -> Result<Snapshot, ScenarioFailure> {
    let name = &target.datasource;
    let redis::Value::Array(parts) = raw else {
        return Err(apparatus(
            index,
            name,
            "snapshot reply is not an array (fail-closed)".to_string(),
        ));
    };
    if parts.len() != 3 {
        return Err(apparatus(
            index,
            name,
            format!(
                "snapshot reply must carry three elements, got {} (fail-closed)",
                parts.len()
            ),
        ));
    }
    let observed =
        redis_observed_type(&parts[0]).map_err(|detail| projection_error(index, name, detail))?;
    let pttl = match &parts[2] {
        redis::Value::Int(n) => *n,
        _ => {
            return Err(apparatus(
                index,
                name,
                "snapshot PTTL reply is not an integer (fail-closed)".to_string(),
            ));
        }
    };
    // The raw PTTL is validated on EVERY snapshot, with or without a
    // declared ttl bound, so `decide` only ever sees a legitimate
    // status and the executor never stores a status it cannot use.
    let ttl = redis_ttl_status(pttl).map_err(|detail| apparatus(index, name, detail))?;
    let columns: Vec<String> = match &expected.columns {
        Some(columns) => columns.clone(),
        None => target
            .r#type
            .schema()
            .iter()
            .map(|column| (*column).to_string())
            .collect(),
    };
    let tuples = if observed == Some(target.r#type) {
        let projection: Vec<&str> = columns.iter().map(String::as_str).collect();
        redis_rows_to_tuples(target.r#type, &parts[1], &projection)
            .map_err(|detail| projection_error(index, name, detail))?
    } else {
        Vec::new()
    };
    Ok(Snapshot {
        observed,
        tuples,
        columns,
        ttl,
    })
}

/// The ONLY verdict-class producer (feature `redis`): type agreement,
/// the rows/bound matcher, and the declared `ttl` bound. It never sees
/// an `Err` status and never classifies a projection failure.
#[cfg(feature = "redis")]
fn decide(
    index: usize,
    target: &RedisTarget,
    expected: &RowsExpectation,
    snapshot: &Snapshot,
) -> Result<(), ScenarioFailure> {
    if snapshot.observed == Some(target.r#type) {
        let rows_hold = match (&expected.rows, &expected.bound) {
            (Some(rows), _) => {
                camel_matchers::rows_match(rows, &snapshot.tuples, expected.unordered)
            }
            (None, Some(bound)) => camel_matchers::bound_holds(bound, snapshot.tuples.len()),
            // The parser guarantees exactly one shape (`rows` XOR
            // `bound`); an empty expectation fails closed.
            (None, None) => false,
        };
        let ttl_ok = match &target.ttl {
            Some(bound) => ttl_holds(&snapshot.ttl, bound),
            None => true,
        };
        if rows_hold && ttl_ok {
            return Ok(());
        }
    }
    Err(mismatch(index, target, expected, snapshot))
}

/// The verdict-class mismatch (feature `redis`): cell-free — it renders
/// only the datasource, the document-authored key, the declared and
/// observed type names, the expectation shape, the actual row count,
/// the schema columns, and (when a ttl bound is declared) the rendered
/// bound and observed TTL status. It never receives the `db_url` and
/// never renders a value, member, or field identifier.
#[cfg(feature = "redis")]
fn mismatch(
    index: usize,
    target: &RedisTarget,
    expected: &RowsExpectation,
    snapshot: &Snapshot,
) -> ScenarioFailure {
    ScenarioFailure::ValidationMismatch {
        action: index,
        detail: redis_mismatch_detail(target, expected, snapshot),
    }
}

/// Renders the cell-free mismatch detail (feature `redis`). Exposed to
/// the colocated tests through the module (private, not public API).
#[cfg(feature = "redis")]
fn redis_mismatch_detail(
    target: &RedisTarget,
    expected: &RowsExpectation,
    snapshot: &Snapshot,
) -> String {
    let observed = snapshot.observed.map_or("none", RedisType::as_str);
    let shape = if let Some(bound) = &expected.bound {
        camel_matchers::render_bound(bound)
    } else {
        let rows = expected.rows.as_ref().map_or(0, Vec::len);
        format!(
            "expected {rows} rows ({})",
            if expected.unordered {
                "unordered"
            } else {
                "ordered"
            }
        )
    };
    let mut detail = format!(
        "redis validation: datasource '{}', key '{}', declared {}, observed {}, {}, actual {} \
         rows, columns: [{}]",
        target.datasource,
        target.key,
        target.r#type.as_str(),
        observed,
        shape,
        snapshot.tuples.len(),
        snapshot.columns.join(", ")
    );
    if let Some(bound) = &target.ttl {
        detail.push_str(&format!(
            ", ttl {} ({})",
            camel_matchers::render_bound(bound),
            render_ttl_status(&snapshot.ttl)
        ));
    }
    detail
}

/// Renders the observed TTL status for a mismatch detail (feature
/// `redis`): the missing/persistent token or the remaining whole
/// milliseconds.
#[cfg(feature = "redis")]
fn render_ttl_status(status: &RedisTtlStatus) -> String {
    match status {
        RedisTtlStatus::Missing => "missing".to_string(),
        RedisTtlStatus::Persistent => "persistent".to_string(),
        RedisTtlStatus::Remaining(ms) => format!("remaining {ms} ms"),
    }
}

/// Whether a validated TTL status satisfies a declared bound (feature
/// `redis`, infallible): a missing or persistent key fails every legal
/// (positive) bound; a remaining value goes through the shared
/// count-bound algebra. All legal bounds are positive, so a remaining
/// `0` satisfies every legal `atMost` and fails every legal `atLeast`
/// and every `Range`.
#[cfg(feature = "redis")]
fn ttl_holds(status: &RedisTtlStatus, bound: &CountBound) -> bool {
    match status {
        RedisTtlStatus::Missing | RedisTtlStatus::Persistent => false,
        RedisTtlStatus::Remaining(ms) => camel_matchers::bound_holds(bound, *ms),
    }
}

/// Maps one raw RESP value to a matcher cell (feature `redis`):
/// `Nil` -> null; `Int` -> number; a simple or bulk string -> a string
/// when the bytes are valid UTF-8. Any other reply kind and a non-UTF-8
/// bulk string fail closed naming the schema `column` and the row
/// `ordinal` — never a silent null, never a lossy replacement.
#[cfg(feature = "redis")]
pub(crate) fn redis_value_to_cell(
    v: &redis::Value,
    column: &str,
    ordinal: usize,
) -> Result<Value, String> {
    match v {
        redis::Value::Nil => Ok(Value::Null),
        redis::Value::Int(n) => Ok(Value::from(*n)),
        redis::Value::SimpleString(s) => Ok(Value::String(s.clone())),
        redis::Value::BulkString(bytes) => std::str::from_utf8(bytes)
            .map(|s| Value::String(s.to_string()))
            .map_err(|_| {
                format!("column `{column}` row {ordinal}: reply is not valid UTF-8 (fail-closed)")
            }),
        _ => Err(format!(
            "column `{column}` row {ordinal}: unsupported reply kind for the value law \
             (fail-closed)"
        )),
    }
}

/// Parses a sorted-set score and normalizes it to a matcher cell
/// (feature `redis`): a finite `f64`; an integral value (`fract() ==
/// 0.0`) becomes a JSON integer only inside `[-2^63, 2^63)` — inclusive
/// lower, exclusive upper — checked BEFORE the conversion, with no
/// saturating cast; outside that range (`+2^63`, `1e20`) it fails
/// closed; a non-integral value becomes a JSON float. A parse failure
/// or non-finite value fails closed naming `column` and `ordinal`.
#[cfg(feature = "redis")]
pub(crate) fn normalize_score(raw: &str, column: &str, ordinal: usize) -> Result<Value, String> {
    let parsed: f64 = raw.parse().map_err(|_| {
        format!("column `{column}` row {ordinal}: score is not a number (fail-closed)")
    })?;
    if !parsed.is_finite() {
        return Err(format!(
            "column `{column}` row {ordinal}: score is not finite (fail-closed)"
        ));
    }
    if parsed.fract() == 0.0 {
        if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&parsed) {
            return Ok(Value::from(parsed as i64));
        }
        return Err(format!(
            "column `{column}` row {ordinal}: integral score is outside the accepted \
             integer range (fail-closed)"
        ));
    }
    serde_json::Number::from_f64(parsed)
        .map(Value::Number)
        .ok_or_else(|| {
            format!(
                "column `{column}` row {ordinal}: score has no JSON float form \
                 (fail-closed)"
            )
        })
}

/// Decodes the raw `PTTL` integer into a [`RedisTtlStatus`] (feature
/// `redis`): `-2` missing, `-1` persistent, `>= 0` remaining whole
/// milliseconds through a checked `usize::try_from`; any other negative
/// (`-3`, `i64::MIN`) fails closed.
#[cfg(feature = "redis")]
fn redis_ttl_status(raw: i64) -> Result<RedisTtlStatus, String> {
    match raw {
        -2 => Ok(RedisTtlStatus::Missing),
        -1 => Ok(RedisTtlStatus::Persistent),
        n if n >= 0 => usize::try_from(n)
            .map(RedisTtlStatus::Remaining)
            .map_err(|_| format!("snapshot PTTL value `{n}` overflows usize (fail-closed)")),
        other => Err(format!(
            "snapshot PTTL value `{other}` is unknown (fail-closed)"
        )),
    }
}

/// Classifies an observed `TYPE` reply (feature `redis`): `none` -> no
/// key; the five known type names -> the declared type; any other
/// observed kind (for example `stream`) fails closed as apparatus.
#[cfg(feature = "redis")]
pub(crate) fn redis_observed_type(v: &redis::Value) -> Result<Option<RedisType>, String> {
    let name = match v {
        redis::Value::SimpleString(name) => name.as_str(),
        redis::Value::BulkString(bytes) => std::str::from_utf8(bytes)
            .map_err(|_| "snapshot TYPE reply is not UTF-8 (fail-closed)".to_string())?,
        _ => return Err("snapshot TYPE reply is not a string (fail-closed)".to_string()),
    };
    if name == "none" {
        return Ok(None);
    }
    RedisType::from_name(name)
        .map(Some)
        .ok_or_else(|| format!("unsupported observed redis type `{name}` (fail-closed)"))
}

/// Projects one raw payload into matcher tuples for the declared type
/// (feature `redis`), applying the `columns` projection in declaration
/// order. `string` reads one `[value]` row (Nil -> null); `hash` pairs
/// `[field, value]` ordered by field; `list` reads `[index, value]`;
/// `set` reads `[member]` ordered lexicographically; `zset` reads
/// `[member, score]` in rank order with the score through
/// [`normalize_score`]. An empty container yields zero rows.
#[cfg(feature = "redis")]
pub(crate) fn redis_rows_to_tuples(
    value_type: RedisType,
    payload: &redis::Value,
    columns: &[&str],
) -> Result<Vec<Vec<Value>>, String> {
    let canonical: Vec<Vec<Value>> = match value_type {
        RedisType::String => vec![vec![redis_value_to_cell(payload, "value", 0)?]],
        RedisType::Hash => hash_rows(payload)?,
        RedisType::List => list_rows(payload)?,
        RedisType::Set => set_rows(payload)?,
        RedisType::Zset => zset_rows(payload)?,
    };
    let schema = value_type.schema();
    let mut projected = Vec::with_capacity(canonical.len());
    for row in &canonical {
        let mut tuple = Vec::with_capacity(columns.len());
        for column in columns {
            let index = schema
                .iter()
                .position(|name| name == column)
                .ok_or_else(|| {
                    format!(
                        "column `{column}` is not part of the {} schema (fail-closed)",
                        value_type.as_str()
                    )
                })?;
            tuple.push(row[index].clone());
        }
        projected.push(tuple);
    }
    Ok(projected)
}

/// The `hash` canonical rows (feature `redis`): the `HGETALL` reply
/// pairs field/value, ordered by field. An odd element count fails
/// closed naming `value` and the row ordinal; the field bytes never
/// render.
#[cfg(feature = "redis")]
fn hash_rows(payload: &redis::Value) -> Result<Vec<Vec<Value>>, String> {
    let redis::Value::Array(elements) = payload else {
        return Err("hash reply is not an array (fail-closed)".to_string());
    };
    let mut pairs: Vec<(String, Value)> = Vec::with_capacity(elements.len() / 2);
    let (chunks, remainder) = elements.as_chunks::<2>();
    for (ordinal, pair) in chunks.iter().enumerate() {
        let field = redis_value_to_cell(&pair[0], "field", ordinal)?;
        let Value::String(field) = field else {
            return Err(format!(
                "column `field` row {ordinal}: hash field is not a string (fail-closed)"
            ));
        };
        let value = redis_value_to_cell(&pair[1], "value", ordinal)?;
        pairs.push((field, value));
    }
    if !remainder.is_empty() {
        return Err(format!(
            "column `value` row {}: hash reply has an odd number of field/value elements \
             (fail-closed)",
            pairs.len()
        ));
    }
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(pairs
        .into_iter()
        .map(|(field, value)| vec![Value::String(field), value])
        .collect())
}

/// The `list` canonical rows (feature `redis`): each `LRANGE` element
/// becomes `[index, value]` with a numeric index.
#[cfg(feature = "redis")]
fn list_rows(payload: &redis::Value) -> Result<Vec<Vec<Value>>, String> {
    let redis::Value::Array(elements) = payload else {
        return Err("list reply is not an array (fail-closed)".to_string());
    };
    let mut rows = Vec::with_capacity(elements.len());
    for (ordinal, element) in elements.iter().enumerate() {
        let value = redis_value_to_cell(element, "value", ordinal)?;
        rows.push(vec![Value::from(ordinal as i64), value]);
    }
    Ok(rows)
}

/// The `set` canonical rows (feature `redis`): each `SMEMBERS` member
/// becomes `[member]`, ordered lexicographically.
#[cfg(feature = "redis")]
fn set_rows(payload: &redis::Value) -> Result<Vec<Vec<Value>>, String> {
    let redis::Value::Array(elements) = payload else {
        return Err("set reply is not an array (fail-closed)".to_string());
    };
    let mut members = Vec::with_capacity(elements.len());
    for (ordinal, element) in elements.iter().enumerate() {
        let member = redis_value_to_cell(element, "member", ordinal)?;
        let Value::String(member) = member else {
            return Err(format!(
                "column `member` row {ordinal}: set member is not a string (fail-closed)"
            ));
        };
        members.push(member);
    }
    members.sort();
    Ok(members
        .into_iter()
        .map(|member| vec![Value::String(member)])
        .collect())
}

/// The `zset` canonical rows (feature `redis`): the `ZRANGE ...
/// WITHSCORES` reply pairs member/score in rank order, the score
/// through [`normalize_score`]. An odd element count fails closed
/// naming `score` and the row ordinal.
#[cfg(feature = "redis")]
fn zset_rows(payload: &redis::Value) -> Result<Vec<Vec<Value>>, String> {
    let redis::Value::Array(elements) = payload else {
        return Err("zset reply is not an array (fail-closed)".to_string());
    };
    let mut rows = Vec::with_capacity(elements.len() / 2);
    let (chunks, remainder) = elements.as_chunks::<2>();
    for (ordinal, pair) in chunks.iter().enumerate() {
        let member = redis_value_to_cell(&pair[0], "member", ordinal)?;
        let raw_score = match &pair[1] {
            redis::Value::BulkString(bytes) => std::str::from_utf8(bytes)
                .map_err(|_| {
                    format!(
                        "column `score` row {ordinal}: score bytes are not valid UTF-8 \
                         (fail-closed)"
                    )
                })?
                .to_string(),
            redis::Value::SimpleString(text) => text.clone(),
            _ => {
                return Err(format!(
                    "column `score` row {ordinal}: score reply is not a string (fail-closed)"
                ));
            }
        };
        let score = normalize_score(&raw_score, "score", ordinal)?;
        rows.push(vec![member, score]);
    }
    if !remainder.is_empty() {
        return Err(format!(
            "column `score` row {}: zset reply has an odd number of member/score elements \
             (fail-closed)",
            rows.len()
        ));
    }
    Ok(rows)
}

/// The fixed, read-only Lua snapshot body for a declared type (feature
/// `redis`): `TYPE`, the declared type's read guarded so a mistyped key
/// cannot raise `WRONGTYPE`, and `PTTL`, in one server-side execution.
#[cfg(feature = "redis")]
const SNAPSHOT_SCRIPT_STRING: &str = "\
local t = redis.call('TYPE', KEYS[1])['ok'] \
local payload = false \
if t == 'string' then payload = redis.call('GET', KEYS[1]) end \
return {t, payload, redis.call('PTTL', KEYS[1])}";

#[cfg(feature = "redis")]
const SNAPSHOT_SCRIPT_HASH: &str = "\
local t = redis.call('TYPE', KEYS[1])['ok'] \
local payload = false \
if t == 'hash' then payload = redis.call('HGETALL', KEYS[1]) end \
return {t, payload, redis.call('PTTL', KEYS[1])}";

#[cfg(feature = "redis")]
const SNAPSHOT_SCRIPT_LIST: &str = "\
local t = redis.call('TYPE', KEYS[1])['ok'] \
local payload = false \
if t == 'list' then payload = redis.call('LRANGE', KEYS[1], 0, -1) end \
return {t, payload, redis.call('PTTL', KEYS[1])}";

#[cfg(feature = "redis")]
const SNAPSHOT_SCRIPT_SET: &str = "\
local t = redis.call('TYPE', KEYS[1])['ok'] \
local payload = false \
if t == 'set' then payload = redis.call('SMEMBERS', KEYS[1]) end \
return {t, payload, redis.call('PTTL', KEYS[1])}";

#[cfg(feature = "redis")]
const SNAPSHOT_SCRIPT_ZSET: &str = "\
local t = redis.call('TYPE', KEYS[1])['ok'] \
local payload = false \
if t == 'zset' then payload = redis.call('ZRANGE', KEYS[1], 0, -1, 'WITHSCORES') end \
return {t, payload, redis.call('PTTL', KEYS[1])}";

/// Selects the fixed snapshot body for the declared type (feature
/// `redis`). The executor dispatches in Rust; the script never infers
/// the read from the observed type.
#[cfg(feature = "redis")]
pub(crate) fn snapshot_script(value_type: RedisType) -> &'static str {
    match value_type {
        RedisType::String => SNAPSHOT_SCRIPT_STRING,
        RedisType::Hash => SNAPSHOT_SCRIPT_HASH,
        RedisType::List => SNAPSHOT_SCRIPT_LIST,
        RedisType::Set => SNAPSHOT_SCRIPT_SET,
        RedisType::Zset => SNAPSHOT_SCRIPT_ZSET,
    }
}

#[cfg(test)]
mod tests {
    // -----------------------------------------------------------------
    // Pure law: value/projection/TTL. All feature `redis`.
    // -----------------------------------------------------------------
    #[cfg(feature = "redis")]
    mod law {
        use camel_api::Value as Cell;
        use camel_matchers::CountBound;
        use redis::Value as Resp;
        use serde_json::json;

        use super::super::{
            RedisTtlStatus, RedisType, normalize_score, redis_observed_type, redis_rows_to_tuples,
            redis_ttl_status, redis_value_to_cell, snapshot_script, ttl_holds,
        };

        #[test]
        fn string_projection_reads_one_value_row() {
            let payload = Resp::BulkString(b"alice".to_vec());
            let rows = redis_rows_to_tuples(RedisType::String, &payload, &["value"]).unwrap();
            assert_eq!(rows, vec![vec![Cell::String("alice".to_string())]]);
        }

        #[test]
        fn string_nil_reply_is_null_cell() {
            let payload = Resp::Nil;
            let rows = redis_rows_to_tuples(RedisType::String, &payload, &["value"]).unwrap();
            assert_eq!(rows, vec![vec![Cell::Null]]);
        }

        #[test]
        fn hash_projection_orders_fields_by_field() {
            let payload = Resp::Array(vec![
                Resp::BulkString(b"b".to_vec()),
                Resp::BulkString(b"2".to_vec()),
                Resp::BulkString(b"a".to_vec()),
                Resp::BulkString(b"1".to_vec()),
            ]);
            let rows =
                redis_rows_to_tuples(RedisType::Hash, &payload, &["field", "value"]).unwrap();
            assert_eq!(
                rows,
                vec![
                    vec![Cell::String("a".to_string()), Cell::String("1".to_string())],
                    vec![Cell::String("b".to_string()), Cell::String("2".to_string())],
                ]
            );
        }

        #[test]
        fn hash_wrong_arity_reply_fails_closed_by_column_and_ordinal() {
            let payload = Resp::Array(vec![Resp::BulkString(b"a".to_vec())]);
            let err =
                redis_rows_to_tuples(RedisType::Hash, &payload, &["field", "value"]).unwrap_err();
            assert!(err.contains("value"), "must name the value column: {err}");
            assert!(err.contains('0'), "must name the row ordinal: {err}");
            assert!(!err.contains("\"a\""), "must not name the field: {err}");
        }

        #[test]
        fn list_projection_reads_index_value_rows() {
            let payload = Resp::Array(vec![
                Resp::BulkString(b"a".to_vec()),
                Resp::BulkString(b"b".to_vec()),
                Resp::BulkString(b"c".to_vec()),
            ]);
            let rows =
                redis_rows_to_tuples(RedisType::List, &payload, &["index", "value"]).unwrap();
            assert_eq!(
                rows,
                vec![
                    vec![Cell::from(0i64), Cell::String("a".to_string())],
                    vec![Cell::from(1i64), Cell::String("b".to_string())],
                    vec![Cell::from(2i64), Cell::String("c".to_string())],
                ]
            );
        }

        #[test]
        fn set_projection_orders_members_lexicographically() {
            let payload = Resp::Array(vec![
                Resp::BulkString(b"c".to_vec()),
                Resp::BulkString(b"a".to_vec()),
                Resp::BulkString(b"b".to_vec()),
            ]);
            let rows = redis_rows_to_tuples(RedisType::Set, &payload, &["member"]).unwrap();
            assert_eq!(
                rows,
                vec![
                    vec![Cell::String("a".to_string())],
                    vec![Cell::String("b".to_string())],
                    vec![Cell::String("c".to_string())],
                ]
            );
        }

        #[test]
        fn zset_projection_reads_member_score_rows_in_rank_order() {
            let payload = Resp::Array(vec![
                Resp::BulkString(b"bob".to_vec()),
                Resp::BulkString(b"1.5".to_vec()),
                Resp::BulkString(b"alice".to_vec()),
                Resp::BulkString(b"2".to_vec()),
            ]);
            let rows =
                redis_rows_to_tuples(RedisType::Zset, &payload, &["member", "score"]).unwrap();
            assert_eq!(
                rows,
                vec![
                    vec![Cell::String("bob".to_string()), json!(1.5)],
                    vec![Cell::String("alice".to_string()), json!(2)],
                ]
            );
        }

        #[test]
        fn columns_select_subset_projection() {
            let payload = Resp::Array(vec![
                Resp::BulkString(b"name".to_vec()),
                Resp::BulkString(b"alice".to_vec()),
                Resp::BulkString(b"age".to_vec()),
                Resp::BulkString(b"42".to_vec()),
            ]);
            let rows = redis_rows_to_tuples(RedisType::Hash, &payload, &["value"]).unwrap();
            assert_eq!(
                rows,
                vec![
                    vec![Cell::String("42".to_string())],
                    vec![Cell::String("alice".to_string())],
                ]
            );
        }

        #[test]
        fn columns_reorder_projection() {
            let payload = Resp::Array(vec![
                Resp::BulkString(b"name".to_vec()),
                Resp::BulkString(b"alice".to_vec()),
                Resp::BulkString(b"age".to_vec()),
                Resp::BulkString(b"42".to_vec()),
            ]);
            let rows =
                redis_rows_to_tuples(RedisType::Hash, &payload, &["value", "field"]).unwrap();
            assert_eq!(
                rows,
                vec![
                    vec![
                        Cell::String("42".to_string()),
                        Cell::String("age".to_string())
                    ],
                    vec![
                        Cell::String("alice".to_string()),
                        Cell::String("name".to_string())
                    ],
                ]
            );
        }

        #[test]
        fn empty_container_yields_zero_rows() {
            let payload = Resp::Array(Vec::new());
            let rows = redis_rows_to_tuples(RedisType::Set, &payload, &["member"]).unwrap();
            assert_eq!(rows, Vec::<Vec<Cell>>::new());
        }

        #[test]
        fn unsupported_reply_kind_fails_closed_by_column_and_ordinal() {
            let err = redis_value_to_cell(&Resp::Okay, "value", 0).unwrap_err();
            assert!(err.contains("value"), "must name the column: {err}");
            assert!(err.contains('0'), "must name the ordinal: {err}");
        }

        #[test]
        fn non_utf8_cell_fails_closed_by_column_and_ordinal() {
            let err =
                redis_value_to_cell(&Resp::BulkString(vec![0xff, 0xfe]), "value", 0).unwrap_err();
            assert!(err.contains("value"), "must name the column: {err}");
            assert!(err.contains('0'), "must name the ordinal: {err}");
        }

        #[test]
        fn integer_reply_maps_to_number() {
            let cell = redis_value_to_cell(&Resp::Int(42), "value", 0).unwrap();
            assert_eq!(cell, Cell::from(42i64));
        }

        #[test]
        fn valid_utf8_bulk_string_maps_to_string() {
            let cell =
                redis_value_to_cell(&Resp::BulkString(b"hello".to_vec()), "value", 0).unwrap();
            assert_eq!(cell, Cell::String("hello".to_string()));
        }

        fn assert_integral(result: Result<Cell, String>, expected: i64) {
            match result {
                Ok(Cell::Number(n)) => {
                    assert!(n.is_i64(), "must be a JSON integer, got {n}");
                    assert_eq!(Cell::Number(n), json!(expected));
                }
                other => panic!("expected an integral score, got {other:?}"),
            }
        }

        #[test]
        fn integral_score_normalizes_to_integer() {
            assert_integral(normalize_score("2", "score", 0), 2);
            assert_integral(normalize_score("2.0", "score", 0), 2);
        }

        #[test]
        fn non_integral_score_matches_float() {
            match normalize_score("1.5", "score", 0) {
                Ok(Cell::Number(n)) => {
                    assert!(n.is_f64(), "must be a JSON float, got {n}");
                    assert_eq!(Cell::Number(n), json!(1.5));
                }
                other => panic!("expected a float score, got {other:?}"),
            }
        }

        #[test]
        fn i64_lower_bound_score_is_accepted_as_integer() {
            assert_integral(
                normalize_score("-9223372036854775808", "score", 0),
                i64::MIN,
            );
        }

        #[test]
        fn plus_2_pow_63_score_fails_closed_apparatus() {
            assert!(normalize_score("9223372036854775808", "score", 0).is_err());
        }

        #[test]
        fn large_integral_score_fails_closed_apparatus() {
            assert!(normalize_score("1e20", "score", 0).is_err());
        }

        #[test]
        fn non_finite_score_fails_closed() {
            for raw in ["inf", "nan"] {
                let err = normalize_score(raw, "score", 0).unwrap_err();
                assert!(err.contains("score"), "must name the column: {err}");
                assert!(err.contains('0'), "must name the ordinal: {err}");
            }
        }

        #[test]
        fn redis_observed_type_maps_none_and_known_types() {
            assert_eq!(
                redis_observed_type(&Resp::SimpleString("none".to_string())),
                Ok(None)
            );
            for (name, ty) in [
                ("string", RedisType::String),
                ("hash", RedisType::Hash),
                ("list", RedisType::List),
                ("set", RedisType::Set),
                ("zset", RedisType::Zset),
            ] {
                assert_eq!(
                    redis_observed_type(&Resp::SimpleString(name.to_string())),
                    Ok(Some(ty)),
                    "observed type `{name}` must map"
                );
            }
            assert!(redis_observed_type(&Resp::SimpleString("stream".to_string())).is_err());
        }

        #[test]
        fn redis_ttl_status_decodes_missing_persistent_and_remaining() {
            assert_eq!(redis_ttl_status(-2), Ok(RedisTtlStatus::Missing));
            assert_eq!(redis_ttl_status(-1), Ok(RedisTtlStatus::Persistent));
            assert_eq!(redis_ttl_status(0), Ok(RedisTtlStatus::Remaining(0)));
            assert_eq!(
                redis_ttl_status(60_000),
                Ok(RedisTtlStatus::Remaining(60_000))
            );
        }

        #[test]
        fn redis_ttl_status_unknown_negative_fails_closed() {
            assert!(redis_ttl_status(-3).is_err());
            assert!(redis_ttl_status(i64::MIN).is_err());
        }

        #[test]
        fn ttl_holds_missing_and_persistent_fail() {
            let bound = CountBound::AtLeast(1);
            assert!(!ttl_holds(&RedisTtlStatus::Missing, &bound));
            assert!(!ttl_holds(&RedisTtlStatus::Persistent, &bound));
        }

        #[test]
        fn ttl_holds_at_most_fails_above_bound() {
            assert!(!ttl_holds(
                &RedisTtlStatus::Remaining(60_000),
                &CountBound::AtMost(30_000)
            ));
        }

        #[test]
        fn ttl_holds_zero_remaining_satisfies_at_most_not_at_least_or_range() {
            let status = RedisTtlStatus::Remaining(0);
            assert!(ttl_holds(&status, &CountBound::AtMost(1)));
            assert!(!ttl_holds(&status, &CountBound::AtLeast(1)));
            assert!(!ttl_holds(&status, &CountBound::Range(1, 60_000)));
        }

        #[test]
        fn snapshot_script_selects_declared_read() {
            for (ty, read) in [
                (RedisType::String, "GET"),
                (RedisType::Hash, "HGETALL"),
                (RedisType::List, "LRANGE"),
                (RedisType::Set, "SMEMBERS"),
                (RedisType::Zset, "ZRANGE"),
            ] {
                let body = snapshot_script(ty);
                assert!(body.contains("TYPE"), "{ty:?} script must issue TYPE");
                assert!(body.contains(read), "{ty:?} script must issue {read}");
                assert!(body.contains("PTTL"), "{ty:?} script must issue PTTL");
            }
        }
    }

    // -----------------------------------------------------------------
    // Decode/decision + injected-source executor. All feature `redis`.
    // -----------------------------------------------------------------
    #[cfg(feature = "redis")]
    mod decide {
        use std::cell::Cell;
        use std::collections::{BTreeMap, HashMap};
        use std::rc::Rc;
        use std::sync::Arc;
        use std::time::Duration;

        use camel_api::Value;
        use camel_matchers::{CountBound, Expectation, RowsExpectation};
        use serde_json::json;

        use super::super::{
            RedisTtlStatus, RedisType, Snapshot, decide, decode_snapshot, poll_with_source,
            projection_error, redis_validate_action,
        };
        use crate::adapters::TransportError;
        use crate::document::{
            RedisTarget, RouteSource, ScenarioDocument, ScenarioTarget, ValidateExpectation,
        };
        use crate::runner::{
            PartnerRouter, ScenarioAction, ScenarioFailure, ScenarioVars, run_scenario,
        };

        fn redis_target(ty: RedisType) -> RedisTarget {
            RedisTarget {
                datasource: "statedb".to_string(),
                key: "k".to_string(),
                r#type: ty,
                ttl: None,
            }
        }

        fn rows_expected(columns: Option<&[&str]>, rows: Vec<Vec<Expectation>>) -> RowsExpectation {
            RowsExpectation {
                columns: columns.map(|c| c.iter().map(|s| s.to_string()).collect()),
                unordered: false,
                rows: Some(rows),
                bound: None,
            }
        }

        fn bound_expected(bound: CountBound) -> RowsExpectation {
            RowsExpectation {
                columns: None,
                unordered: false,
                rows: None,
                bound: Some(bound),
            }
        }

        fn string_reply(value: &str, pttl: i64) -> redis::Value {
            redis::Value::Array(vec![
                redis::Value::SimpleString("string".to_string()),
                redis::Value::BulkString(value.as_bytes().to_vec()),
                redis::Value::Int(pttl),
            ])
        }

        fn missing_reply() -> redis::Value {
            redis::Value::Array(vec![
                redis::Value::SimpleString("none".to_string()),
                redis::Value::Array(Vec::new()),
                redis::Value::Int(-2),
            ])
        }

        fn scripted(
            replies: Vec<redis::Value>,
            calls: Rc<Cell<usize>>,
        ) -> impl FnMut() -> std::future::Ready<Result<redis::Value, ScenarioFailure>> {
            let mut i = 0usize;
            move || {
                calls.set(calls.get() + 1);
                let idx = i.min(replies.len() - 1);
                i += 1;
                std::future::ready(Ok(replies[idx].clone()))
            }
        }

        #[test]
        fn projection_error_maps_to_action_transport() {
            let failure = projection_error(
                0,
                "statedb",
                "column `value` row 0: unsupported reply kind".to_string(),
            );
            assert_eq!(
                failure,
                ScenarioFailure::ActionTransport {
                    action: 0,
                    source: TransportError::Other {
                        message: "redis validation: datasource 'statedb': column `value` row 0: \
                                  unsupported reply kind"
                            .to_string(),
                    },
                }
            );
        }

        #[test]
        fn decode_snapshot_unknown_negative_pttl_without_ttl_bound_is_apparatus() {
            let target = redis_target(RedisType::String);
            let expected = rows_expected(
                Some(&["value"]),
                vec![vec![Expectation::Equals(json!("v"))]],
            );
            let raw = redis::Value::Array(vec![
                redis::Value::SimpleString("string".to_string()),
                redis::Value::BulkString(b"x".to_vec()),
                redis::Value::Int(-3),
            ]);
            let err = decode_snapshot(0, &target, &expected, raw).unwrap_err();
            match err {
                ScenarioFailure::ActionTransport {
                    source: TransportError::Other { message },
                    ..
                } => {
                    assert!(
                        message.contains("statedb"),
                        "must name the datasource: {message}"
                    );
                    assert!(
                        message.contains("-3"),
                        "must name the PTTL detail: {message}"
                    );
                }
                other => panic!("expected ActionTransport, got {other:?}"),
            }
        }

        #[test]
        fn decode_snapshot_unknown_negative_pttl_with_ttl_bound_is_apparatus() {
            let mut target = redis_target(RedisType::String);
            target.ttl = Some(CountBound::AtLeast(1000));
            let expected = rows_expected(
                Some(&["value"]),
                vec![vec![Expectation::Equals(json!("v"))]],
            );
            let raw = redis::Value::Array(vec![
                redis::Value::SimpleString("string".to_string()),
                redis::Value::BulkString(b"x".to_vec()),
                redis::Value::Int(-3),
            ]);
            let err = decode_snapshot(0, &target, &expected, raw).unwrap_err();
            assert!(matches!(err, ScenarioFailure::ActionTransport { .. }));
        }

        #[test]
        fn decode_snapshot_missing_key_is_verdict_data() {
            let target = redis_target(RedisType::String);
            let expected = rows_expected(
                Some(&["value"]),
                vec![vec![Expectation::Equals(json!("v"))]],
            );
            let raw = redis::Value::Array(vec![
                redis::Value::SimpleString("none".to_string()),
                redis::Value::Array(Vec::new()),
                redis::Value::Int(-2),
            ]);
            let snapshot = decode_snapshot(0, &target, &expected, raw).expect("verdict data");
            assert_eq!(snapshot.observed, None);
            assert_eq!(snapshot.ttl, RedisTtlStatus::Missing);
        }

        #[tokio::test(start_paused = true)]
        async fn poll_with_source_unknown_negative_pttl_stops_immediately_without_bound() {
            let calls = Rc::new(Cell::new(0));
            let target = redis_target(RedisType::String);
            let expected = rows_expected(
                Some(&["value"]),
                vec![vec![Expectation::Equals(json!("v"))]],
            );
            let start = tokio::time::Instant::now();
            let result = poll_with_source(
                0,
                &target,
                &expected,
                Some(Duration::from_secs(2)),
                scripted(
                    vec![redis::Value::Array(vec![
                        redis::Value::SimpleString("string".to_string()),
                        redis::Value::BulkString(b"x".to_vec()),
                        redis::Value::Int(-3),
                    ])],
                    calls.clone(),
                ),
            )
            .await;
            assert!(matches!(
                result,
                Err(ScenarioFailure::ActionTransport { .. })
            ));
            assert!(start.elapsed() < Duration::from_secs(1));
            assert_eq!(calls.get(), 1, "the apparatus path stops the poll at once");
        }

        #[tokio::test(start_paused = true)]
        async fn poll_with_source_unknown_negative_pttl_stops_immediately_with_bound() {
            let calls = Rc::new(Cell::new(0));
            let mut target = redis_target(RedisType::String);
            target.ttl = Some(CountBound::AtLeast(1));
            let expected = rows_expected(
                Some(&["value"]),
                vec![vec![Expectation::Equals(json!("v"))]],
            );
            let start = tokio::time::Instant::now();
            let result = poll_with_source(
                0,
                &target,
                &expected,
                Some(Duration::from_secs(2)),
                scripted(
                    vec![redis::Value::Array(vec![
                        redis::Value::SimpleString("string".to_string()),
                        redis::Value::BulkString(b"x".to_vec()),
                        redis::Value::Int(-3),
                    ])],
                    calls.clone(),
                ),
            )
            .await;
            assert!(matches!(
                result,
                Err(ScenarioFailure::ActionTransport { .. })
            ));
            assert!(start.elapsed() < Duration::from_secs(1));
            assert_eq!(calls.get(), 1);
        }

        #[tokio::test(start_paused = true)]
        async fn poll_with_source_first_missing_then_present_passes() {
            let calls = Rc::new(Cell::new(0));
            let target = redis_target(RedisType::String);
            let expected = rows_expected(
                Some(&["value"]),
                vec![vec![Expectation::Equals(json!("v"))]],
            );
            let result = poll_with_source(
                0,
                &target,
                &expected,
                Some(Duration::from_secs(2)),
                scripted(vec![missing_reply(), string_reply("v", -1)], calls.clone()),
            )
            .await;
            assert_eq!(result, Ok(()));
            assert!(calls.get() >= 2, "the poll must re-snapshot");
        }

        #[tokio::test(start_paused = true)]
        async fn poll_with_source_first_present_then_deleted_final_snapshot_fails() {
            let calls = Rc::new(Cell::new(0));
            let target = redis_target(RedisType::String);
            let expected = rows_expected(
                Some(&["value"]),
                vec![vec![Expectation::Equals(json!("v"))]],
            );
            let result = poll_with_source(
                0,
                &target,
                &expected,
                Some(Duration::from_secs(2)),
                scripted(vec![string_reply("v", -1), missing_reply()], calls.clone()),
            )
            .await;
            assert!(matches!(
                result,
                Err(ScenarioFailure::ValidationMismatch { .. })
            ));
            assert!(calls.get() >= 2, "no early settle");
        }

        #[tokio::test(start_paused = true)]
        async fn poll_with_source_transient_wrong_type_then_correct_passes() {
            let calls = Rc::new(Cell::new(0));
            let target = redis_target(RedisType::Hash);
            let expected = rows_expected(
                Some(&["value"]),
                vec![vec![Expectation::Equals(json!("alice"))]],
            );
            let hash_reply = redis::Value::Array(vec![
                redis::Value::SimpleString("hash".to_string()),
                redis::Value::Array(vec![
                    redis::Value::BulkString(b"name".to_vec()),
                    redis::Value::BulkString(b"alice".to_vec()),
                ]),
                redis::Value::Int(-1),
            ]);
            let result = poll_with_source(
                0,
                &target,
                &expected,
                Some(Duration::from_secs(2)),
                scripted(vec![string_reply("v", -1), hash_reply], calls.clone()),
            )
            .await;
            assert_eq!(result, Ok(()));
            assert!(calls.get() >= 2);
        }

        #[tokio::test(start_paused = true)]
        async fn poll_with_source_ttl_passes_early_fails_at_deadline() {
            let calls = Rc::new(Cell::new(0));
            let mut target = redis_target(RedisType::String);
            target.ttl = Some(CountBound::AtLeast(30_000));
            let expected = bound_expected(CountBound::AtLeast(0));
            let result = poll_with_source(
                0,
                &target,
                &expected,
                Some(Duration::from_secs(2)),
                scripted(
                    vec![string_reply("v", 60_000), string_reply("v", 0)],
                    calls.clone(),
                ),
            )
            .await;
            match result {
                Err(ScenarioFailure::ValidationMismatch { detail, .. }) => {
                    assert!(
                        detail.contains("at least 30000"),
                        "bound must render: {detail}"
                    );
                    assert!(
                        detail.contains("remaining 0 ms"),
                        "observed TTL must render: {detail}"
                    );
                }
                other => panic!("expected ValidationMismatch, got {other:?}"),
            }
            assert!(calls.get() >= 2, "TTL never settles early");
        }

        #[tokio::test(start_paused = true)]
        async fn poll_with_source_ceiling_breach_stops_immediately() {
            let calls = Rc::new(Cell::new(0));
            let target = redis_target(RedisType::Set);
            let expected = bound_expected(CountBound::AtMost(1));
            let set_reply = redis::Value::Array(vec![
                redis::Value::SimpleString("set".to_string()),
                redis::Value::Array(vec![
                    redis::Value::BulkString(b"a".to_vec()),
                    redis::Value::BulkString(b"b".to_vec()),
                ]),
                redis::Value::Int(-1),
            ]);
            let start = tokio::time::Instant::now();
            let result = poll_with_source(
                0,
                &target,
                &expected,
                Some(Duration::from_secs(2)),
                scripted(vec![set_reply], calls.clone()),
            )
            .await;
            assert!(matches!(
                result,
                Err(ScenarioFailure::ValidationMismatch { .. })
            ));
            assert!(start.elapsed() < Duration::from_secs(1));
            assert_eq!(calls.get(), 1);
        }

        #[test]
        fn redis_decide_missing_key_is_mismatch() {
            let mut target = redis_target(RedisType::String);
            target.key = "rc-decide".to_string();
            let expected = rows_expected(
                Some(&["value"]),
                vec![vec![Expectation::Equals(json!("v"))]],
            );
            let snapshot = Snapshot {
                observed: None,
                tuples: Vec::new(),
                columns: vec!["value".to_string()],
                ttl: RedisTtlStatus::Missing,
            };
            match decide(0, &target, &expected, &snapshot) {
                Err(ScenarioFailure::ValidationMismatch { detail, .. }) => {
                    assert!(detail.contains("rc-decide"));
                    assert!(detail.contains("string"));
                    assert!(detail.contains("none"));
                }
                other => panic!("expected mismatch, got {other:?}"),
            }
        }

        #[test]
        fn redis_decide_wrong_type_is_mismatch() {
            let target = redis_target(RedisType::Hash);
            let expected = rows_expected(
                Some(&["field", "value"]),
                vec![vec![
                    Expectation::Equals(json!("name")),
                    Expectation::Equals(json!("alice")),
                ]],
            );
            let snapshot = Snapshot {
                observed: Some(RedisType::String),
                tuples: Vec::new(),
                columns: vec!["field".to_string(), "value".to_string()],
                ttl: RedisTtlStatus::Remaining(60_000),
            };
            match decide(0, &target, &expected, &snapshot) {
                Err(ScenarioFailure::ValidationMismatch { detail, .. }) => {
                    assert!(detail.contains("hash"));
                    assert!(detail.contains("string"));
                    assert!(
                        !detail.contains("alice"),
                        "value must not project: {detail}"
                    );
                }
                other => panic!("expected mismatch, got {other:?}"),
            }
        }

        #[test]
        fn redis_decide_type_agreement_rows_pass() {
            let target = redis_target(RedisType::String);
            let expected = RowsExpectation {
                columns: None,
                unordered: false,
                rows: Some(vec![vec![Expectation::Equals(json!("alice"))]]),
                bound: None,
            };
            let snapshot = Snapshot {
                observed: Some(RedisType::String),
                tuples: vec![vec![Value::String("alice".to_string())]],
                columns: vec!["value".to_string()],
                ttl: RedisTtlStatus::Persistent,
            };
            assert_eq!(decide(0, &target, &expected, &snapshot), Ok(()));
        }

        #[test]
        fn redis_decide_count_bound_passes_on_projected_row_count() {
            let target = redis_target(RedisType::Set);
            let expected = bound_expected(CountBound::AtLeast(2));
            let snapshot = Snapshot {
                observed: Some(RedisType::Set),
                tuples: vec![
                    vec![Value::String("a".to_string())],
                    vec![Value::String("b".to_string())],
                    vec![Value::String("c".to_string())],
                ],
                columns: vec!["member".to_string()],
                ttl: RedisTtlStatus::Persistent,
            };
            assert_eq!(decide(0, &target, &expected, &snapshot), Ok(()));
        }

        #[test]
        fn redis_decide_ceiling_breach_is_mismatch() {
            let target = redis_target(RedisType::Set);
            let expected = bound_expected(CountBound::AtMost(1));
            let snapshot = Snapshot {
                observed: Some(RedisType::Set),
                tuples: vec![
                    vec![Value::String("a".to_string())],
                    vec![Value::String("b".to_string())],
                    vec![Value::String("c".to_string())],
                ],
                columns: vec!["member".to_string()],
                ttl: RedisTtlStatus::Persistent,
            };
            match decide(0, &target, &expected, &snapshot) {
                Err(ScenarioFailure::ValidationMismatch { detail, .. }) => {
                    assert!(detail.contains("at most 1"), "bound must render: {detail}");
                    assert!(
                        detail.contains("actual 3 rows"),
                        "count must render: {detail}"
                    );
                }
                other => panic!("expected mismatch, got {other:?}"),
            }
        }

        #[test]
        fn redis_decide_persistent_key_fails_ttl_bound() {
            let mut target = redis_target(RedisType::String);
            target.ttl = Some(CountBound::AtLeast(1000));
            let expected = bound_expected(CountBound::AtLeast(0));
            let snapshot = Snapshot {
                observed: Some(RedisType::String),
                tuples: vec![vec![Value::String("v".to_string())]],
                columns: vec!["value".to_string()],
                ttl: RedisTtlStatus::Persistent,
            };
            match decide(0, &target, &expected, &snapshot) {
                Err(ScenarioFailure::ValidationMismatch { detail, .. }) => {
                    assert!(detail.contains("at least 1000"));
                    assert!(detail.contains("persistent"));
                }
                other => panic!("expected mismatch, got {other:?}"),
            }
        }

        #[test]
        fn redis_decide_zero_remaining_ttl_bounds() {
            let snapshot = Snapshot {
                observed: Some(RedisType::String),
                tuples: vec![vec![Value::String("v".to_string())]],
                columns: vec!["value".to_string()],
                ttl: RedisTtlStatus::Remaining(0),
            };
            let expected = bound_expected(CountBound::AtLeast(0));
            for (ttl, should_pass) in [
                (CountBound::AtMost(1), true),
                (CountBound::AtLeast(1), false),
                (CountBound::Range(1, 60_000), false),
            ] {
                let mut target = redis_target(RedisType::String);
                target.ttl = Some(ttl);
                let result = decide(0, &target, &expected, &snapshot);
                assert_eq!(
                    result.is_ok(),
                    should_pass,
                    "unexpected verdict for {target:?}: {result:?}"
                );
            }
        }

        #[test]
        fn redis_mismatch_detail_elides_values_and_db_url() {
            let mut target = redis_target(RedisType::String);
            target.key = "rc-detail".to_string();
            let expected = RowsExpectation {
                columns: None,
                unordered: false,
                rows: Some(vec![vec![Expectation::Equals(json!("expected-secret"))]]),
                bound: None,
            };
            let snapshot = Snapshot {
                observed: Some(RedisType::String),
                tuples: vec![vec![Value::String("actual-secret".to_string())]],
                columns: vec!["value".to_string()],
                ttl: RedisTtlStatus::Persistent,
            };
            match decide(0, &target, &expected, &snapshot) {
                Err(ScenarioFailure::ValidationMismatch { detail, .. }) => {
                    assert!(detail.contains("rc-detail"));
                    assert!(detail.contains("expected 1 rows"));
                    assert!(detail.contains("actual 1 rows"));
                    assert!(detail.contains("value"));
                    assert!(!detail.contains("actual-secret"));
                    assert!(!detail.contains("expected-secret"));
                }
                other => panic!("expected mismatch, got {other:?}"),
            }
        }

        #[test]
        fn redis_secret_hash_fields_and_set_members_never_reach_diagnostics() {
            let hash_target = redis_target(RedisType::Hash);
            let mut hash_target = hash_target;
            hash_target.key = "rc-secret".to_string();
            let hash_expected = rows_expected(
                Some(&["field", "value"]),
                vec![vec![
                    Expectation::Equals(json!("EXPECTED_FIELD")),
                    Expectation::Equals(json!("EXPECTED_VALUE")),
                ]],
            );
            let hash_snapshot = Snapshot {
                observed: Some(RedisType::Hash),
                tuples: vec![vec![
                    Value::String("SENTINEL_FIELD".to_string()),
                    Value::String("SENTINEL_VALUE".to_string()),
                ]],
                columns: vec!["field".to_string(), "value".to_string()],
                ttl: RedisTtlStatus::Persistent,
            };
            let debug = format!("{hash_snapshot:?}");
            assert!(!debug.contains("SENTINEL_FIELD"));
            assert!(!debug.contains("SENTINEL_VALUE"));
            match decide(0, &hash_target, &hash_expected, &hash_snapshot) {
                Err(ScenarioFailure::ValidationMismatch { detail, .. }) => {
                    assert!(detail.contains("statedb"));
                    assert!(detail.contains("rc-secret"));
                    assert!(detail.contains("hash"));
                    assert!(detail.contains("expected 1 rows"));
                    assert!(detail.contains("actual 1 rows"));
                    assert!(detail.contains("field"));
                    assert!(detail.contains("value"));
                    assert!(!detail.contains("SENTINEL_FIELD"));
                    assert!(!detail.contains("SENTINEL_VALUE"));
                    assert!(!detail.contains("EXPECTED_FIELD"));
                    assert!(!detail.contains("EXPECTED_VALUE"));
                }
                other => panic!("expected mismatch, got {other:?}"),
            }

            let mut set_target = redis_target(RedisType::Set);
            set_target.key = "rc-secret".to_string();
            let set_expected = rows_expected(
                Some(&["member"]),
                vec![vec![Expectation::Equals(json!("EXPECTED_MEMBER"))]],
            );
            let set_snapshot = Snapshot {
                observed: Some(RedisType::Set),
                tuples: vec![vec![Value::String("SENTINEL_MEMBER".to_string())]],
                columns: vec!["member".to_string()],
                ttl: RedisTtlStatus::Persistent,
            };
            assert!(!format!("{set_snapshot:?}").contains("SENTINEL_MEMBER"));
            match decide(0, &set_target, &set_expected, &set_snapshot) {
                Err(ScenarioFailure::ValidationMismatch { detail, .. }) => {
                    assert!(detail.contains("statedb"));
                    assert!(detail.contains("rc-secret"));
                    assert!(detail.contains("set"));
                    assert!(detail.contains("expected 1 rows"));
                    assert!(detail.contains("actual 1 rows"));
                    assert!(detail.contains("member"));
                    assert!(!detail.contains("SENTINEL_MEMBER"));
                    assert!(!detail.contains("EXPECTED_MEMBER"));
                }
                other => panic!("expected mismatch, got {other:?}"),
            }
        }

        #[test]
        fn redis_ttl_mismatch_detail_includes_observed_status() {
            let mut target = redis_target(RedisType::String);
            target.ttl = Some(CountBound::AtLeast(6000));
            let expected = bound_expected(CountBound::AtLeast(0));
            for (ttl, needle) in [
                (RedisTtlStatus::Persistent, "persistent"),
                (RedisTtlStatus::Remaining(5_000), "5000"),
            ] {
                let snapshot = Snapshot {
                    observed: Some(RedisType::String),
                    tuples: vec![vec![Value::String("v".to_string())]],
                    columns: vec!["value".to_string()],
                    ttl,
                };
                match decide(0, &target, &expected, &snapshot) {
                    Err(ScenarioFailure::ValidationMismatch { detail, .. }) => {
                        assert!(detail.contains("at least 6000"), "bound: {detail}");
                        assert!(detail.contains(needle), "observed `{needle}`: {detail}");
                    }
                    other => panic!("expected mismatch, got {other:?}"),
                }
            }
        }

        #[tokio::test]
        async fn redis_validate_no_catalog_fails_closed() {
            let doc = ScenarioDocument {
                source_path: std::path::PathBuf::new(),
                route_source: RouteSource::RouteFiles(vec![std::path::PathBuf::from(
                    "routes.yaml",
                )]),
                scenario: vec![ScenarioAction::Validate {
                    target: ScenarioTarget::Redis(redis_target(RedisType::String)),
                    expectation: ValidateExpectation::Rows(bound_expected(CountBound::AtLeast(1))),
                    deadline: None,
                    elapsed_at_least: None,
                }],
                partners: None,
                env: None,
                env_passthrough: None,
                profile: None,
                send_deadline: None,
                inbound: None,
                logs: None,
            };
            let router = PartnerRouter::new(BTreeMap::new());
            let mut vars = ScenarioVars::new();
            let failure = run_scenario(&doc, &router, &mut vars)
                .await
                .expect_err("no catalog must fail closed");
            match failure {
                ScenarioFailure::ActionTransport {
                    action: 0,
                    source: TransportError::Other { message },
                    ..
                } => assert!(message.contains("no datasource catalog"), "{message}"),
                other => panic!("expected ActionTransport, got {other:?}"),
            }
        }

        fn config_with_url(db_url: &str) -> camel_api::datasource::DatasourceConfig {
            camel_api::datasource::DatasourceConfig {
                db_url: db_url.to_string(),
                provider: None,
                max_connections: None,
                min_connections: None,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                ssl_mode: None,
                ssl_root_cert: None,
                ssl_cert: None,
                ssl_key: None,
                extra: HashMap::new(),
            }
        }

        use camel_api::datasource::{
            CheckFuture, CreatePoolFuture, DatasourceCatalog, DatasourceConfig, DatasourceHandle,
            PoolFactory,
        };
        use camel_api::error::CamelError;
        use camel_api::lifecycle::HealthStatus;
        use camel_core::datasource::RuntimeDatasourceCatalog;

        #[tokio::test]
        async fn redis_unknown_datasource_names_label() {
            let mut configs = HashMap::new();
            configs.insert(
                "other".to_string(),
                config_with_url("redis://localhost:6379"),
            );
            let catalog: Arc<dyn DatasourceCatalog> =
                Arc::new(RuntimeDatasourceCatalog::new(configs));
            let mut target = redis_target(RedisType::String);
            target.datasource = "missing".to_string();
            let expected = bound_expected(CountBound::AtLeast(1));
            let failure = redis_validate_action(0, &target, &expected, None, Some(&catalog))
                .await
                .expect_err("unknown datasource");
            match failure {
                ScenarioFailure::ActionTransport {
                    source: TransportError::Other { message },
                    ..
                } => assert_eq!(message, "redis validation: unknown datasource 'missing'"),
                other => panic!("expected ActionTransport, got {other:?}"),
            }
        }

        const SENTINEL_URL: &str = "redis://user:s3cr3t@sentinel.invalid:6379/0";

        struct StubRedisFactory;

        impl PoolFactory for StubRedisFactory {
            fn create<'a>(&'a self, _config: &'a DatasourceConfig) -> CreatePoolFuture<'a> {
                Box::pin(async {
                    Err(CamelError::ProcessorError(format!(
                        "cannot open {SENTINEL_URL}"
                    )))
                })
            }

            fn check<'a>(&'a self, _handle: &'a DatasourceHandle) -> CheckFuture<'a> {
                Box::pin(async { HealthStatus::Healthy })
            }

            fn supported_schemes(&self) -> &[&str] {
                &["redis"]
            }

            fn name(&self) -> &'static str {
                "redis"
            }
        }

        #[tokio::test]
        async fn redis_resolution_pool_failure_redacts_url() {
            let mut configs = HashMap::new();
            configs.insert("statedb".to_string(), config_with_url(SENTINEL_URL));
            let catalog = RuntimeDatasourceCatalog::new(configs);
            catalog
                .register_factory("redis", Arc::new(StubRedisFactory))
                .unwrap();
            let catalog: Arc<dyn DatasourceCatalog> = Arc::new(catalog);
            let target = redis_target(RedisType::String);
            let expected = bound_expected(CountBound::AtLeast(1));
            let failure = redis_validate_action(0, &target, &expected, None, Some(&catalog))
                .await
                .expect_err("pool failure");
            match failure {
                ScenarioFailure::ActionTransport {
                    source: TransportError::Other { message },
                    ..
                } => {
                    assert!(message.contains("redis validation"), "{message}");
                    assert!(message.contains("statedb"), "{message}");
                    assert!(message.contains("[REDACTED]"), "{message}");
                    assert!(!message.contains(SENTINEL_URL), "{message}");
                }
                other => panic!("expected ActionTransport, got {other:?}"),
            }
        }

        struct StubWrongTypeFactory;

        impl PoolFactory for StubWrongTypeFactory {
            fn create<'a>(&'a self, _config: &'a DatasourceConfig) -> CreatePoolFuture<'a> {
                Box::pin(async { Ok(Arc::new(()) as Arc<dyn std::any::Any + Send + Sync>) })
            }

            fn check<'a>(&'a self, _handle: &'a DatasourceHandle) -> CheckFuture<'a> {
                Box::pin(async { HealthStatus::Healthy })
            }

            fn supported_schemes(&self) -> &[&str] {
                &["redis"]
            }

            fn name(&self) -> &'static str {
                "redis"
            }
        }

        #[tokio::test]
        async fn redis_downcast_failure_keeps_driver_detail() {
            let mut configs = HashMap::new();
            configs.insert(
                "statedb".to_string(),
                config_with_url("redis://localhost:6379/0"),
            );
            let catalog = RuntimeDatasourceCatalog::new(configs);
            catalog
                .register_factory("redis", Arc::new(StubWrongTypeFactory))
                .unwrap();
            let catalog: Arc<dyn DatasourceCatalog> = Arc::new(catalog);
            let target = redis_target(RedisType::String);
            let expected = bound_expected(CountBound::AtLeast(1));
            let failure = redis_validate_action(0, &target, &expected, None, Some(&catalog))
                .await
                .expect_err("downcast failure");
            match failure {
                ScenarioFailure::ActionTransport {
                    source: TransportError::Other { message },
                    ..
                } => {
                    assert!(message.contains("redis validation"), "{message}");
                    assert!(message.contains("statedb"), "{message}");
                    assert!(message.contains("failed to downcast handle"), "{message}");
                    assert!(!message.contains("redis://localhost:6379/0"), "{message}");
                }
                other => panic!("expected ActionTransport, got {other:?}"),
            }
        }
    }

    // -----------------------------------------------------------------
    // Production acquisition protocol: the REAL eval_raw executes one
    // EVAL over a loopback fake RESP server. Feature `redis`, tokio only.
    // -----------------------------------------------------------------
    #[cfg(feature = "redis")]
    mod protocol {
        use futures::FutureExt;
        use std::panic::AssertUnwindSafe;
        use std::time::Duration;

        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        use tokio::net::TcpListener;

        use super::super::{RedisType, eval_raw, snapshot_script};

        async fn read_line<R: AsyncBufReadExt + Unpin>(
            reader: &mut R,
            buf: &mut Vec<u8>,
        ) -> std::io::Result<usize> {
            buf.clear();
            reader.read_until(b'\n', buf).await
        }

        async fn read_command<R: AsyncBufReadExt + Unpin>(
            reader: &mut R,
        ) -> std::io::Result<Option<Vec<Vec<u8>>>> {
            let mut line = Vec::new();
            if read_line(reader, &mut line).await? == 0 {
                return Ok(None);
            }
            let count: usize = std::str::from_utf8(&line[1..line.len() - 2])
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            let mut args = Vec::with_capacity(count);
            for _ in 0..count {
                read_line(reader, &mut line).await?;
                let len: usize = std::str::from_utf8(&line[1..line.len() - 2])
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                let mut buf = vec![0u8; len];
                reader.read_exact(&mut buf).await?;
                let mut crlf = [0u8; 2];
                reader.read_exact(&mut crlf).await?;
                args.push(buf);
            }
            Ok(Some(args))
        }

        #[tokio::test]
        async fn eval_raw_sends_exactly_one_eval_command() {
            let listener =
                tokio::time::timeout(Duration::from_secs(1), TcpListener::bind("127.0.0.1:0"))
                    .await
                    .expect("bind budget")
                    .expect("bind");
            let addr = listener.local_addr().expect("addr");
            let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<String>>(8);
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.expect("accept");
                let (read_half, mut write_half) = stream.into_split();
                let mut reader = BufReader::new(read_half);
                while let Some(args) = read_command(&mut reader).await.expect("read command") {
                    if args.first().is_some_and(|a| a.as_slice() == b"CLIENT") {
                        write_half
                            .write_all(b"+OK\r\n")
                            .await
                            .expect("handshake ok");
                        continue;
                    }
                    let recorded: Vec<String> = args
                        .iter()
                        .map(|a| String::from_utf8_lossy(a).into_owned())
                        .collect();
                    let _ = tx.send(recorded).await;
                    write_half
                        .write_all(b"*3\r\n+string\r\n$1\r\nv\r\n:-1\r\n")
                        .await
                        .expect("snapshot reply");
                }
            });

            // Catch assertion panics as well as timeouts so the server is
            // always aborted and joined before the original failure reports.
            let result = tokio::time::timeout(
                Duration::from_secs(10),
                AssertUnwindSafe(async {
                    let client = redis::Client::open(format!("redis://{addr}")).expect("client");
                    let conn = client
                        .get_multiplexed_async_connection()
                        .await
                        .expect("connection");

                    let raw = tokio::time::timeout(
                        Duration::from_secs(5),
                        eval_raw(
                            &conn,
                            "rc-proto",
                            snapshot_script(RedisType::String),
                            0,
                            "statedb",
                            "redis://127.0.0.1:1",
                        ),
                    )
                    .await
                    .expect("eval_raw must complete")
                    .expect("eval_raw must succeed");
                    assert_eq!(
                        raw,
                        redis::Value::Array(vec![
                            redis::Value::SimpleString("string".to_string()),
                            redis::Value::BulkString(b"v".to_vec()),
                            redis::Value::Int(-1),
                        ])
                    );

                    let first = rx.recv().await.expect("one post-handshake command");
                    assert_eq!(
                        first,
                        vec![
                            "EVAL".to_string(),
                            snapshot_script(RedisType::String).to_string(),
                            "1".to_string(),
                            "rc-proto".to_string(),
                        ],
                        "the one command must be the fixed EVAL"
                    );
                    assert!(first[1].contains("TYPE"));
                    assert!(first[1].contains("GET"));
                    assert!(first[1].contains("PTTL"));

                    assert!(
                        tokio::time::timeout(Duration::from_millis(200), rx.recv())
                            .await
                            .is_err(),
                        "no second post-handshake command may arrive"
                    );

                    drop(conn);
                })
                .catch_unwind(),
            )
            .await;
            server.abort();
            let _ = tokio::time::timeout(Duration::from_secs(1), server).await;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(panic)) => std::panic::resume_unwind(panic),
                Err(_) => panic!("Redis protocol setup and assertions exceeded 10s"),
            }
        }
    }

    // -----------------------------------------------------------------
    // Feature-off twin.
    // -----------------------------------------------------------------
    #[cfg(not(feature = "redis"))]
    mod twin {
        use camel_matchers::{CountBound, RowsExpectation};

        use super::super::redis_validate_action;
        use crate::document::{RedisTarget, RedisType};
        use crate::runner::ScenarioFailure;

        #[tokio::test]
        async fn redis_validate_feature_off_names_gate() {
            let target = RedisTarget {
                datasource: "statedb".to_string(),
                key: "k".to_string(),
                r#type: RedisType::String,
                ttl: None,
            };
            let expected = RowsExpectation {
                columns: None,
                unordered: false,
                rows: None,
                bound: Some(CountBound::AtLeast(1)),
            };
            let result = redis_validate_action(0, &target, &expected, None, None).await;
            assert_eq!(
                result,
                Err(ScenarioFailure::ValidationMismatch {
                    action: 0,
                    detail: "redis validation requires the `redis` feature".to_string(),
                })
            );
        }
    }
}
#[cfg(all(test, feature = "redis", feature = "redis-live"))]
#[path = "redis_validate_live.rs"]
mod live;
