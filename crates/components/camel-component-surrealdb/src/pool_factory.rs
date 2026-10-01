//! SurrealDB PoolFactory — creates Surreal\<Any\> clients and registers with the datasource catalog.

use std::any::Any as StdAny;
use std::sync::Arc;

use camel_api::datasource::{
    CheckFuture, CloseFuture, CreatePoolFuture, DatasourceConfig, PoolFactory,
};
use camel_api::lifecycle::HealthStatus;
use camel_component_api::{NetworkRetryPolicy, retry_async};
use surrealdb::Surreal;
use surrealdb::engine::any::Any as SurrealAny;
use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Root;

use crate::error::SurrealDbError;

/// Redacts the user:password portion of a SurrealDB endpoint URL for safe
/// display. Mirrors the canonical `redact_db_url` implementation in
/// `camel-sql/src/config.rs`: returns `scheme://***@host/db` for URLs with
/// userinfo, or the original URL otherwise. Falls back to the original input
/// when the URL cannot be parsed (e.g. `memory` or `kube` scheme-less forms).
pub fn redact_db_url(db_url: &str) -> String {
    match url::Url::parse(db_url) {
        Ok(mut parsed) => {
            if parsed.username().is_empty() && parsed.password().is_none() {
                return db_url.to_string();
            }
            let _ = parsed.set_username("***");
            let _ = parsed.set_password(Some("***"));
            parsed.to_string()
        }
        Err(_) => db_url.to_string(),
    }
}

/// Extracts a string from the `extra` map on a `DatasourceConfig`.
fn extra_str(config: &DatasourceConfig, key: &str) -> Result<String, camel_api::CamelError> {
    extra_str_opt(config, key).ok_or_else(|| {
        camel_api::CamelError::Config(format!(
            "datasource extra field '{key}' is required for surrealdb"
        ))
    })
}

