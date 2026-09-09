//! SQL row-shape verification for the `validate` action's `sql` target
//! (bd rc-25lup.2): the datasource pool resolution, the per-snapshot
//! row-to-tuple mapping with its fail-closed type law, the by-name
//! projection, the poll lattice for a live datasource, and the
//! cell-free mismatch renderer. Split out of the runner core like
//! [`super::partner_validate`] so the sql grammar validation and the
//! row-shape assertion stay separately navigable; the runner
//! dispatches the `sql` target here.
//!
//! Papal deviation (e_opus, bd rc-25lup.2, 2026-09-09): the partner
//! poll settles early because recorded arrivals only add, but SQL row
//! sets are NON-monotone — a concurrent DELETE shrinks them — so this
//! poll never settles early and the final snapshot at the deadline
//! decides.

use std::sync::Arc;
use std::time::Duration;

use camel_matchers::RowsExpectation;

use super::ScenarioFailure;
use crate::document::SqlTarget;

/// The poll interval of a sql validate with a deadline (feature
/// `sql`): a fresh row snapshot every 100 ms until the deadline
/// passes. The snapshot itself is a bounded pool query; the sleep
/// between snapshots means the poll never busy-waits.
#[cfg(feature = "sql")]
const SQL_VALIDATE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Asserts the rows-expectation against the datasource's live state
/// (feature `sql`): one immediate snapshot decides without a
/// deadline; with one, the poll re-snapshots at
/// [`SQL_VALIDATE_POLL_INTERVAL`] and the FINAL snapshot at expiry
/// decides — never an early settle, because SQL state is
/// non-monotone (a DELETE shrinks a row set that an earlier snapshot
/// already matched).
///
/// Every snapshot failure is an apparatus failure: the pool
/// resolution, driver, and decode errors carry the datasource NAME
/// only — the `db_url` passes through
/// [`crate::sql_action::sanitize_db_error`] (ADR-0051) — while the
/// assertion outcome is a verdict-class
/// [`ScenarioFailure::ValidationMismatch`] whose detail renders the
/// expectation shape and the actual row count, never cell values or
/// query text.
#[cfg(feature = "sql")]
pub(crate) async fn sql_validate_action(
    index: usize,
    target: &SqlTarget,
    expected: &RowsExpectation,
    deadline: Option<Duration>,
    catalog: Option<&Arc<dyn camel_api::datasource::DatasourceCatalog>>,
) -> Result<(), ScenarioFailure> {
    // Apparatus class (exit 2), mirroring the `sql:` action arm: a
    // validate sql target without a catalog means the boot-owning
    // caller never passed the cascade's datasources — the scenario
    // cannot even reach its declared subject.
    let Some(catalog) = catalog else {
        return Err(ScenarioFailure::ActionTransport {
            action: index,
            source: crate::adapters::TransportError::Other {
                message: "sql validation: no datasource catalog is available; the \
                          boot-owning caller must pass the cascade's catalog"
                    .to_string(),
            },
        });
    };
    // Pool resolution, copied from `execute_sql_prepare`: the
    // datasource NAME obeys the identifier law (never a URL), every
    // driver error string is sanitized against the config's `db_url`
    // before it reaches the failure, and the handle must be the
    // sqlx pool the catalog provisions.
    let name = &target.datasource;
    let Some(config) = catalog.get_config(name) else {
        return Err(ScenarioFailure::ActionTransport {
            action: index,
            source: crate::adapters::TransportError::Other {
                message: format!("sql validation: unknown datasource '{name}'"),
            },
        });
    };
    let db_url = config.db_url.clone();
    // One sanitizing wrapper for every driver-error string below: the
    // datasource NAME renders, its URL never (ADR-0051).
    let sanitize = |err_text: String| crate::sql_action::sanitize_db_error(&err_text, &db_url);
    let handle = catalog
        .get_pool(name)
        .await
        .map_err(|e| apparatus(index, name, sanitize(e.to_string())))?;
    let pool = handle
        .downcast::<sqlx::AnyPool>()
        .map_err(|e| apparatus(index, name, sanitize(e.to_string())))?;
    match deadline {
        // No deadline: one immediate snapshot decides for every
        // shape, exactly like the partner no-deadline read.
        None => {
            let snapshot = snapshot(index, target, expected, &pool, &db_url).await?;
            decide(index, name, expected, &snapshot)
        }
        // Poll: the only mid-window exit is an upper-bound ceiling
        // breach (unrecoverable even under deletion, since the count
        // observed above the ceiling already falsifies the claim at
        // this instant); every other shape waits out the window and
        // the final snapshot at expiry decides.
        Some(deadline) => {
            let until = tokio::time::Instant::now() + deadline;
            loop {
                let snapshot = snapshot(index, target, expected, &pool, &db_url).await?;
                if let Some(bound) = expected.bound.as_ref()
                    && camel_matchers::above_ceiling(bound, snapshot.tuples.len())
                {
                    return Err(mismatch(index, name, expected, &snapshot));
                }
                let now = tokio::time::Instant::now();
                if now >= until {
                    return decide(index, name, expected, &snapshot);
                }
                tokio::time::sleep((until - now).min(SQL_VALIDATE_POLL_INTERVAL)).await;
            }
        }
    }
}

