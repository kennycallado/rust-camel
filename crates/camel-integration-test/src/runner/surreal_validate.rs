//! Surreal record projection for the `validate` action's `surreal`
//! target (surreal-state-tier tasks 2.3 and 2.4): the datasource
//! resolution through the catalog, the fail-closed record-to-tuple
//! mapping with its SurrealQL type law, the poll lattice for a live
//! datasource, and the cell-free mismatch renderer. Split out of the
//! runner core like [`super::sql_validate`] so the surreal executor
//! stays separately navigable; the runner dispatches the `surreal`
//! target here.
//!
//! The executor's shape is the sql validate seam's
//! ([`super::sql_validate`]): a [`RowsExpectation`] in, a
//! [`ScenarioFailure`] out, and the verdict decided through the same
//! `camel_matchers` expectation matchers the sql target uses. The
//! poll never settles early: record sets are not monotone — a
//! DELETE shrinks them — so the final snapshot at the deadline
//! decides, exactly like the sql twin.

use std::sync::Arc;
use std::time::Duration;

use camel_matchers::RowsExpectation;

use super::ScenarioFailure;
use crate::document::SurrealTarget;

/// The poll interval of a surreal validate with a deadline (feature
/// `surreal`): a fresh record snapshot every 100 ms until the
/// deadline passes, the sql validate's interval. The snapshot itself
/// is a bounded query round-trip; the sleep between snapshots means
/// the poll never busy-waits.
#[cfg(feature = "surreal")]
const SURREAL_VALIDATE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Asserts the rows-expectation against the datasource's live record
/// set (feature `surreal`): one immediate snapshot decides without a
/// deadline; with one, the poll re-snapshots at
/// [`SURREAL_VALIDATE_POLL_INTERVAL`] and the FINAL snapshot at
/// expiry decides — never an early settle, because record sets are
/// non-monotone (a DELETE shrinks a record set that an earlier
/// snapshot already matched). Every apparatus failure is an
/// apparatus-class [`ScenarioFailure`]: the
/// datasource resolution, driver, and decode errors carry the
/// datasource NAME only — the `db_url` passes through
/// [`crate::steering::sanitize_db_error`] (ADR-0051) — while the
/// assertion outcome is a verdict-class
/// [`ScenarioFailure::ValidationMismatch`] whose detail renders the
/// expectation shape and the actual row count, never cell values or
/// query text.
#[cfg(feature = "surreal")]
pub(crate) async fn surreal_validate_action(
    index: usize,
    target: &SurrealTarget,
    expected: &RowsExpectation,
    deadline: Option<Duration>,
    catalog: Option<&Arc<dyn camel_api::datasource::DatasourceCatalog>>,
) -> Result<(), ScenarioFailure> {
    // Apparatus class (exit 2), mirroring the `surreal:` action arm
    // and the sql validate arm: a validate surreal target without a
    // catalog means the boot-owning caller never passed the cascade's
    // datasources — the scenario cannot even reach its declared
    // subject.
    let Some(catalog) = catalog else {
        return Err(ScenarioFailure::ActionTransport {
            action: index,
            source: crate::adapters::TransportError::Other {
                message: "surreal validation: no datasource catalog is available; the \
                          boot-owning caller must pass the cascade's catalog"
                    .to_string(),
            },
        });
    };
    // Datasource resolution through the steering seam: the datasource
    // NAME obeys the identifier law (never a URL), every driver error
    // string is sanitized against the config's `db_url` before it
    // reaches the failure (ADR-0051), and the handle must be the
    // `Surreal<Any>` client the catalog provisions (the `check`
    // precedent in the surrealdb pool factory). The resolver's message
    // is already complete, so it maps straight into the transport
    // class — never through `apparatus`, which would double the
    // `surreal validation: datasource` prefix.
    let name = &target.datasource;
    let (client, db_url) = crate::steering::resolve_datasource::<
        surrealdb::Surreal<surrealdb::engine::any::Any>,
    >(catalog, name, "surreal validation")
    .await
    .map_err(|message| ScenarioFailure::ActionTransport {
        action: index,
        source: crate::adapters::TransportError::Other { message },
    })?;
    // The shared poll driver owns the deadline discipline. The only
    // mid-window exit is an upper-bound ceiling breach (unrecoverable
    // even under deletion, since the count observed above the ceiling
    // already falsifies the claim at this instant); every other shape
    // waits out the window and the final snapshot at expiry decides.
    // Record sets are NOT monotone, so there is never an early settle.
    let client = &client;
    let db_url = &db_url;
    super::poll::poll_until(
        deadline,
        SURREAL_VALIDATE_POLL_INTERVAL,
        move || async move { snapshot(index, name, target, expected, client, db_url).await },
        move |snapshot: &Snapshot| {
            expected
                .bound
                .as_ref()
                .is_some_and(|bound| camel_matchers::above_ceiling(bound, snapshot.tuples.len()))
                .then(|| Err(mismatch(index, name, expected, snapshot)))
        },
        move |snapshot: &Snapshot| decide(index, name, expected, snapshot),
    )
    .await
}

