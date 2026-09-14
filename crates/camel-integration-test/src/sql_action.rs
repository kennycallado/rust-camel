//! Scenario `sql:` action — datasource state preparation for the
//! integration tier (bd rc-25lup.1).
//!
//! Papal-locked architecture (bd rc-25lup, consult 2026-09-08): this is
//! the scenario state branch. `sql:` seeds mutable state at rest
//! through a datasource's pool before route assertions run; reads stay
//! in the `validate` sql target and the two vocabularies are never
//! mixed (Citrus precedent: fixture setup and assertions are separate
//! actions). [`is_read_statement`] + [`validate_sql_action`] enforce
//! that split at document load time.
//!
//! Executor diagnostics pass through [`sanitize_db_error`], which
//! redacts the datasource URL per ADR-0051 (Credential Redaction at
//! Diagnostic Boundaries): database URLs carry credential bytes and
//! must not reach test failure output.

use camel_api::CamelError;
use camel_config::config::CamelConfig;
use serde::Deserialize;

/// Key under which a scenario action selects this vocabulary.
pub const SQL_ACTION_KEY: &str = "sql";

/// Diagnostic key for the boot-time lint that rejects per-connection
/// sqlite `:memory:` datasource URLs (spa-3, bd rc-25lup.1).
pub const SQL_MEMORY_NOT_SHARED: &str = "sql-memory-not-shared";

/// Raw serde shape of the scenario `sql:` action as parsed from the
/// document. Field names stay snake_case despite the `camelCase`
/// rename (no multi-word fields today) — the attribute is load-bearing
/// for future fields.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RawSqlAction {
    datasource: String,
    prepare: Vec<String>,
}

/// Validated, typed twin of [`RawSqlAction`].
#[derive(Debug, Clone)]
pub struct SqlAction {
    pub datasource: String,
    pub prepare: Vec<String>,
}

/// Whether `stmt` is a read (`select`/`with` prefix), tolerating
/// leading whitespace and a wrapping parenthesis group. Re-trimming
/// after each dropped `(` is what makes `"  ( select 1 )"` a read.
pub fn is_read_statement(stmt: &str) -> bool {
    let mut rest = stmt;
    loop {
        rest = rest.trim_start();
        if let Some(without_paren) = rest.strip_prefix('(') {
            rest = without_paren;
        } else {
            break;
        }
    }
    let lowered = rest.to_lowercase();
    lowered.starts_with("select") || lowered.starts_with("with")
}

/// Validates the raw shape into the typed action. Returns owned error
/// strings; the `document.rs` hook wraps them into `DocError`.
pub fn validate_sql_action(raw: &RawSqlAction, action_index: usize) -> Result<SqlAction, String> {
    if raw.prepare.is_empty() {
        return Err(format!(
            "sql action {action_index}: prepare list must not be empty"
        ));
    }
    for (i, stmt) in raw.prepare.iter().enumerate() {
        if is_read_statement(stmt) {
            return Err(format!(
                "sql action {action_index}: prepare statement {i} is a read (select/with \
                 prefix); reads belong to the validate sql target"
            ));
        }
    }
    Ok(SqlAction {
        datasource: raw.datasource.clone(),
        prepare: raw.prepare.clone(),
    })
}

/// Replaces every occurrence of `db_url` in `err_text` with
/// `[REDACTED]` (ADR-0051). An empty `db_url` is a no-op — an empty
/// pattern would corrupt the text.
pub fn sanitize_db_error(err_text: &str, db_url: &str) -> String {
    if db_url.is_empty() {
        return err_text.to_string();
    }
    err_text.replace(db_url, "[REDACTED]")
}