/// Extracts an optional string from the `extra` map on a `DatasourceConfig`.
fn extra_str_opt(config: &DatasourceConfig, key: &str) -> Option<String> {
    config
        .extra
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Extracts a string for a `mem://` datasource, falling back to `default`
/// when the key is absent. A key that is present but holds a non-string value
/// is a hard error rather than a silent fallback, matching the remote path's
/// treatment of malformed extra fields.
fn extra_str_or_default(
    config: &DatasourceConfig,
    key: &str,
    default: &str,
) -> Result<String, camel_api::CamelError> {
    match config.extra.get(key) {
        None => Ok(default.to_string()),
        Some(value) => match value.as_str() {
            Some(s) => Ok(s.to_string()),
            None => Err(camel_api::CamelError::Config(format!(
                "datasource extra field '{key}' must be a string for surrealdb"
            ))),
        },
    }
}

/// PoolFactory for SurrealDB. Creates a `Surreal<Any>` client per datasource.
///
/// Auth order (per SDK examples + spike): connect → signin → use_ns → use_db.
/// Root fields are `String` (v3 SDK), not `&str`.
///
/// `mem://` endpoints are the exception: each connect spawns a fresh embedded
/// instance with authentication disabled (there is no root user to sign in),
/// so the username/password extras are not required and the signin step is
/// skipped. Namespace/database default to `"test"` when absent, and the
/// `use_ns`/`use_db` calls still run with those defaults. Remote endpoints
/// (`ws`/`wss`/`http`/`https`) keep the mandatory extras and the signin step.
///
/// # Retry semantics
///
/// Two phases are retried, both with [`NetworkRetryPolicy::default()`] (the
/// canonical enabled-with-backoff policy):
///
/// - **Transport establishment**: `connect(endpoint)` is wrapped in
///   `retry_async` with [`SurrealDbError::is_retryable`]. Connection setup is
///   idempotent — reconnecting twice is harmless — so transient failures
///   (connection refused, DNS hiccup, TLS negotiation drop) retry with capped
///   exponential backoff per ADR-0013.
///
/// - **Post-connect setup** (`signin → use_ns → use_db`): retried with the
///   [`is_transaction_conflict`](crate::error::is_transaction_conflict) classifier. SurrealDB v3 can return a
///   retryable `QueryError::TransactionConflict` from these calls when
///   multiple connections concurrently establish against the same ns/db and
///   contend on the catalog write transaction (e.g. parallel integration
///   tests, or concurrent pool creation in production). The setup sequence
///   is idempotent, so retrying is safe. Auth failures (bad credentials) and
///   `NotFound` errors are NOT transaction conflicts, so they fail fast
///   without burning retry attempts.
///
/// This mirrors `camel-sql/src/consumer.rs` (which retries `pool.connect()`)
/// but differs in policy source: SQL reads `retry` from its endpoint config
/// (SQL creates pools per-endpoint), whereas surrealdb pools are
/// datasource-scoped and the pool factory receives only `DatasourceConfig` —
/// so the default policy is used here. This is the only load-bearing retry
/// site in the surrealdb component today. `SurrealDbEndpointConfig::retry`
/// remains as the public contract for future producer-side retry (none of
/// today's producer paths emit a retryable variant; see ADR-0013).
pub struct SurrealDbPoolFactory;

impl PoolFactory for SurrealDbPoolFactory {
    fn create<'a>(&'a self, config: &'a DatasourceConfig) -> CreatePoolFuture<'a> {
        Box::pin(async move {
            let endpoint = &config.db_url;
            // `mem://` spawns an isolated embedded instance with
            // authentication disabled per connect — there is no root user to
            // sign in, so credentials are not required and ns/db default to
            // "test". Remote endpoints keep the mandatory credential extras.
            let is_mem = url::Url::parse(endpoint).is_ok_and(|parsed| parsed.scheme() == "mem");
            let (ns, db, credentials) = if is_mem {
                (
                    extra_str_or_default(config, "namespace", "test")?,
                    extra_str_or_default(config, "database", "test")?,
                    None,
                )
            } else {
                (
                    extra_str(config, "namespace")?,
                    extra_str(config, "database")?,
                    Some((
                        extra_str(config, "username")?,
                        extra_str(config, "password")?,
                    )),
                )
            };

            // Retry only the transport-establishment call (idempotent).
            // Per ADR-0013 security note: `connect` operates on the
            // creds-free `db_url` (credentials live in the `extra` map and
            // are passed separately via `signin`), so the error Display
            // logged by `retry_async` at WARN does not leak credentials.
            let policy = NetworkRetryPolicy::default();
            let client: Surreal<SurrealAny> = retry_async::<_, _, _, _, SurrealDbError>(
                &policy,
                "surrealdb",
                "connect",
                || async {
                    connect(endpoint)
                        .await
                        .map_err(|source| SurrealDbError::Connection { source })
                },
                SurrealDbError::is_retryable,
                None,
            )
            .await
            .map_err(|e| {
                camel_api::CamelError::ProcessorError(format!(
                    "failed to create surrealdb datasource pool ({}): {e}",
                    redact_db_url(endpoint)
                ))
            })?;

            // Post-connect setup: signin (remote only) → use_ns → use_db.
            // Retried only on transaction conflicts (see
            // `is_transaction_conflict` and the struct-level retry-semantics
            // comment). Auth failures, not-found, and other permanent errors
            // fail fast without burning attempts.
            retry_async::<_, _, _, _, surrealdb::Error>(
                &policy,
                "surrealdb",
                "setup",
                || async {
                    if let Some((username, password)) = &credentials {
                        client
                            .signin(Root {
                                username: username.clone(),
                                password: password.clone(),
                            })
                            .await?;
                    }
                    client.use_ns(&ns).await?;
                    client.use_db(&db).await?;
                    Ok(())
                },
                crate::error::is_transaction_conflict,
                None,
            )
            .await
            .map_err(|e| {
                camel_api::CamelError::ProcessorError(format!(
                    "surrealdb setup (signin/use_ns/use_db) failed for endpoint {}: {e}",
                    redact_db_url(endpoint)
                ))
            })?;

            tracing::info!(
                "surrealdb datasource pool created: endpoint={}, ns={}, db={}",
                redact_db_url(endpoint),
                ns,
                db
            );

            Ok(Arc::new(client) as Arc<dyn StdAny + Send + Sync>)
        })
    }

    fn check<'a>(&'a self, handle: &'a camel_api::datasource::DatasourceHandle) -> CheckFuture<'a> {
        Box::pin(async move {
            match handle.downcast::<Surreal<SurrealAny>>() {
                Ok(client) => match client.query("INFO FOR DB").await {
                    Ok(_) => HealthStatus::Healthy,
                    Err(e) => {
                        tracing::warn!("datasource '{}' health check failed: {}", handle.name, e);
                        HealthStatus::Unhealthy
                    }
                },
                Err(_) => HealthStatus::Unhealthy,
            }
        })
    }

    fn close<'a>(&'a self, handle: &'a camel_api::datasource::DatasourceHandle) -> CloseFuture<'a> {
        Box::pin(async move {
            let client = handle.downcast::<Surreal<SurrealAny>>().map_err(|e| {
                camel_api::CamelError::ProcessorError(format!(
                    "datasource '{}': pool close downcast failed: {}",
                    handle.name, e
                ))
            })?;
            // `invalidate()` is the SDK's only teardown lever: it revokes
            // the client's active auth session. The datasource catalog
            // keeps the handle cached after close_all (it never drops its
            // entry), so releasing by drop is not available here — the
            // invalidation is the observable release on the remote tiers.
            // On the embedded `mem://` tier authentication is disabled, so
            // the call is hygiene: nothing is revoked (the SDK's own doc
            // example runs `invalidate()` on a fresh `mem://` client and
            // expects Ok), but it must still complete Ok so a teardown
            // reports real failures only.
            client.invalidate().await.map_err(|e| {
                camel_api::CamelError::ProcessorError(format!(
                    "datasource '{}': surrealdb invalidate failed: {}",
                    handle.name, e
                ))
            })?;
            Ok(())
        })
    }

    fn supported_schemes(&self) -> &[&str] {
        &["ws", "wss", "http", "https", "mem"]
    }

    fn name(&self) -> &'static str {
        "surrealdb"
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use camel_api::datasource::DatasourceConfig;
    use toml::Value as TomlValue;

    use super::*;

    fn make_test_config(url: &str) -> DatasourceConfig {
        let mut extra = HashMap::new();
        extra.insert("namespace".into(), TomlValue::String("test_ns".into()));
        extra.insert("database".into(), TomlValue::String("test_db".into()));
        extra.insert("username".into(), TomlValue::String("test_user".into()));
        extra.insert("password".into(), TomlValue::String("test_pass".into()));
        DatasourceConfig {
            db_url: url.to_string(),
            provider: Some("surrealdb".into()),
            max_connections: None,
            min_connections: None,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
            extra,
        }
    }

    #[test]
    fn factory_name_is_surrealdb() {
        let factory = SurrealDbPoolFactory;
        assert_eq!(factory.name(), "surrealdb");
    }

    #[test]
    fn factory_supports_ws_scheme() {
        let factory = SurrealDbPoolFactory;
        assert!(factory.supported_schemes().contains(&"ws"));
    }

    #[test]
    fn factory_supports_wss_scheme() {
        let factory = SurrealDbPoolFactory;
        assert!(factory.supported_schemes().contains(&"wss"));
    }

    #[test]
    fn factory_supports_http_scheme() {
        let factory = SurrealDbPoolFactory;
        assert!(factory.supported_schemes().contains(&"http"));
    }

    #[test]
    fn factory_supports_https_scheme() {
        let factory = SurrealDbPoolFactory;
        assert!(factory.supported_schemes().contains(&"https"));
    }

    #[test]
    fn factory_matches_ws_url() {
        let factory = SurrealDbPoolFactory;
        let config = make_test_config("ws://localhost:8000");
        assert!(factory.matches(&config));
    }

    #[test]
    fn factory_matches_wss_url() {
        let factory = SurrealDbPoolFactory;
        let config = make_test_config("wss://localhost:8000");
        assert!(factory.matches(&config));
    }

    #[test]
    fn factory_matches_http_url() {
        let factory = SurrealDbPoolFactory;
        let config = make_test_config("http://localhost:8000");
        assert!(factory.matches(&config));
    }

    #[test]
    fn factory_does_not_match_postgres_url() {
        let factory = SurrealDbPoolFactory;
        let config = make_test_config("postgresql://localhost:5432/mydb");
        assert!(!factory.matches(&config));
    }

    #[test]
    fn extra_str_returns_value_for_valid_key() {
        let config = make_test_config("ws://localhost:8000");
        assert_eq!(extra_str(&config, "namespace").unwrap(), "test_ns");
    }

    #[test]
    fn extra_str_returns_error_for_missing_key() {
        let config = make_test_config("ws://localhost:8000");
        let err = extra_str(&config, "nonexistent").unwrap_err();
        assert!(err.to_string().contains("nonexistent"));
    }

    // --- URL redaction tests (CRITICAL: secret leak prevention) ---

    #[test]
    fn test_url_redaction_hides_credentials() {
        // wss://user:secret@host/db → wss://***:***@host/db
        let redacted = redact_db_url("wss://user:secret@host:8000/db");
        assert!(
            !redacted.contains("secret"),
            "redacted URL must not contain password: {redacted}"
        );
        assert!(
            !redacted.contains("user") || redacted.contains("***"),
            "redacted URL must not contain username: {redacted}"
        );
        assert!(
            redacted.contains("***"),
            "redacted URL must contain redaction marker: {redacted}"
        );
        assert!(
            redacted.contains("host"),
            "redacted URL must preserve host: {redacted}"
        );
    }

    #[test]
    fn test_url_redaction_preserves_url_without_credentials() {
        // URL with no userinfo → unchanged
        let url = "wss://localhost:8000";
        assert_eq!(redact_db_url(url), url);
    }

    #[test]
    fn test_url_redaction_preserves_unparseable_url() {
        // Unparseable URL (e.g. bare scheme-less path) → returned as-is
        let url = "local::memory";
        assert_eq!(redact_db_url(url), url);
    }

    #[test]
    fn test_url_redaction_with_token_only() {
        // URL with token-only (no password) — SurrealDB auth tokens
        let redacted = redact_db_url("wss://token@host/db");
        assert!(
            !redacted.contains("token") || redacted.contains("***"),
            "redacted URL must not leak token: {redacted}"
        );
    }

    // --- mem scheme (embedded, isolated instance per connect) ---

    /// A `mem://` config with no extras at all: the embedded instance runs
    /// with authentication disabled, so none of the remote credential
    /// extras are required.
    fn make_mem_config() -> DatasourceConfig {
        DatasourceConfig {
            db_url: "mem://".to_string(),
            provider: Some("surrealdb".into()),
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

    #[test]
    fn factory_supports_mem_scheme() {
        let factory = SurrealDbPoolFactory;
        assert!(factory.supported_schemes().contains(&"mem"));
    }

    #[tokio::test]
    async fn mem_connect_without_credentials() {
        let factory = SurrealDbPoolFactory;
        let config = make_mem_config();
        assert!(factory.matches(&config));
        let handle = factory
            .create(&config)
            .await
            .expect("mem:// create must succeed without credentials");
        let client = handle
            .downcast::<Surreal<SurrealAny>>()
            .expect("handle must be a Surreal<Any> client");
        client
            .query("RETURN 1")
            .await
            .expect("query transport must succeed")
            .check()
            .expect("query statement must succeed");
    }

    #[tokio::test]
    async fn mem_wrong_type_extra_is_error() {
        let factory = SurrealDbPoolFactory;
        let mut config = make_mem_config();
        config
            .extra
            .insert("namespace".into(), TomlValue::Integer(123));
        let err = factory
            .create(&config)
            .await
            .expect_err("mem:// create must reject a non-string namespace extra");
        assert!(
            err.to_string().contains("namespace"),
            "wrong-type error must name the extra field: {err}"
        );
        assert!(
            err.to_string().contains("must be a string"),
            "wrong-type error must state the value must be a string: {err}"
        );
    }

    #[tokio::test]
    async fn mem_connect_creates_isolated_client() {
        let factory = SurrealDbPoolFactory;
        let config = make_mem_config();
        let first = factory
            .create(&config)
            .await
            .expect("first mem:// create must succeed");
        let second = factory
            .create(&config)
            .await
            .expect("second mem:// create must succeed");
        let c1 = first
            .downcast::<Surreal<SurrealAny>>()
            .expect("first handle must be a Surreal<Any> client");
        let c2 = second
            .downcast::<Surreal<SurrealAny>>()
            .expect("second handle must be a Surreal<Any> client");
        assert!(
            !Arc::ptr_eq(&c1, &c2),
            "each mem:// create must yield a distinct client"
        );

        c1.query("CREATE person:one SET name = 'one'")
            .await
            .expect("create transport must succeed")
            .check()
            .expect("create statement must succeed");
        c2.query("CREATE person:two SET name = 'two'")
            .await
            .expect("create transport must succeed")
            .check()
            .expect("create statement must succeed");

        // Each instance must see exactly its own record. On a shared
        // instance both rows would be visible to both clients.
        let rows: Vec<surrealdb::types::Value> = c2
            .query("SELECT * FROM person")
            .await
            .expect("select transport must succeed")
            .check()
            .expect("select statement must succeed")
            .take(0)
            .expect("select must return an array");
        assert_eq!(
            rows.len(),
            1,
            "second client must see only its own record: {rows:?}"
        );
        let rows: Vec<surrealdb::types::Value> = c1
            .query("SELECT * FROM person")
            .await
            .expect("select transport must succeed")
            .check()
            .expect("select statement must succeed")
            .take(0)
            .expect("select must return an array");
        assert_eq!(
            rows.len(),
            1,
            "first client must see only its own record: {rows:?}"
        );
    }

    #[tokio::test]
    async fn close_hook_invalidates_and_completes() {
        // The close path the catalog's close_all reaches
        // (`factory.close(handle)`): create a mem client, close it, and
        // require Ok — proving `invalidate()` ran and completed. On the
        // auth-free mem tier the revocation itself is unobservable
        // (hygiene); observable release is the remote tiers' concern,
        // exercised by the existing remote component tests.
        let factory = SurrealDbPoolFactory;
        let config = make_mem_config();
        let inner = factory
            .create(&config)
            .await
            .expect("mem:// create must succeed");
        // The catalog wraps the factory's inner into a named handle
        // (`DatasourceHandle::new`) before close_all reaches
        // `factory.close(handle)`; mirror that wrapping exactly.
        let handle = camel_api::datasource::DatasourceHandle::new(
            "statedb".into(),
            factory.name().into(),
            inner,
        );
        factory
            .close(&handle)
            .await
            .expect("close must complete Ok — invalidate() ran on the mem client");
        // Re-run safety per the `PoolFactory::close` idempotency
        // contract: a second close over the same auth-free client must
        // still complete Ok.
        factory
            .close(&handle)
            .await
            .expect("close must stay safe to re-run");
    }

    #[tokio::test]
    async fn remote_scheme_still_requires_credentials() {
        let factory = SurrealDbPoolFactory;
        // ws:// config with namespace/database but missing username/password:
        // the remote auth contract is unchanged — missing extras fail before
        // any connection is attempted.
        let mut extra = HashMap::new();
        extra.insert("namespace".into(), TomlValue::String("test_ns".into()));
        extra.insert("database".into(), TomlValue::String("test_db".into()));
        let config = DatasourceConfig {
            db_url: "ws://localhost:8000".to_string(),
            provider: Some("surrealdb".into()),
            max_connections: None,
            min_connections: None,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
            extra,
        };
        let err = factory
            .create(&config)
            .await
            .expect_err("ws:// create must fail without credentials");
        assert!(
            err.to_string().contains("username"),
            "missing-extra error must name the field: {err}"
        );
    }
}
