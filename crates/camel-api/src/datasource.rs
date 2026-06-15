use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::CamelError;
use crate::lifecycle::HealthStatus;

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DatasourceConfig {
    pub db_url: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub max_connections: Option<u32>,
    #[serde(default)]
    pub min_connections: Option<u32>,
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    #[serde(default)]
    pub max_lifetime_secs: Option<u64>,
    #[serde(default)]
    pub ssl_mode: Option<String>,
    #[serde(default)]
    pub ssl_root_cert: Option<String>,
    #[serde(default)]
    pub ssl_cert: Option<String>,
    #[serde(default)]
    pub ssl_key: Option<String>,
    /// Generic key-value pairs for database-specific configuration.
    /// Components read their bespoke fields from here.
    /// SQL ignores this; SurrealDB reads namespace/database/username/password.
    #[serde(default)]
    pub extra: HashMap<String, toml::Value>,
}

impl fmt::Debug for DatasourceConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatasourceConfig")
            .field("db_url", &"[REDACTED]")
            .field("provider", &self.provider)
            .field("max_connections", &self.max_connections)
            .field("min_connections", &self.min_connections)
            .field("idle_timeout_secs", &self.idle_timeout_secs)
            .field("max_lifetime_secs", &self.max_lifetime_secs)
            .field("ssl_mode", &self.ssl_mode)
            .field("ssl_root_cert", &self.ssl_root_cert)
            .field("ssl_cert", &self.ssl_cert)
            .field("ssl_key", &self.ssl_key.as_ref().map(|_| "[REDACTED]"))
            .field("extra", &"[REDACTED]")
            .finish()
    }
}

impl DatasourceConfig {
    pub fn validate(&self) -> Result<(), CamelError> {
        if self.db_url.trim().is_empty() {
            return Err(CamelError::Config(
                "datasource db_url cannot be empty".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct DatasourceHandle {
    pub name: String,
    pub provider: String,
    inner: Arc<dyn Any + Send + Sync>,
}

impl DatasourceHandle {
    pub fn new(name: String, provider: String, inner: Arc<dyn Any + Send + Sync>) -> Self {
        Self {
            name,
            provider,
            inner,
        }
    }

    pub fn downcast<T: 'static + Send + Sync>(&self) -> Result<Arc<T>, CamelError> {
        self.inner.clone().downcast::<T>().map_err(|_| {
            CamelError::ProcessorError(format!(
                "datasource '{}' (provider '{}'): failed to downcast handle",
                self.name, self.provider
            ))
        })
    }
}

impl fmt::Debug for DatasourceHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatasourceHandle")
            .field("name", &self.name)
            .field("provider", &self.provider)
            .finish()
    }
}

#[doc(hidden)]
pub struct ResourceRef {
    pub kind: String,
    pub name: String,
}

impl fmt::Debug for ResourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResourceRef")
            .field("kind", &self.kind)
            .field("name", &self.name)
            .finish()
    }
}

pub type CreatePoolResult = Result<Arc<dyn Any + Send + Sync>, CamelError>;
pub type CreatePoolFuture<'a> = Pin<Box<dyn Future<Output = CreatePoolResult> + Send + 'a>>;
pub type CheckFuture<'a> = Pin<Box<dyn Future<Output = HealthStatus> + Send + 'a>>;

pub trait PoolFactory: Send + Sync + 'static {
    fn create<'a>(&'a self, config: &'a DatasourceConfig) -> CreatePoolFuture<'a>;

    fn check<'a>(&'a self, handle: &'a DatasourceHandle) -> CheckFuture<'a>;

    fn supported_schemes(&self) -> &[&str];

    fn matches(&self, config: &DatasourceConfig) -> bool {
        self.supported_schemes().iter().any(|s| {
            config.db_url.starts_with(&format!("{}://", s))
                || config.db_url.starts_with(&format!("{}::", s))
        })
    }

    fn name(&self) -> &'static str;
}

pub type GetPoolFuture<'a> =
    Pin<Box<dyn Future<Output = Result<DatasourceHandle, CamelError>> + Send + 'a>>;