/// One driver-side failure of the apparatus (feature `sql`): the
/// transport class carrying the datasource NAME and the
/// pre-sanitized error text — the `db_url` never renders
/// (ADR-0051).
#[cfg(feature = "sql")]
fn apparatus(index: usize, name: &str, text: String) -> ScenarioFailure {
    ScenarioFailure::ActionTransport {
        action: index,
        source: crate::adapters::TransportError::Other {
            message: format!("sql validation: datasource '{name}': {text}"),
        },
    }
}

/// The feature-off twin (the partner no-http precedent): the document
/// parser rejects a sql validate target without the feature, so only
/// a directly-constructed document reaches this arm — it fails with
/// the verdict-class mismatch naming the gate instead of passing
/// silently.
#[cfg(not(feature = "sql"))]
pub(crate) async fn sql_validate_action(
    index: usize,
    target: &SqlTarget,
    expected: &RowsExpectation,
    deadline: Option<Duration>,
    catalog: Option<&Arc<dyn camel_api::datasource::DatasourceCatalog>>,
) -> Result<(), ScenarioFailure> {
    let _ = (target, expected, deadline, catalog);
    Err(ScenarioFailure::ValidationMismatch {
        action: index,
        detail: "sql validation requires the `sql` feature".to_string(),
    })
}

/// One snapshot of the query's row set (feature `sql`): the projected
/// tuples plus the column names the projection ran against. Driver
/// failures map to the apparatus class with the sanitized error; a
/// row whose column decodes through no arm is an explicit failure
/// naming the column and its sqlx type info (fail-closed: never a
/// silent null, never a sentinel a wildcard could match away).
#[cfg(feature = "sql")]
struct Snapshot {
    /// The result set's column names in query order, from
    /// `row.columns()`; empty when the query returned no rows.
    columns: Vec<String>,
    /// The projected rows: every row narrowed to the declared
    /// projection by name (declared order), or kept in query order
    /// when no projection is declared.
    tuples: Vec<Vec<camel_api::Value>>,
}

/// Fetches the query's rows and projects them per the expectation
/// (feature `sql`). The declared projection resolves by NAME against
/// each snapshot's own column list, so a reordered SELECT satisfies
/// the same declaration; a name the result does not carry fails
/// closed instead of silently dropping the column.
#[cfg(feature = "sql")]
async fn snapshot(
    index: usize,
    target: &SqlTarget,
    expected: &RowsExpectation,
    pool: &sqlx::AnyPool,
    db_url: &str,
) -> Result<Snapshot, ScenarioFailure> {
    let rows = sqlx::query(&target.query)
        .fetch_all(pool)
        .await
        .map_err(|e| {
            apparatus(
                index,
                &target.datasource,
                crate::sql_action::sanitize_db_error(&e.to_string(), db_url),
            )
        })?;
    // An empty result set carries no column names to resolve a
    // declared projection against: it returns zero tuples here and
    // the DECIDE step (rows_match's length check, or bound_holds)
    // renders the verdict — an empty snapshot must never abort the
    // poll window on a projection error or misreport the
    // no-deadline case as an unknown column.
    if rows.is_empty() {
        return Ok(Snapshot {
            columns: Vec::new(),
            tuples: Vec::new(),
        });
    }
    let columns: Vec<String> = sqlx::Row::columns(&rows[0])
        .iter()
        .map(|column| sqlx::Column::name(column).to_string())
        .collect();
    // The projection indices, resolved once per snapshot by name: a
    // declared name absent from the result's columns fails closed
    // before any row matching runs.
    let projection: Result<Vec<usize>, ScenarioFailure> = match &expected.columns {
        Some(declared) => declared
            .iter()
            .map(|want| {
                columns.iter().position(|have| have == want).ok_or_else(|| {
                    ScenarioFailure::ValidationMismatch {
                        action: index,
                        detail: format!("sql validation: unknown projection column `{want}`"),
                    }
                })
            })
            .collect(),
        None => Ok((0..columns.len()).collect()),
    };
    let projection = projection?;
    let mut tuples = Vec::with_capacity(rows.len());
    for row in &rows {
        let tuple = any_row_to_tuple(row, &columns).map_err(|detail| {
            ScenarioFailure::ValidationMismatch {
                action: index,
                detail,
            }
        })?;
        tuples.push(projection.iter().map(|&i| tuple[i].clone()).collect());
    }
    Ok(Snapshot { columns, tuples })
}

