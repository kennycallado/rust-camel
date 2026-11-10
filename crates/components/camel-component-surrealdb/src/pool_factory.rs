//! SurrealDB PoolFactory — creates Surreal\<Any\> clients and registers with the datasource catalog.

use std::any::Any as StdAny;
use std::collections::HashMap;
use std::future::IntoFuture;
use std::sync::Arc;
use std::time::Duration;

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

/// Datasource-scoped deadlines for the SDK calls the component owns.
///
/// Parsed from the datasource `extra` map as TOML integers in milliseconds
/// (`connect_timeout_ms`, `operation_timeout_ms`). Both are connection/server
/// concerns, so they live on the datasource (which already owns the URL and
/// credentials) rather than on the endpoint URI.
#[derive(Debug)]
pub(crate) struct SurrealDbTimeouts {
    /// Bound for one connect attempt in the factory's `create`.
    pub connect: Duration,
    /// Bound for one post-connect operation (signin, namespace/database
    /// selection, statement execution, live subscription).
    pub operation: Duration,
}

impl SurrealDbTimeouts {
    /// Default connect bound: 10s (workspace precedent for connect).
    pub(crate) const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 10_000;
    /// Default per-operation bound: 30s (workspace precedent for a request).
    pub(crate) const DEFAULT_OPERATION_TIMEOUT_MS: u64 = 30_000;

    /// Parses the timeout extras, applying the defaults for absent keys.
    ///
    /// A present key MUST be a positive TOML integer (milliseconds). Zero,
    /// negative, and non-integer values (including strings and booleans) are
    /// rejected with an error naming the offending option. There is no upper
    /// cap: a very long bound is a deliberate operator choice.
    pub(crate) fn from_extra(
        extra: &HashMap<String, toml::Value>,
    ) -> Result<Self, camel_api::CamelError> {
        Ok(Self {
            connect: Self::parse_ms(
                extra,
                "connect_timeout_ms",
                Self::DEFAULT_CONNECT_TIMEOUT_MS,
            )?,
            operation: Self::parse_ms(
                extra,
                "operation_timeout_ms",
                Self::DEFAULT_OPERATION_TIMEOUT_MS,
            )?,
        })
    }

    /// Parses one millisecond bound, falling back to `default_ms` when absent.
    fn parse_ms(
        extra: &HashMap<String, toml::Value>,
        key: &str,
        default_ms: u64,
    ) -> Result<Duration, camel_api::CamelError> {
        match extra.get(key) {
            None => Ok(Duration::from_millis(default_ms)),
            Some(value) => match value.as_integer() {
                None => Err(camel_api::CamelError::Config(format!(
                    "datasource extra field '{key}' must be a positive integer \
                     (milliseconds), got {}",
                    value.type_str()
                ))),
                Some(ms) if ms <= 0 => Err(camel_api::CamelError::Config(format!(
                    "datasource extra field '{key}' must be a positive integer \
                     (milliseconds), got {ms}"
                ))),
                Some(ms) => Ok(Duration::from_millis(ms as u64)),
            },
        }
    }
}

/// Builds the SDK error a bounded setup call returns on deadline expiry.
///
/// The setup `retry_async` is typed `surrealdb::Error` and classifies via
/// [`crate::error::is_transaction_conflict`]. An internal message is not a
/// transaction conflict, so a setup timeout fails fast while the honest
/// phase+bound text survives into the outer error.
pub(crate) fn setup_timeout_error(phase: &'static str, deadline_ms: u64) -> surrealdb::Error {
    surrealdb::Error::internal(format!(
        "surrealdb {phase} exceeded the {deadline_ms}ms deadline"
    ))
}

