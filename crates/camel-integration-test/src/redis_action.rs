//! Scenario `redis:` delete-only prepare action — datasource state
//! preparation for the integration tier (bd rc-6wodc).
//!
//! Mirrors the scenario `sql:` branch (bd rc-25lup): `redis:` deletes
//! document-authored keys through a datasource before route
//! assertions run, or in a document-level `finally:` list as teardown;
//! reads stay in the `validate` redis target
//! (`datasource/key/type/ttl`) and the two vocabularies are never
//! mixed. [`validate_redis_del_action`] enforces the literal-key law
//! at document load time.
//!
//! Delete-only by design: no `FLUSHDB`, no `FLUSHALL`, no pattern
//! delete, no generic command runner. A key containing a glob
//! metacharacter is a load-time grammar defect; pattern delete is a
//! separate future issue (rc-ql8eq).
//!
//! The grammar and validation are ungated and run in every build;
//! executing the delete is demand-gated behind the harness `redis`
//! feature (the `sql:`/`surreal:` precedent, ADR-0069 §8). Executor
//! diagnostics pass through [`crate::steering::sanitize_db_error`],
//! which redacts the datasource URL per ADR-0051 (Credential
//! Redaction at Diagnostic Boundaries).

use serde::Deserialize;

/// Key under which a scenario action selects this vocabulary.
pub const REDIS_DEL_ACTION_KEY: &str = "redis";

/// Raw serde shape of the scenario `redis:` delete action as parsed
/// from the document. Field names stay snake_case despite the
/// `camelCase` rename (`del` is single-word) — the attribute is
/// load-bearing for future fields.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RawRedisDelAction {
    datasource: String,
    del: Vec<String>,
}

/// Validated, typed twin of [`RawRedisDelAction`]: `del` becomes
/// `keys`, the literal keys one `DEL` removes.
#[derive(Debug, Clone)]
pub struct RedisDelAction {
    pub datasource: String,
    pub keys: Vec<String>,
}

/// Validates the raw shape into the typed action. Returns owned error
/// strings; the `document.rs` hook wraps them into `DocError`.
///
/// The grammar is ungated: every defect here is a document defect,
/// independent of what the current build can execute, so a featureless
/// build reports it before the demand gate.
pub fn validate_redis_del_action(
    raw: &RawRedisDelAction,
    action_index: usize,
) -> Result<RedisDelAction, String> {
    // Identifier law: the datasource is a configured name, never an
    // env-interpolated URL.
    if raw.datasource.is_empty() {
        return Err(format!(
            "redis action {action_index}: `datasource` must not be empty (it names a configured \
             datasource)"
        ));
    }
    if raw.del.is_empty() {
        return Err(format!(
            "redis action {action_index}: `del` must declare at least one key (the empty-list \
             law)"
        ));
    }
    for (i, key) in raw.del.iter().enumerate() {
        if let Some(found) = crate::document::glob_metacharacters_in(key) {
            return Err(format!(
                "redis action {action_index}: key `{key}` (del entry {i}) contains glob \
                 metacharacters `{found}`; `del` takes literal keys only"
            ));
        }
    }
    Ok(RedisDelAction {
        datasource: raw.datasource.clone(),
        keys: raw.del.clone(),
    })
}

#[cfg(feature = "redis")]
use std::sync::Arc;

#[cfg(feature = "redis")]
use camel_api::datasource::DatasourceCatalog;

/// Deletes every declared literal key through the named datasource's
/// connection in ONE `DEL` (feature `redis`).
///
/// Resolution goes through the booted catalog's steering axis — one
/// datasource name, one connection handle, never a self-constructed
/// connection (the `Redis state assertion` law). `DEL` is idempotent:
/// a missing key is success, so the deleted count is ignored. A driver
/// error is reported with the datasource name and the ADR-0051
/// sanitized driver text; the URL never renders.
#[cfg(feature = "redis")]
pub async fn execute_redis_del(
    catalog: &Arc<dyn DatasourceCatalog>,
    action: &RedisDelAction,
) -> Result<(), String> {
    let name = &action.datasource;
    let (conn, db_url) = crate::steering::resolve_datasource::<redis::aio::MultiplexedConnection>(
        catalog,
        name,
        "redis delete",
    )
    .await?;
    let mut cmd = redis::cmd("DEL");
    for key in &action.keys {
        cmd.arg(key);
    }
    let mut conn = (*conn).clone();
    cmd.query_async::<i64>(&mut conn).await.map_err(|e| {
        format!(
            "datasource '{name}': {}",
            crate::steering::sanitize_db_error(&e.to_string(), &db_url)
        )
    })?;
    Ok(())
}
