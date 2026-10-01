//! Scenario `surreal:` action — datasource state preparation for the
//! integration tier (surreal-state-tier task 1.1).
//!
//! Mirrors the scenario `sql:` branch (bd rc-25lup): `surreal:` seeds
//! mutable state at rest through a datasource before route assertions
//! run; reads stay in the `validate` surreal target and the two
//! vocabularies are never mixed. [`is_surreal_read_statement`] +
//! [`validate_surreal_action`] enforce that split at document load
//! time.
//!
//! The grammar and validation are ungated and run in every build;
//! executing the prepared statements is demand-gated behind the
//! harness `surreal` feature (the `sql:`/`http` precedent, ADR-0069
//! §8).

use serde::Deserialize;

/// Key under which a scenario action selects this vocabulary.
pub const SURREAL_ACTION_KEY: &str = "surreal";

/// Raw serde shape of the scenario `surreal:` action as parsed from
/// the document. Field names stay snake_case despite the `camelCase`
/// rename (no multi-word fields today) — the attribute is load-bearing
/// for future fields.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RawSurrealAction {
    datasource: String,
    prepare: Vec<String>,
}

/// Validated, typed twin of [`RawSurrealAction`].
#[derive(Debug, Clone)]
pub struct SurrealAction {
    pub datasource: String,
    pub prepare: Vec<String>,
}

#[cfg(test)]
impl RawSurrealAction {
    /// Test-only constructor: the raw shape's fields stay private in
    /// the production grammar; the external test module builds
    /// instances through here (the sql tests construct in-module
    /// instead).
    pub(crate) fn raw(datasource: &str, prepare: Vec<&str>) -> RawSurrealAction {
        RawSurrealAction {
            datasource: datasource.to_string(),
            prepare: prepare.into_iter().map(str::to_string).collect(),
        }
    }
}

/// Whether `stmt` is a read (`select` prefix), tolerating leading
/// whitespace and a wrapping parenthesis group. Re-trimming after each
/// dropped `(` is what makes `"  ( select 1 )"` a read. SurrealQL has
/// no CTE form, so `select` is the only read prefix (the sql twin also
/// accepts `with`).
pub fn is_surreal_read_statement(stmt: &str) -> bool {
    let mut rest = stmt;
    loop {
        rest = rest.trim_start();
        if let Some(without_paren) = rest.strip_prefix('(') {
            rest = without_paren;
        } else {
            break;
        }
    }
    rest.to_lowercase().starts_with("select")
}

/// Validates the raw shape into the typed action. Returns owned error
/// strings; the `document.rs` hook wraps them into `DocError`.
pub fn validate_surreal_action(
    raw: &RawSurrealAction,
    action_index: usize,
) -> Result<SurrealAction, String> {
    if raw.prepare.is_empty() {
        return Err(format!(
            "surreal action {action_index}: prepare list must not be empty"
        ));
    }
    for (i, stmt) in raw.prepare.iter().enumerate() {
        if is_surreal_read_statement(stmt) {
            return Err(format!(
                "surreal action {action_index}: prepare statement {i} is a read (select \
                 prefix); reads belong to the validate surreal target"
            ));
        }
    }
    Ok(SurrealAction {
        datasource: raw.datasource.clone(),
        prepare: raw.prepare.clone(),
    })
}

#[cfg(feature = "surreal")]
use std::sync::Arc;

#[cfg(feature = "surreal")]
use camel_api::datasource::DatasourceCatalog;

#[cfg(feature = "surreal")]
use surrealdb::Surreal;
#[cfg(feature = "surreal")]
use surrealdb::engine::any::Any as SurrealAny;

#[cfg(feature = "surreal")]
use crate::sql_action::sanitize_db_error;

/// Executes the prepare statements in order against the named
/// datasource's Surreal client (the sql executor's twin, bd rc-25lup.1).
/// Stops at the first failure, reporting the statement index and a
/// sanitized error; never emits the datasource URL.
///
/// The SDK reports statement errors INSIDE the response object — an
/// awaited `query` only establishes transport success — so each
/// response is `.check()`ed before its results are discarded. Each
/// prepare statement rides its own `query` call, which keeps the
/// reported index exact even though SurrealQL batch semantics would
/// otherwise blur statement boundaries.
#[cfg(feature = "surreal")]
pub async fn execute_surreal_prepare(
    catalog: &Arc<dyn DatasourceCatalog>,
    action: &SurrealAction,
) -> Result<(), String> {
    let name = &action.datasource;
    let Some(config) = catalog.get_config(name) else {
        return Err(format!("surreal action: unknown datasource '{name}'"));
    };
    let handle = catalog.get_pool(name).await.map_err(|e| {
        format!(
            "surreal action: datasource '{name}': {}",
            sanitize_db_error(&e.to_string(), &config.db_url)
        )
    })?;
    let client = handle.downcast::<Surreal<SurrealAny>>().map_err(|e| {
        format!(
            "surreal action: datasource '{name}': {}",
            sanitize_db_error(&e.to_string(), &config.db_url)
        )
    })?;
    for (i, stmt) in action.prepare.iter().enumerate() {
        client
            .query(stmt)
            .await
            .map_err(|e| {
                format!(
                    "datasource '{name}' statement [{i}]: {}",
                    sanitize_db_error(&e.to_string(), &config.db_url)
                )
            })?
            .check()
            .map_err(|e| {
                format!(
                    "datasource '{name}' statement [{i}]: {}",
                    sanitize_db_error(&e.to_string(), &config.db_url)
                )
            })?;
    }
    Ok(())
}