/// Runs one health-check query under the built-in operation bound.
///
/// The `check` hook receives only a [`camel_api::datasource::DatasourceHandle`],
/// which carries no configuration, so the built-in
/// [`SurrealDbTimeouts::DEFAULT_OPERATION_TIMEOUT_MS`] applies here. Deadline
/// expiry and SDK failure both report [`HealthStatus::Unhealthy`]; only the
/// log line distinguishes them.
///
/// Parameterized over the future so the elapsed branch is testable: a real
/// client answers `INFO FOR DB` immediately on every tier, so the deadline
/// can only be exercised with a future that never resolves. `name` is the
/// datasource name echoed in the warn lines, mirroring [`invalidate_bound`].
async fn health_probe(
    fut: impl IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
    name: &str,
) -> HealthStatus {
    let deadline_ms = SurrealDbTimeouts::DEFAULT_OPERATION_TIMEOUT_MS;
    match tokio::time::timeout(Duration::from_millis(deadline_ms), fut).await {
        Ok(Ok(_)) => HealthStatus::Healthy,
        Ok(Err(e)) => {
            tracing::warn!("datasource '{}' health check failed: {}", name, e);
            HealthStatus::Unhealthy
        }
        Err(_elapsed) => {
            tracing::warn!(
                "datasource '{}' surrealdb health check exceeded the {}ms deadline",
                name,
                deadline_ms
            );
            HealthStatus::Unhealthy
        }
    }
}