/// The feature-off twin (the sql no-sql precedent): the document
/// grammar is ungated, so a well-formed surreal validate target
/// parses in every build — only a directly-constructed call reaches
/// this arm — and it fails with the verdict-class mismatch naming the
/// gate instead of passing silently.
#[cfg(not(feature = "surreal"))]
pub(crate) async fn surreal_validate_action(
    index: usize,
    target: &SurrealTarget,
    expected: &RowsExpectation,
    deadline: Option<Duration>,
    catalog: Option<&Arc<dyn camel_api::datasource::DatasourceCatalog>>,
) -> Result<(), ScenarioFailure> {
    let _ = (target, expected, deadline, catalog);
    Err(ScenarioFailure::ValidationMismatch {
        action: index,
        detail: "surreal validation requires the `surreal` feature".to_string(),
    })
}

/// One driver-side failure of the apparatus (feature `surreal`): the
/// transport class carrying the datasource NAME and the
/// pre-sanitized error text — the `db_url` never renders (ADR-0051).
#[cfg(feature = "surreal")]
fn apparatus(index: usize, name: &str, text: String) -> ScenarioFailure {
    ScenarioFailure::ActionTransport {
        action: index,
        source: crate::adapters::TransportError::Other {
            message: format!("surreal validation: datasource '{name}': {text}"),
        },
    }
}

/// One snapshot of the query's record set (feature `surreal`): the
/// projected tuples plus the projection field names the projection
/// ran against.
#[cfg(feature = "surreal")]
struct Snapshot {
    /// The projection field names in declared order: the declared
    /// `columns` (loader law, task 1.2: surreal row patterns always
    /// carry one), empty for a count-bound-only expectation.
    columns: Vec<String>,
    /// The projected rows: one tuple per result object, or empty
    /// tuples when no projection ran (only the row count reaches the
    /// matcher).
    tuples: Vec<Vec<camel_api::Value>>,
}

/// Executes the query's read and projects its result objects per the
/// expectation (feature `surreal`). The SDK reports statement errors
/// INSIDE the response object — an awaited `query` only establishes
/// transport success (the prepare executor's law) — so the response is
/// `.check()`ed before any result is read, and every driver-error
/// string is sanitized against the config's `db_url` (ADR-0051).
///
/// A count-bound-only expectation skips projection entirely: only the
/// row count reaches the matcher, so the tuples stay empty per row.
#[cfg(feature = "surreal")]
async fn snapshot(
    index: usize,
    name: &str,
    target: &SurrealTarget,
    expected: &RowsExpectation,
    client: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    db_url: &str,
) -> Result<Snapshot, ScenarioFailure> {
    let mut response = client
        .query(&target.query)
        .await
        .map_err(|e| {
            apparatus(
                index,
                name,
                crate::steering::sanitize_db_error(&e.to_string(), db_url),
            )
        })?
        .check()
        .map_err(|e| {
            apparatus(
                index,
                name,
                crate::steering::sanitize_db_error(&e.to_string(), db_url),
            )
        })?;
    let rows: Vec<surrealdb::types::Value> = response.take(0).map_err(|e| {
        apparatus(
            index,
            name,
            crate::steering::sanitize_db_error(&e.to_string(), db_url),
        )
    })?;
    let tuples = match &expected.columns {
        Some(fields) => surreal_rows_to_tuples(rows, fields).map_err(|detail| {
            ScenarioFailure::ValidationMismatch {
                action: index,
                detail,
            }
        })?,
        None => vec![Vec::new(); rows.len()],
    };
    Ok(Snapshot {
        columns: expected.columns.clone().unwrap_or_default(),
        tuples,
    })
}

/// Decides the snapshot against the expectation's one shape (feature
/// `surreal`), the sql twin's matcher call: concrete row patterns
/// through [`camel_matchers::rows_match`] (positional when ordered, a
/// perfect matching when unordered), a row-count bound through
/// [`camel_matchers::bound_holds`].
#[cfg(feature = "surreal")]
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

/// The mismatch detail of a failed rows assertion (feature
/// `surreal`): the datasource name, the expectation shape in its own
/// grammar ([`camel_matchers::render_bound`] for a bound, the row
/// count and order rule for concrete rows), the actual row count, and
/// the projection field names — never an actual cell value, never the
/// query text, never the `db_url` (ADR-0051).
#[cfg(feature = "surreal")]
fn mismatch(
    index: usize,
    datasource: &str,
    expected: &RowsExpectation,
    snapshot: &Snapshot,
) -> ScenarioFailure {
    ScenarioFailure::ValidationMismatch {
        action: index,
        detail: surreal_mismatch_detail(
            datasource,
            expected,
            snapshot.tuples.len(),
            &snapshot.columns,
        ),
    }
}