/// Boot-time lint (ungated): rejects per-connection sqlite `:memory:`
/// datasource URLs, which give every pooled connection its own private
/// in-memory database — an INSERT through one connection and a SELECT
/// through another can hit different databases.
///
/// The remediation message names two accepted forms:
///
/// - Named shared-memory URI, the scenario-tier convention, e.g.
///   `sqlite:file:memdb_appdb?mode=memory&cache=shared`. All pool
///   connections share one named in-memory database; the name is stable
///   across connections and parses with the sqlx driver. `sqlite:file:`
///   matches no automatic datasource factory prefix (factories match
///   `scheme://` or `scheme::`), so the config must also pin
///   `provider = "sqlx"`.
/// - Bare shared cache, `sqlite::memory:?cache=shared`. Accepted, but
///   each parse gets a sqlx-assigned private name, and pool connections
///   under the Any driver can hold private databases — pin
///   `max_connections = 1` with this form.
///
/// Iterates `config.datasources` in BTreeMap order (the config stores a
/// `HashMap`, whose iteration order is unspecified) so diagnostics are
/// deterministic. The error names the datasource only, never its URL
/// (ADR-0051 credential redaction).
pub fn ensure_sqlite_memory_shared(config: &CamelConfig) -> Result<(), CamelError> {
    let mut names: Vec<&String> = config.datasources.keys().collect();
    names.sort();
    for name in names {
        let lowered = config.datasources[name].db_url.to_lowercase();
        let prefix = [
            "sqlite::memory:",
            "sqlite://:memory:",
            "sqlite://file::memory:",
        ]
        .iter()
        .find(|p| lowered.starts_with(**p))
        .copied();
        if let Some(prefix) = prefix {
            let remainder = &lowered[prefix.len()..];
            if !remainder.contains("cache=shared") {
                return Err(CamelError::Config(format!(
                    "{}: datasource '{}' uses a per-connection sqlite :memory: URL without \
                     cache=shared; INSERT and SELECT can hit different databases. Prefer the \
                     named shared-memory URI sqlite:file:memdb_{}?mode=memory&cache=shared; \
                     sqlite:file: matches no automatic datasource prefix, so also set \
                     provider = \"sqlx\". The bare sqlite::memory:?cache=shared URL is also \
                     accepted, but cross-connection sharing is not guaranteed; set \
                     max_connections = 1 so state stays on one connection",
                    SQL_MEMORY_NOT_SHARED, name, name
                )));
            }
        }
    }
    Ok(())
}

#[cfg(feature = "sql")]
use std::sync::Arc;

#[cfg(feature = "sql")]
use camel_api::datasource::DatasourceCatalog;