/// Runs the teardown `invalidate()` call under the built-in operation bound.
///
/// Like [`health_probe`], the `close` hook carries no configuration, so the
/// built-in [`SurrealDbTimeouts::DEFAULT_OPERATION_TIMEOUT_MS`] applies.
/// Parameterized over the future for the same reason: a `mem://` client's
/// `invalidate()` returns at once, so the elapsed branch needs a future that
/// never resolves.
async fn invalidate_bound(
    fut: impl IntoFuture<Output = Result<(), surrealdb::Error>>,
    name: &str,
) -> Result<(), camel_api::CamelError> {
    let deadline_ms = SurrealDbTimeouts::DEFAULT_OPERATION_TIMEOUT_MS;
    match tokio::time::timeout(Duration::from_millis(deadline_ms), fut).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(camel_api::CamelError::ProcessorError(format!(
            "datasource '{name}': surrealdb invalidate failed: {e}"
        ))),
        Err(_elapsed) => Err(camel_api::CamelError::ProcessorError(format!(
            "datasource '{name}': surrealdb invalidate exceeded the {deadline_ms}ms deadline"
        ))),
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
            // Deadline extras are parsed first so an invalid value fails
            // before any credential lookup or connection attempt. The `?`
            // maps `CamelError::Config` straight through, naming the option.
            let timeouts = SurrealDbTimeouts::from_extra(&config.extra)?;
            let connect_ms = timeouts.connect.as_millis() as u64;
            let operation_ms = timeouts.operation.as_millis() as u64;
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
                    tokio::time::timeout(timeouts.connect, connect(endpoint))
                        .await
                        .map_err(|_| crate::error::timeout_error("connect", connect_ms))?
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
                        tokio::time::timeout(
                            timeouts.operation,
                            client.signin(Root {
                                username: username.clone(),
                                password: password.clone(),
                            }),
                        )
                        .await
                        .map_err(|_| setup_timeout_error("signin", operation_ms))??;
                    }
                    tokio::time::timeout(timeouts.operation, client.use_ns(&ns))
                        .await
                        .map_err(|_| setup_timeout_error("namespace selection", operation_ms))??;
                    tokio::time::timeout(timeouts.operation, client.use_db(&db))
                        .await
                        .map_err(|_| setup_timeout_error("database selection", operation_ms))??;
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
                Ok(client) => health_probe(client.query("INFO FOR DB"), &handle.name).await,
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
            // reports real failures only. The call runs under the built-in
            // operation bound so a wedged session cannot stall teardown.
            invalidate_bound(client.invalidate(), &handle.name).await?;
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
    use std::time::Duration;

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

    // --- timeout extras (datasource-scoped deadlines) ---

    #[test]
    fn timeouts_default_when_extras_absent() {
        let extra = HashMap::new();
        let timeouts =
            SurrealDbTimeouts::from_extra(&extra).expect("absent extras must use the defaults");
        assert_eq!(timeouts.connect, Duration::from_secs(10));
        assert_eq!(timeouts.operation, Duration::from_secs(30));
    }

    #[test]
    fn timeouts_explicit_values_used() {
        let mut extra = HashMap::new();
        extra.insert("connect_timeout_ms".into(), TomlValue::Integer(100));
        extra.insert("operation_timeout_ms".into(), TomlValue::Integer(5000));
        let timeouts =
            SurrealDbTimeouts::from_extra(&extra).expect("valid integers must be accepted");
        assert_eq!(timeouts.connect, Duration::from_millis(100));
        assert_eq!(timeouts.operation, Duration::from_millis(5000));
    }

    #[test]
    fn timeouts_zero_rejected_naming_option() {
        let mut connect_extra = HashMap::new();
        connect_extra.insert("connect_timeout_ms".into(), TomlValue::Integer(0));
        let err = SurrealDbTimeouts::from_extra(&connect_extra)
            .expect_err("connect_timeout_ms = 0 must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("connect_timeout_ms"),
            "error must name the option: {msg}"
        );
        assert!(
            msg.contains("positive integer"),
            "error must state the positive-integer rule: {msg}"
        );

        let mut operation_extra = HashMap::new();
        operation_extra.insert("operation_timeout_ms".into(), TomlValue::Integer(0));
        let err = SurrealDbTimeouts::from_extra(&operation_extra)
            .expect_err("operation_timeout_ms = 0 must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("operation_timeout_ms"),
            "error must name the option: {msg}"
        );
        assert!(
            msg.contains("positive integer"),
            "error must state the positive-integer rule: {msg}"
        );
    }

    #[test]
    fn timeouts_negative_rejected() {
        let mut extra = HashMap::new();
        extra.insert("connect_timeout_ms".into(), TomlValue::Integer(-1));
        let err = SurrealDbTimeouts::from_extra(&extra)
            .expect_err("connect_timeout_ms = -1 must be rejected");
        assert!(
            err.to_string().contains("connect_timeout_ms"),
            "error must name the option: {err}"
        );
    }

    #[test]
    fn timeouts_empty_string_rejected() {
        let mut extra = HashMap::new();
        extra.insert(
            "operation_timeout_ms".into(),
            TomlValue::String(String::new()),
        );
        let err = SurrealDbTimeouts::from_extra(&extra)
            .expect_err("operation_timeout_ms = \"\" must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("operation_timeout_ms"),
            "error must name the option: {msg}"
        );
        assert!(
            msg.contains("positive integer"),
            "error must state the positive-integer rule: {msg}"
        );
    }

    #[test]
    fn timeouts_non_numeric_rejected() {
        let mut extra = HashMap::new();
        extra.insert("operation_timeout_ms".into(), TomlValue::Boolean(true));
        let err = SurrealDbTimeouts::from_extra(&extra)
            .expect_err("operation_timeout_ms = true must be rejected");
        assert!(
            err.to_string().contains("operation_timeout_ms"),
            "error must name the option: {err}"
        );
    }

    #[test]
    fn setup_timeout_error_is_not_transaction_conflict() {
        let err = setup_timeout_error("signin", 30000);
        assert!(
            !crate::error::is_transaction_conflict(&err),
            "a setup deadline expiry must not be classified as a transaction conflict"
        );
        let msg = err.to_string();
        assert!(msg.contains("signin"), "message must name the phase: {msg}");
        assert!(
            msg.contains("30000ms"),
            "message must name the bound: {msg}"
        );
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

    // --- deadline behaviour (design sites 1, 5, 6) ---

    #[tokio::test]
    async fn factory_parse_rejects_bad_timeout_before_connecting() {
        // The timeout extras are parsed before the credential extras and
        // before any connect attempt: a rejected value must surface with no
        // network I/O (the URL below is never dialled).
        let factory = SurrealDbPoolFactory;
        let mut config = make_test_config("ws://127.0.0.1:1");
        config
            .extra
            .insert("connect_timeout_ms".into(), TomlValue::Integer(0));
        let err = factory
            .create(&config)
            .await
            .expect_err("connect_timeout_ms = 0 must be rejected before connecting");
        assert!(
            err.to_string().contains("connect_timeout_ms"),
            "error must name the rejected option: {err}"
        );
    }

    #[tokio::test]
    async fn stalled_endpoint_trips_connect_bound() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener must bind");
        let addr = listener
            .local_addr()
            .expect("a bound listener must expose its address");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept must not fail");
            // Hold the socket open: dropping it would send a FIN and abort
            // the ws handshake as a non-timeout transport error instead of
            // letting the connect bound trip.
            tokio::time::sleep(Duration::from_secs(30)).await; // allow-test-sleep: holds the accepted socket open so the ws handshake stalls (required by this test)
            drop(stream);
        });

        let mut config = make_test_config(&format!("ws://{addr}"));
        config
            .extra
            .insert("connect_timeout_ms".into(), TomlValue::Integer(100));

        let factory = SurrealDbPoolFactory;
        let started = std::time::Instant::now();
        let outcome = tokio::time::timeout(Duration::from_secs(5), factory.create(&config))
            .await
            .expect("the connect bound must trip well inside the 5s outer guard");
        let elapsed = started.elapsed();

        let err = outcome.expect_err("a stalled endpoint must not yield a client");
        assert!(
            err.to_string()
                .contains("surrealdb connect exceeded the 100ms deadline"),
            "error must name the connect phase and bound: {err}"
        );
        assert!(
            elapsed >= Duration::from_millis(100),
            "the configured bound must have elapsed, took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn stalled_connect_attempt_is_not_retried() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener must bind");
        let addr = listener
            .local_addr()
            .expect("a bound listener must expose its address");
        let accepts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let accept_counter = Arc::clone(&accepts);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept must not fail");
            accept_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // Hold the socket open so the handshake stalls (see the
            // sibling test for why the stream must not be dropped).
            tokio::time::sleep(Duration::from_secs(30)).await; // allow-test-sleep: holds the accepted socket open so the ws handshake stalls (required by this test)
            drop(stream);
        });

        let mut config = make_test_config(&format!("ws://{addr}"));
        config
            .extra
            .insert("connect_timeout_ms".into(), TomlValue::Integer(100));

        let factory = SurrealDbPoolFactory;
        let outcome = tokio::time::timeout(Duration::from_secs(5), factory.create(&config))
            .await
            .expect("the connect bound must trip well inside the 5s outer guard");
        let err = outcome.expect_err("a stalled endpoint must not yield a client");
        assert!(
            err.to_string()
                .contains("surrealdb connect exceeded the 100ms deadline"),
            "error must name the connect phase and bound: {err}"
        );
        assert_eq!(
            accepts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a timed-out connect attempt must not be retried"
        );
    }

    #[tokio::test]
    async fn health_check_timeout_maps_to_unhealthy() {
        // Real clock for `create` and the happy arm: the mem:// engine
        // hands the connect off to another thread, so a paused clock would
        // auto-advance to the 10s connect deadline before that handoff
        // completes. Only the elapsed arm needs a paused clock.
        let factory = SurrealDbPoolFactory;
        let config = make_mem_config();
        let handle = factory
            .create(&config)
            .await
            .expect("mem:// create must succeed");
        let client = handle
            .downcast::<Surreal<SurrealAny>>()
            .expect("handle must be a Surreal<Any> client");

        assert_eq!(
            health_probe(client.query("INFO FOR DB"), "test-ds").await,
            HealthStatus::Healthy,
            "a probe that completes must report Healthy"
        );

        // With the clock paused the 30s operation bound fires as soon as the
        // runtime is idle, so the pending future trips it immediately.
        tokio::time::pause();
        assert_eq!(
            health_probe(
                std::future::pending::<Result<surrealdb::IndexedResults, surrealdb::Error>>(),
                "test-ds",
            )
            .await,
            HealthStatus::Unhealthy,
            "a probe past its deadline must report Unhealthy"
        );
    }

    #[tokio::test]
    async fn invalidate_timeout_names_datasource_and_deadline() {
        // Real clock for `create` and the happy arm (see the sibling test for
        // why); paused only for the elapsed arm.
        let factory = SurrealDbPoolFactory;
        let config = make_mem_config();
        let handle = factory
            .create(&config)
            .await
            .expect("mem:// create must succeed");
        let client = handle
            .downcast::<Surreal<SurrealAny>>()
            .expect("handle must be a Surreal<Any> client");

        invalidate_bound(client.invalidate(), "statedb")
            .await
            .expect("a completing invalidate must return Ok");

        tokio::time::pause();
        let err = invalidate_bound(
            std::future::pending::<Result<(), surrealdb::Error>>(),
            "statedb",
        )
        .await
        .expect_err("a stalled invalidate must trip the deadline");
        let msg = err.to_string();
        assert!(
            msg.contains("statedb"),
            "error must name the datasource: {msg}"
        );
        assert!(
            msg.contains("invalidate exceeded the 30000ms deadline"),
            "error must name the teardown phase and bound: {msg}"
        );
    }
}