pub trait DatasourceCatalog: Send + Sync {
    fn get_config(&self, name: &str) -> Option<DatasourceConfig>;
    fn get_pool<'a>(&'a self, name: &'a str) -> GetPoolFuture<'a>;
    fn register_factory(&self, kind: &str, factory: Arc<dyn PoolFactory>)
    -> Result<(), CamelError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datasource_config_validate_rejects_empty_db_url() {
        let config = DatasourceConfig {
            db_url: "".to_string(),
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
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn datasource_config_validate_accepts_valid() {
        let config = DatasourceConfig {
            db_url: "postgresql://localhost:5432/mydb".to_string(),
            provider: None,
            max_connections: Some(10),
            min_connections: Some(2),
            idle_timeout_secs: Some(300),
            max_lifetime_secs: Some(1800),
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
            extra: HashMap::new(),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn datasource_config_debug_redacts_db_url() {
        let config = DatasourceConfig {
            db_url: "postgresql://user:pass@localhost:5432/mydb".to_string(),
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
        };
        let debug_str = format!("{:?}", config);
        assert!(
            debug_str.contains("[REDACTED]"),
            "Debug output should redact db_url: {}",
            debug_str
        );
        assert!(
            !debug_str.contains("user:pass"),
            "Debug output should not contain credentials: {}",
            debug_str
        );
    }

    #[test]
    fn datasource_handle_downcast_fails_on_wrong_type() {
        let handle = DatasourceHandle::new("test".to_string(), "mock".to_string(), Arc::new(42u32));
        let result: Result<Arc<String>, CamelError> = handle.downcast();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("failed to downcast"));
    }

    #[test]
    fn pool_factory_matches_by_scheme() {
        struct PostgresFactory;
        impl PoolFactory for PostgresFactory {
            fn create<'a>(&'a self, _config: &'a DatasourceConfig) -> CreatePoolFuture<'a> {
                Box::pin(async { Ok(Arc::new("pool") as Arc<dyn Any + Send + Sync>) })
            }
            fn check<'a>(&'a self, _handle: &'a DatasourceHandle) -> CheckFuture<'a> {
                Box::pin(async { HealthStatus::Healthy })
            }
            fn supported_schemes(&self) -> &[&str] {
                &["postgresql", "postgres"]
            }
            fn name(&self) -> &'static str {
                "postgres"
            }
        }

        let factory = PostgresFactory;
        let pg_config = DatasourceConfig {
            db_url: "postgresql://localhost/mydb".to_string(),
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
        };
        assert!(factory.matches(&pg_config));

        let mysql_config = DatasourceConfig {
            db_url: "mysql://localhost/mydb".to_string(),
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
        };
        assert!(!factory.matches(&mysql_config));
    }

    #[test]
    fn datasource_config_extra_defaults_empty() {
        let config = DatasourceConfig {
            db_url: "ws://localhost:8000".to_string(),
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
        };
        assert!(config.extra.is_empty());
    }

    #[test]
    fn datasource_config_extra_deserializes_from_toml() {
        let toml_str = r#"
db_url = "ws://localhost:8000"
provider = "surrealdb"

[extra]
namespace = "camel"
database = "runtime"
"#;
        let config: DatasourceConfig = toml::from_str(toml_str).unwrap(); // allow-unwrap
        assert_eq!(config.db_url, "ws://localhost:8000");
        assert_eq!(config.provider.as_deref(), Some("surrealdb"));
        assert_eq!(config.extra.len(), 2);
        assert_eq!(
            config.extra.get("namespace").and_then(|v| v.as_str()),
            Some("camel")
        );
    }

    #[test]
    fn datasource_config_extra_backward_compat_without_extra_block() {
        let toml_str = r#"
db_url = "ws://localhost:8000"
"#;
        let config: DatasourceConfig = toml::from_str(toml_str).unwrap(); // allow-unwrap
        assert_eq!(config.db_url, "ws://localhost:8000");
        assert!(
            config.extra.is_empty(),
            "TOML without [extra] must deserialize with empty extra (#[serde(default)])"
        );
    }

    #[test]
    fn datasource_config_debug_redacts_extra() {
        let mut extra = HashMap::new();
        extra.insert(
            "password".to_string(),
            toml::Value::String("secret123".to_string()),
        );
        let config = DatasourceConfig {
            db_url: "ws://localhost:8000".to_string(),
            provider: None,
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
        let debug_str = format!("{:?}", config);
        assert!(
            debug_str.contains("[REDACTED]"),
            "extra should be redacted in Debug: {}",
            debug_str
        );
        assert!(
            !debug_str.contains("secret123"),
            "password must not appear in Debug: {}",
            debug_str
        );
    }
}