/// Renders a failed surreal rows assertion: "surreal {datasource},
/// {render_bound(bound) | expected N rows (ordered|unordered)},
/// actual {count} rows, columns: [names]".
#[cfg(feature = "surreal")]
fn surreal_mismatch_detail(
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
        "surreal {datasource}, {shape}, actual {actual_count} rows, columns: [{}]",
        columns.join(", ")
    )
}

/// Maps one SurrealQL value to a matcher cell (feature `surreal`),
/// the fail-closed type law (surreal-state-tier task 2.3): the
/// absence and null kinds stay `null`; booleans map to booleans;
/// integers and finite floats map to JSON numbers; strings, UUIDs and
/// datetimes map to their string forms; a record id maps to its
/// `table:key` string; objects and arrays map to the structured value
/// the matcher verbs see whole, built RECURSIVELY through this
/// function so a nested unsupported kind fails closed naming the
/// field path (`field.key`, `field[0]`).
///
/// Every other SurrealQL kind — decimal and non-finite numbers,
/// bytes, durations, geometries, tables, files, ranges, regexes, and
/// sets — is an explicit failure naming the field and its kind:
/// fail-closed, never a silent null and never a sentinel a wildcard
/// could match away.
#[cfg(feature = "surreal")]
pub(crate) fn surreal_value_to_cell(
    v: &surrealdb::types::Value,
    field: &str,
) -> Result<camel_api::Value, String> {
    use surrealdb::types::Number;
    use surrealdb::types::ToSql as _;
    use surrealdb::types::Value as Sv;

    let unsupported = |kind: &str| {
        format!(
            "surreal validation: field `{field}` has unsupported SurrealQL kind `{kind}` (fail-closed)"
        )
    };
    match v {
        Sv::None | Sv::Null => Ok(camel_api::Value::Null),
        Sv::Bool(b) => Ok(camel_api::Value::Bool(*b)),
        Sv::Number(Number::Int(i)) => Ok(camel_api::Value::from(*i)),
        Sv::Number(Number::Float(f)) if f.is_finite() => Ok(camel_api::Value::from(*f)),
        Sv::Number(Number::Float(f)) => Err(unsupported(if f.is_nan() {
            "NaN"
        } else {
            "infinite float"
        })),
        Sv::Number(Number::Decimal(_)) => Err(unsupported("decimal")),
        Sv::String(s) => Ok(camel_api::Value::String(s.clone())),
        Sv::Uuid(u) => Ok(camel_api::Value::String(u.to_string())),
        Sv::Datetime(d) => Ok(camel_api::Value::String(d.to_string())),
        Sv::RecordId(rid) => Ok(camel_api::Value::String(rid.to_sql())),
        Sv::Object(object) => {
            let mut map = serde_json::Map::with_capacity(object.len());
            for (key, value) in object.iter() {
                let path = format!("{field}.{key}");
                map.insert(key.clone(), surreal_value_to_cell(value, &path)?);
            }
            Ok(camel_api::Value::Object(map))
        }
        Sv::Array(array) => {
            let mut items = Vec::with_capacity(array.len());
            for (i, value) in array.iter().enumerate() {
                let path = format!("{field}[{i}]");
                items.push(surreal_value_to_cell(value, &path)?);
            }
            Ok(camel_api::Value::Array(items))
        }
        Sv::Bytes(_) => Err(unsupported("bytes")),
        Sv::Duration(_) => Err(unsupported("duration")),
        Sv::Geometry(_) => Err(unsupported("geometry")),
        Sv::Table(_) => Err(unsupported("table")),
        Sv::File(_) => Err(unsupported("file")),
        Sv::Range(_) => Err(unsupported("range")),
        Sv::Regex(_) => Err(unsupported("regex")),
        Sv::Set(_) => Err(unsupported("set")),
    }
}

/// Projects one snapshot's result objects into matcher tuples (feature
/// `surreal`): every object narrows to the declared projection, field
/// by field in declared order, through
/// [`surreal_value_to_cell`]. A field the result object does not
/// carry fails closed naming it (an unknown projection field never
/// silently drops), and so does a result row that is not an object.
#[cfg(feature = "surreal")]
pub(crate) fn surreal_rows_to_tuples(
    rows: Vec<surrealdb::types::Value>,
    fields: &[String],
) -> Result<Vec<Vec<camel_api::Value>>, String> {
    use surrealdb::types::Value as Sv;

    let mut tuples = Vec::with_capacity(rows.len());
    for row in rows {
        let Sv::Object(object) = &row else {
            return Err(
                "surreal validation: projection expects object results, got a non-object row \
                 (fail-closed)"
                    .to_string(),
            );
        };
        let mut tuple = Vec::with_capacity(fields.len());
        for field in fields {
            let value = object.get(field).ok_or_else(|| {
                format!(
                    "surreal validation: unknown projection field `{field}`: the result does \
                     not carry it (fail-closed)"
                )
            })?;
            tuple.push(surreal_value_to_cell(value, field)?);
        }
        tuples.push(tuple);
    }
    Ok(tuples)
}
