//! SurrealDB PoolFactory — creates Surreal<Any> clients and registers with the datasource catalog.

use std::any::Any as StdAny;
use std::sync::Arc;

use camel_api::datasource::{CheckFuture, CreatePoolFuture, DatasourceConfig, PoolFactory};
use camel_api::lifecycle::HealthStatus;
use surrealdb::Surreal;
use surrealdb::engine::any::Any as SurrealAny;
use surrealdb::engine::any::connect;
use surrealdb::opt::auth::Root;

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
    config
        .extra
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            camel_api::CamelError::Config(format!(
                "datasource extra field '{key}' is required for surrealdb"
            ))
        })
}

/// PoolFactory for SurrealDB. Creates a `Surreal<Any>` client per datasource.
///
/// Auth order (per SDK examples + spike): connect → signin → use_ns → use_db.
/// Root fields are `String` (v3 SDK), not `&str`.
pub struct SurrealDbPoolFactory;

impl PoolFactory for SurrealDbPoolFactory {
    fn create<'a>(&'a self, config: &'a DatasourceConfig) -> CreatePoolFuture<'a> {
        Box::pin(async move {
            let endpoint = &config.db_url;
            let ns = extra_str(config, "namespace")?;
            let db = extra_str(config, "database")?;
            let user = extra_str(config, "username")?;
            let pass = extra_str(config, "password")?;

            // SDK-documented connection path: connect → signin → use_ns → use_db
            let client: Surreal<SurrealAny> = connect(endpoint).await.map_err(|e| {
                camel_api::CamelError::ProcessorError(format!(
                    "failed to create surrealdb datasource pool ({}): {e}",
                    redact_db_url(endpoint)
                ))
            })?;

            // Auth (Root fields are String in v3 SDK — clone)
            client
                .signin(Root {
                    username: user.clone(),
                    password: pass.clone(),
                })
                .await
                .map_err(|e| {
                    camel_api::CamelError::ProcessorError(format!(
                        "surrealdb signin failed for endpoint {}: {e}",
                        redact_db_url(endpoint)
                    ))
                })?;

            client.use_ns(&ns).await.map_err(|e| {
                camel_api::CamelError::ProcessorError(format!(
                    "surrealdb use_ns failed for endpoint {}: {e}",
                    redact_db_url(endpoint)
                ))
            })?;

            client.use_db(&db).await.map_err(|e| {
                camel_api::CamelError::ProcessorError(format!(
                    "surrealdb use_db failed for endpoint {}: {e}",
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

    fn supported_schemes(&self) -> &[&str] {
        &["ws", "wss", "http", "https"]
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
}