/// Executes the prepare statements in order against the named
/// datasource's pool. Stops at the first failure, reporting the
/// statement index and a sanitized error; never emits the datasource
/// URL.
#[cfg(feature = "sql")]
pub async fn execute_sql_prepare(
    catalog: &Arc<dyn DatasourceCatalog>,
    action: &SqlAction,
) -> Result<(), String> {
    let name = &action.datasource;
    let Some(config) = catalog.get_config(name) else {
        return Err(format!("sql action: unknown datasource '{name}'"));
    };
    let handle = catalog.get_pool(name).await.map_err(|e| {
        format!(
            "sql action: datasource '{name}': {}",
            sanitize_db_error(&e.to_string(), &config.db_url)
        )
    })?;
    let pool = handle.downcast::<sqlx::AnyPool>().map_err(|e| {
        format!(
            "sql action: datasource '{name}': {}",
            sanitize_db_error(&e.to_string(), &config.db_url)
        )
    })?;
    for (i, stmt) in action.prepare.iter().enumerate() {
        sqlx::query(stmt).execute(&*pool).await.map_err(|e| {
            format!(
                "datasource '{name}' statement [{i}]: {}",
                sanitize_db_error(&e.to_string(), &config.db_url)
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use camel_api::datasource::DatasourceConfig;

    fn lint_config(db_url: &str) -> CamelConfig {
        let mut config = CamelConfig::default();
        config.datasources.insert(
            "appdb".to_string(),
            DatasourceConfig {
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
            },
        );
        config
    }

    fn raw(datasource: &str, prepare: Vec<&str>) -> RawSqlAction {
        RawSqlAction {
            datasource: datasource.to_string(),
            prepare: prepare.into_iter().map(str::to_string).collect(),
        }
    }

    #[test]
    fn read_statement_detection() {
        assert!(is_read_statement("SELECT 1"));
        assert!(is_read_statement("  ( select 1 )"));
        assert!(is_read_statement("With x AS (SELECT 1) SELECT * FROM x"));
        assert!(!is_read_statement("INSERT INTO t VALUES (1)"));
        assert!(!is_read_statement("(CREATE TABLE t (x INTEGER))"));
    }

    #[test]
    fn validation_rejects_empty_prepare() {
        let err = validate_sql_action(&raw("appdb", vec![]), 3).unwrap_err();
        assert!(err.contains("sql action 3"), "got: {err}");
        assert!(err.contains("prepare list must not be empty"), "got: {err}");
    }

    #[test]
    fn validation_rejects_read_prefixes() {
        let err = validate_sql_action(&raw("appdb", vec!["(SELECT 1)"]), 0).unwrap_err();
        assert!(err.contains("statement 0"), "got: {err}");

        let err = validate_sql_action(
            &raw(
                "appdb",
                vec![
                    "CREATE TABLE t (x INTEGER)",
                    "with cte as (select 1) select * from cte",
                ],
            ),
            0,
        )
        .unwrap_err();
        assert!(err.contains("statement 1"), "got: {err}");
    }

    #[test]
    fn validation_accepts_mutations() {
        let action = validate_sql_action(
            &raw(
                "appdb",
                vec![
                    "CREATE TABLE t (x INTEGER)",
                    "INSERT INTO t VALUES (1)",
                    "DELETE FROM t WHERE x = 1",
                ],
            ),
            0,
        )
        .unwrap();
        assert_eq!(action.datasource, "appdb");
        assert_eq!(
            action.prepare,
            vec![
                "CREATE TABLE t (x INTEGER)".to_string(),
                "INSERT INTO t VALUES (1)".to_string(),
                "DELETE FROM t WHERE x = 1".to_string(),
            ]
        );
    }

    #[test]
    fn sanitizer_redacts_db_url() {
        let sanitized = sanitize_db_error(
            "connect failed sqlite::memory:?cache=shared&x=1",
            "sqlite::memory:?cache=shared&x=1",
        );
        assert!(sanitized.contains("[REDACTED]"), "got: {sanitized}");
        assert!(!sanitized.contains("cache=shared&x=1"), "got: {sanitized}");
    }

    #[test]
    fn sanitizer_empty_url_noop() {
        assert_eq!(sanitize_db_error("boom", ""), "boom");
    }

    #[test]
    fn lint_message_steers_to_named_form() {
        let err = ensure_sqlite_memory_shared(&lint_config("sqlite::memory:")).unwrap_err();
        let text = err.to_string();
        assert!(text.contains(SQL_MEMORY_NOT_SHARED), "got: {text}");
        assert!(
            text.contains("sqlite:file:memdb_appdb?mode=memory&cache=shared"),
            "got: {text}"
        );
        assert!(text.contains("provider"), "got: {text}");
        assert!(text.contains("max_connections = 1"), "got: {text}");
        assert!(!text.contains("<name>"), "got: {text}");
        assert!(!text.contains("<scenario>"), "got: {text}");
    }

    #[test]
    fn lint_accepts_bare_shared_form() {
        let config = lint_config("sqlite::memory:?cache=shared");
        assert!(ensure_sqlite_memory_shared(&config).is_ok());
    }

    #[test]
    fn lint_named_file_uri_passes() {
        let config = lint_config("sqlite:file:memdb_appdb?mode=memory&cache=shared");
        assert!(ensure_sqlite_memory_shared(&config).is_ok());
    }

    #[test]
    fn lint_rejects_file_memory_uri_without_shared_cache() {
        let config = lint_config("sqlite://file::memory:?mode=memory");
        let err = ensure_sqlite_memory_shared(&config).unwrap_err();
        assert!(err.to_string().contains(SQL_MEMORY_NOT_SHARED));
    }

    #[test]
    fn lint_accepts_file_memory_uri_with_shared_cache() {
        let config = lint_config("sqlite://file::memory:?mode=memory&cache=shared");
        assert!(ensure_sqlite_memory_shared(&config).is_ok());
    }
}