/// Decides the FINAL snapshot against the expectation's one shape
/// (feature `sql`): concrete row patterns through
/// [`camel_matchers::rows_match`] (positional when ordered, a perfect
/// matching when unordered), a row-count bound through
/// [`camel_matchers::bound_holds`].
#[cfg(feature = "sql")]
fn decide(
    index: usize,
    datasource: &str,
    expected: &RowsExpectation,
    snapshot: &Snapshot,
) -> Result<(), ScenarioFailure> {
    let passed = match (&expected.rows, &expected.bound) {
        (Some(rows), _) => camel_matchers::rows_match(rows, &snapshot.tuples, expected.unordered),
        (None, Some(bound)) => camel_matchers::bound_holds(bound, snapshot.tuples.len()),
        // The parser guarantees exactly one shape (`rows` XOR
        // `bound`); an empty expectation matches nothing here, so a
        // hand-built one fails closed instead of passing silently.
        (None, None) => false,
    };
    if passed {
        Ok(())
    } else {
        Err(mismatch(index, datasource, expected, snapshot))
    }
}

/// The mismatch detail of a failed rows assertion (feature `sql`):
/// the datasource name, the expectation shape in its own grammar
/// ([`camel_matchers::render_bound`] for a bound, the row count and
/// order rule for concrete rows), the actual row count, and the
/// column names — never an actual cell value (rows may hold payload
/// bytes), never the query text, never the `db_url` (ADR-0051).
#[cfg(feature = "sql")]
fn mismatch(
    index: usize,
    datasource: &str,
    expected: &RowsExpectation,
    snapshot: &Snapshot,
) -> ScenarioFailure {
    // The projection the assertion ran against: the declared names
    // when one narrowed the rows, the query-order names otherwise.
    let columns: Vec<String> = match &expected.columns {
        Some(declared) => declared.clone(),
        None => snapshot.columns.clone(),
    };
    ScenarioFailure::ValidationMismatch {
        action: index,
        detail: sql_mismatch_detail(datasource, expected, snapshot.tuples.len(), &columns),
    }
}

/// Renders a failed sql rows assertion: "sql {datasource},
/// {render_bound(bound) | expected N rows (ordered|unordered)},
/// actual {count} rows, columns: [names]".
#[cfg(feature = "sql")]
fn sql_mismatch_detail(
    datasource: &str,
    expected: &RowsExpectation,
    actual_count: usize,
    columns: &[String],
) -> String {
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
    format!(
        "sql {datasource}, {shape}, actual {actual_count} rows, columns: [{}]",
        columns.join(", ")
    )
}

/// Maps one sqlx `AnyRow` to a matcher tuple, one cell per column in
/// `names` order (feature `sql`): NULL stays `null`; integers and
/// floats map to JSON numbers; booleans map to booleans; text maps to
/// a JSON string; and a blob parses as JSON first, falling back to a
/// lossy-UTF-8 string (the `reply_bytes_value` precedent — a text
/// body holding JSON is observed as the structured value the matcher
/// verbs expect).
///
/// A column whose value decodes through NO arm is an explicit
/// validation failure naming the column and its sqlx type info:
/// fail-closed, never a silent null and never a sentinel a wildcard
/// could match away. This is a documented limitation of the any-tier
/// mapping ahead of surrealdb parity (bd rc-25lup.2).
#[cfg(feature = "sql")]
pub(crate) fn any_row_to_tuple(
    row: &sqlx::any::AnyRow,
    names: &[String],
) -> Result<Vec<camel_api::Value>, String> {
    use sqlx::Row;
    use sqlx::ValueRef as _;

    let mut tuple = Vec::with_capacity(names.len());
    for (i, name) in names.iter().enumerate() {
        // The raw value read: NULL detection needs the value itself,
        // not a decode attempt (an i64 decode of NULL errors the same
        // way an unsupported type does).
        let raw = row
            .try_get_raw(i)
            .map_err(|e| format!("sql validation: column `{name}`: {e}"))?;
        if raw.is_null() {
            tuple.push(camel_api::Value::Null);
            continue;
        }
        let type_info = raw.type_info();
        if let Ok(v) = row.try_get::<i64, _>(i) {
            tuple.push(camel_api::Value::from(v));
        } else if let Ok(v) = row.try_get::<f64, _>(i) {
            // A non-finite SQL double has no JSON number; it reads as
            // `null` (serde_json's own from_f64 law).
            tuple.push(camel_api::Value::from(v));
        } else if let Ok(v) = row.try_get::<bool, _>(i) {
            tuple.push(camel_api::Value::Bool(v));
        } else if let Ok(v) = row.try_get::<String, _>(i) {
            tuple.push(camel_api::Value::String(v));
        } else if let Ok(v) = row.try_get::<Vec<u8>, _>(i) {
            tuple.push(super::reply_bytes_value(&v));
        } else {
            return Err(format!(
                "sql validation: column `{name}` has unsupported type {type_info}; \
                 no any-tier decode arm matches (fail-closed)"
            ));
        }
    }
    Ok(tuple)
}
