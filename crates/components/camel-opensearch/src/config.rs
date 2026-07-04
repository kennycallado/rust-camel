use camel_component_api::CamelError;
use camel_component_api::NetworkRetryPolicy;
use camel_component_api::parse_uri;
use std::convert::TryFrom;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

/// Default timeout in milliseconds for OpenSearch operations.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

// --- OpenSearchOperation enum ---

/// OpenSearch operations supported by this component.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(try_from = "String")]
pub enum OpenSearchOperation {
    INDEX,
    SEARCH,
    GET,
    DELETE,
    EXISTS,
    UPDATE,
    BULK,
    MULTIGET,
    DELETEINDEX,
    MULTISEARCH,
    PING,
    UNKNOWN(String),
}

impl FromStr for OpenSearchOperation {
    type Err = CamelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "INDEX" => Ok(OpenSearchOperation::INDEX),
            "SEARCH" => Ok(OpenSearchOperation::SEARCH),
            "GET" => Ok(OpenSearchOperation::GET),
            "DELETE" => Ok(OpenSearchOperation::DELETE),
            "EXISTS" => Ok(OpenSearchOperation::EXISTS),
            "UPDATE" => Ok(OpenSearchOperation::UPDATE),
            "BULK" => Ok(OpenSearchOperation::BULK),
            "MULTIGET" => Ok(OpenSearchOperation::MULTIGET),
            "DELETE_INDEX" => Ok(OpenSearchOperation::DELETEINDEX),
            "MULTI_SEARCH" => Ok(OpenSearchOperation::MULTISEARCH),
            "PING" => Ok(OpenSearchOperation::PING),
            other => Err(CamelError::Config(format!(
                "unknown OpenSearch operation: {}",
                other
            ))),
        }
    }
}

impl fmt::Display for OpenSearchOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenSearchOperation::INDEX => write!(f, "INDEX"),
            OpenSearchOperation::SEARCH => write!(f, "SEARCH"),
            OpenSearchOperation::GET => write!(f, "GET"),
            OpenSearchOperation::DELETE => write!(f, "DELETE"),
            OpenSearchOperation::EXISTS => write!(f, "EXISTS"),
            OpenSearchOperation::UPDATE => write!(f, "UPDATE"),
            OpenSearchOperation::BULK => write!(f, "BULK"),
            OpenSearchOperation::MULTIGET => write!(f, "MULTIGET"),
            OpenSearchOperation::DELETEINDEX => write!(f, "DELETE_INDEX"),
            OpenSearchOperation::MULTISEARCH => write!(f, "MULTI_SEARCH"),
            OpenSearchOperation::PING => write!(f, "PING"),
            OpenSearchOperation::UNKNOWN(s) => write!(f, "{}", s),
        }
    }
}

impl TryFrom<String> for OpenSearchOperation {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        OpenSearchOperation::from_str(&s).map_err(|e| e.to_string())
    }
}

// --- OpenSearchConfig (global defaults) ---

/// Global OpenSearch configuration defaults.
///
/// This struct holds component-level defaults that can be set via Camel.toml
/// and applied to endpoint configurations when specific values aren't provided.
#[derive(Clone, serde::Deserialize)]
pub struct OpenSearchConfig {
    #[serde(default = "OpenSearchConfig::default_host")]
    pub host: String,
    #[serde(default = "OpenSearchConfig::default_port")]
    pub port: u16,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub default_operation: Option<OpenSearchOperation>,
    #[serde(default)]
    pub index_name: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub max_bulk_bytes: Option<usize>,
    #[serde(default)]
    pub size: Option<u32>,
    #[serde(default)]
    pub from: Option<u32>,
    /// Retry policy for transient HTTP failures (5xx, network errors).
    #[serde(default)]
    pub retry: NetworkRetryPolicy,
}

impl OpenSearchConfig {
    fn default_host() -> String {
        "localhost".to_string()
    }

    fn default_port() -> u16 {
        9200
    }

    pub fn with_host(mut self, v: impl Into<String>) -> Self {
        self.host = v.into();
        self
    }

    pub fn with_port(mut self, v: u16) -> Self {
        self.port = v;
        self
    }

    pub fn with_default_operation(mut self, v: OpenSearchOperation) -> Self {
        self.default_operation = Some(v);
        self
    }

    pub fn with_username(mut self, v: impl Into<String>) -> Self {
        self.username = Some(v.into());
        self
    }

    pub fn with_password(mut self, v: impl Into<String>) -> Self {
        self.password = Some(v.into());
        self
    }

    pub fn with_index_name(mut self, v: impl Into<String>) -> Self {
        self.index_name = Some(v.into());
        self
    }

    pub fn with_timeout_ms(mut self, v: u64) -> Self {
        self.timeout_ms = Some(v);
        self
    }

    pub fn with_max_bulk_bytes(mut self, v: usize) -> Self {
        self.max_bulk_bytes = Some(v);
        self
    }

    pub fn with_size(mut self, v: u32) -> Self {
        self.size = Some(v);
        self
    }

    pub fn with_from(mut self, v: u32) -> Self {
        self.from = Some(v);
        self
    }

    pub fn with_retry(mut self, v: NetworkRetryPolicy) -> Self {
        self.retry = v;
        self
    }
}

impl Default for OpenSearchConfig {
    fn default() -> Self {
        Self {
            host: Self::default_host(),
            port: Self::default_port(),
            username: None,
            password: None,
            default_operation: None,
            index_name: None,
            timeout_ms: Some(DEFAULT_TIMEOUT_MS),
            max_bulk_bytes: None,
            size: None,
            from: None,
            retry: NetworkRetryPolicy::default(),
        }
    }
}

impl fmt::Debug for OpenSearchConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenSearchConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("default_operation", &self.default_operation)
            .field("index_name", &self.index_name)
            .field("timeout_ms", &self.timeout_ms)
            .field("max_bulk_bytes", &self.max_bulk_bytes)
            .field("size", &self.size)
            .field("from", &self.from)
            .field("retry", &self.retry)
            .finish()
    }
}

// --- OpenSearchEndpointConfig (parsed from URI) ---

/// Configuration parsed from an OpenSearch URI.
///
/// Format: `opensearch://host:port/index?operation=INDEX&username=X&password=Y`
/// or `opensearchs://host:port/index?operation=SEARCH` (TLS enabled).
///
/// # Fields with Global Defaults (Option<T>)
///
/// These fields can be set via global defaults in Camel.toml. They are `Option<T>`
/// to distinguish between "not set by URI" (`None`) and "explicitly set by URI" (`Some(v)`).
/// After calling `merge_with_global()`, all are guaranteed `Some`.
///
/// - `host` - OpenSearch server hostname
/// - `port` - OpenSearch server port
/// - `username` - Username for authentication
/// - `password` - Password for authentication
///
/// # Fields Without Global Defaults
///
/// These fields are per-endpoint only:
///
/// - `index_name` - Target index name (required)
/// - `operation` - OpenSearch operation to perform (default: SEARCH)
/// - `is_tls` - Whether to use HTTPS (determined by scheme: `opensearchs` = true)
#[derive(Clone)]
#[non_exhaustive]
pub struct OpenSearchEndpointConfig {
    /// OpenSearch server hostname. `None` if not set in URI.
    pub host: Option<String>,

    /// OpenSearch server port. `None` if not set in URI.
    pub port: Option<u16>,

    /// Username for authentication. Default: None.
    pub username: Option<String>,

    /// Password for authentication. Default: None.
    pub password: Option<String>,

    /// Target index name. Required.
    pub index_name: String,

    /// OpenSearch operation to perform. Default: SEARCH.
    pub operation: OpenSearchOperation,

    /// Whether to use HTTPS. Determined by scheme (`opensearchs` = true).
    pub is_tls: bool,

    /// Request timeout in milliseconds.
    pub timeout_ms: Option<u64>,

    /// Maximum serialized bulk payload size in bytes.
    pub max_bulk_bytes: Option<usize>,

    /// Search result size (pagination page size).
    pub size: Option<u32>,

    /// Search result offset (pagination start).
    pub from: Option<u32>,

    /// Retry policy for transient HTTP failures (5xx, network errors).
    pub retry: NetworkRetryPolicy,

    /// Whether `retry` was explicitly set via URI params. Used by
    /// [`merge_with_global`] to decide whether URI values win over
    /// the global config. Internal tracking flag, not serialized.
    retry_set_from_uri: bool,
}

impl fmt::Debug for OpenSearchEndpointConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenSearchEndpointConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("index_name", &self.index_name)
            .field("operation", &self.operation)
            .field("is_tls", &self.is_tls)
            .field("timeout_ms", &self.timeout_ms)
            .field("max_bulk_bytes", &self.max_bulk_bytes)
            .field("size", &self.size)
            .field("from", &self.from)
            .field("retry", &self.retry)
            .finish()
    }
}

impl OpenSearchEndpointConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;

        let is_tls = parts.scheme == "opensearchs";

        if parts.scheme != "opensearch" && parts.scheme != "opensearchs" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'opensearch' or 'opensearchs', got '{}'",
                parts.scheme
            )));
        }

        // Parse host and port from path (format: //host:port/index or //host/index or //host:port)
        let (host, port, path_remainder) = if parts.path.starts_with("//") {
            let path = &parts.path[2..]; // Remove leading //
            if path.is_empty() {
                // opensearch://?operation=SEARCH → no host, no port
                (None, None, "")
            } else {
                // Split on '/' to separate host:port from index path
                let (authority, remainder) = match path.split_once('/') {
                    Some((auth, rem)) => (auth, rem),
                    None => (path, ""),
                };

                let (host_part, port_part) = match authority.split_once(':') {
                    Some((h, p)) => (h, Some(p)),
                    None => (authority, None),
                };

                let host = if host_part.is_empty() {
                    None
                } else {
                    Some(host_part.to_string())
                };
                let port = port_part.and_then(|p| p.parse().ok());
                (host, port, remainder)
            }
        } else {
            (None, None, parts.path.as_str())
        };

        // First non-empty path segment is the index_name
        let index_name = path_remainder
            .split('/')
            .find(|s| !s.is_empty())
            .ok_or_else(|| CamelError::InvalidUri("missing index name in URI path".to_string()))?
            .to_string();

        // Validate index name against OpenSearch naming rules
        if index_name.contains('\0') {
            return Err(CamelError::InvalidUri(
                "index name must not contain null bytes".into(),
            ));
        }
        if index_name.contains("..") {
            return Err(CamelError::InvalidUri(
                "index name must not contain '..'".into(),
            ));
        }
        if !index_name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            return Err(CamelError::InvalidUri(
                "index name must contain only lowercase letters, digits, hyphens, and underscores"
                    .into(),
            ));
        }
        if index_name.len() > 255 {
            return Err(CamelError::InvalidUri(
                "index name must be at most 255 bytes".into(),
            ));
        }

        // Parse operation (default to SEARCH)
        let operation = parts
            .params
            .get("operation")
            .map(|s| OpenSearchOperation::from_str(s))
            .transpose()?
            .unwrap_or(OpenSearchOperation::SEARCH);

        // Parse username and password
        let username = parts.params.get("username").cloned();
        let password = parts.params.get("password").cloned();
        let timeout_ms = parts
            .params
            .get("timeout_ms")
            .map(|v| {
                v.parse::<u64>().map_err(|_| {
                    CamelError::InvalidUri("timeout_ms must be a valid u64".to_string())
                })
            })
            .transpose()?;
        let max_bulk_bytes = parts
            .params
            .get("max_bulk_bytes")
            .map(|v| {
                v.parse::<usize>().map_err(|_| {
                    CamelError::InvalidUri("max_bulk_bytes must be a valid usize".to_string())
                })
            })
            .transpose()?;
        let size = parts
            .params
            .get("size")
            .map(|v| {
                v.parse::<u32>()
                    .map_err(|_| CamelError::InvalidUri("size must be a valid u32".to_string()))
            })
            .transpose()?;
        let from = parts
            .params
            .get("from")
            .map(|v| {
                v.parse::<u32>()
                    .map_err(|_| CamelError::InvalidUri("from must be a valid u32".to_string()))
            })
            .transpose()?;

        // Parse retry policy from URI params
        let mut retry = NetworkRetryPolicy::default();
        let mut retry_set_from_uri = false;
        if let Some(raw) = parts.params.get("retryEnabled") {
            retry.enabled = raw.parse::<bool>().map_err(|_| {
                CamelError::InvalidUri(format!("retryEnabled must be a boolean, got '{raw}'"))
            })?;
            retry_set_from_uri = true;
        }
        if let Some(raw) = parts.params.get("retryMaxAttempts") {
            retry.max_attempts = raw.parse::<u32>().map_err(|_| {
                CamelError::InvalidUri(format!("retryMaxAttempts must be a u32, got '{raw}'"))
            })?;
            retry_set_from_uri = true;
        }
        if let Some(raw) = parts.params.get("retryInitialDelayMs") {
            retry.initial_delay = Duration::from_millis(raw.parse::<u64>().map_err(|_| {
                CamelError::InvalidUri(format!("retryInitialDelayMs must be a u64, got '{raw}'"))
            })?);
            retry_set_from_uri = true;
        }
        if let Some(raw) = parts.params.get("retryMultiplier") {
            retry.multiplier = raw.parse::<f64>().map_err(|_| {
                CamelError::InvalidUri(format!("retryMultiplier must be a f64, got '{raw}'"))
            })?;
            retry_set_from_uri = true;
        }
        if let Some(raw) = parts.params.get("retryMaxDelayMs") {
            retry.max_delay = Duration::from_millis(raw.parse::<u64>().map_err(|_| {
                CamelError::InvalidUri(format!("retryMaxDelayMs must be a u64, got '{raw}'"))
            })?);
            retry_set_from_uri = true;
        }
        if let Some(raw) = parts.params.get("retryJitter") {
            retry.jitter_factor = raw.parse::<f64>().map_err(|_| {
                CamelError::InvalidUri(format!("retryJitter must be a f64, got '{raw}'"))
            })?;
            retry_set_from_uri = true;
        }

        Ok(Self {
            host,
            port,
            username,
            password,
            index_name,
            operation,
            is_tls,
            timeout_ms,
            max_bulk_bytes,
            size,
            from,
            retry,
            retry_set_from_uri,
        })
    }

    /// Merge with global defaults.
    ///
    /// This method fills in `None` fields from the provided `OpenSearchConfig`.
    /// It's intended to be called after parsing a URI when global component
    /// defaults should be applied.
    pub fn merge_with_global(&self, global: &OpenSearchConfig) -> Self {
        let operation = match &self.operation {
            OpenSearchOperation::UNKNOWN(_) => global
                .default_operation
                .clone()
                .unwrap_or(OpenSearchOperation::SEARCH),
            op => op.clone(),
        };
        Self {
            host: self.host.clone().or_else(|| Some(global.host.clone())),
            port: self.port.or(Some(global.port)),
            username: self.username.clone().or_else(|| global.username.clone()),
            password: self.password.clone().or_else(|| global.password.clone()),
            index_name: if self.index_name.is_empty() {
                global.index_name.clone().unwrap_or_default()
            } else {
                self.index_name.clone()
            },
            operation,
            is_tls: self.is_tls,
            timeout_ms: self.timeout_ms.or(global.timeout_ms),
            max_bulk_bytes: self.max_bulk_bytes.or(global.max_bulk_bytes),
            size: self.size.or(global.size),
            from: self.from.or(global.from),
            retry: if self.retry_set_from_uri {
                self.retry.clone()
            } else {
                global.retry.clone()
            },
            retry_set_from_uri: self.retry_set_from_uri,
        }
    }

    pub fn with_host(mut self, v: impl Into<String>) -> Self {
        self.host = Some(v.into());
        self
    }

    pub fn with_port(mut self, v: u16) -> Self {
        self.port = Some(v);
        self
    }

    pub fn with_username(mut self, v: impl Into<String>) -> Self {
        self.username = Some(v.into());
        self
    }

    pub fn with_password(mut self, v: impl Into<String>) -> Self {
        self.password = Some(v.into());
        self
    }

    pub fn with_operation(mut self, v: OpenSearchOperation) -> Self {
        self.operation = v;
        self
    }

    pub fn with_timeout_ms(mut self, v: u64) -> Self {
        self.timeout_ms = Some(v);
        self
    }

    pub fn with_max_bulk_bytes(mut self, v: usize) -> Self {
        self.max_bulk_bytes = Some(v);
        self
    }

    pub fn with_size(mut self, v: u32) -> Self {
        self.size = Some(v);
        self
    }

    pub fn with_from(mut self, v: u32) -> Self {
        self.from = Some(v);
        self
    }

    /// Build the base URL for the OpenSearch connection.
    ///
    /// Returns `http://host:port` or `https://host:port`.
    /// Uses fallback values of "localhost" and 9200 if host/port are `None`.
    pub fn base_url(&self) -> String {
        let scheme = if self.is_tls { "https" } else { "http" };
        let host = self.host.as_deref().unwrap_or("localhost");
        let port = self.port.unwrap_or(9200);
        format!("{}://{}:{}", scheme, host, port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- OpenSearchOperation tests ---

    #[test]
    fn test_operation_from_str() {
        assert_eq!(
            OpenSearchOperation::from_str("INDEX").unwrap(),
            OpenSearchOperation::INDEX
        );
        assert_eq!(
            OpenSearchOperation::from_str("SEARCH").unwrap(),
            OpenSearchOperation::SEARCH
        );
        assert_eq!(
            OpenSearchOperation::from_str("GET").unwrap(),
            OpenSearchOperation::GET
        );
        assert_eq!(
            OpenSearchOperation::from_str("DELETE").unwrap(),
            OpenSearchOperation::DELETE
        );
        assert_eq!(
            OpenSearchOperation::from_str("EXISTS").unwrap(),
            OpenSearchOperation::EXISTS
        );
        assert_eq!(
            OpenSearchOperation::from_str("UPDATE").unwrap(),
            OpenSearchOperation::UPDATE
        );
        assert_eq!(
            OpenSearchOperation::from_str("BULK").unwrap(),
            OpenSearchOperation::BULK
        );
        assert_eq!(
            OpenSearchOperation::from_str("MULTIGET").unwrap(),
            OpenSearchOperation::MULTIGET
        );
        assert_eq!(
            OpenSearchOperation::from_str("DELETE_INDEX").unwrap(),
            OpenSearchOperation::DELETEINDEX
        );
        assert_eq!(
            OpenSearchOperation::from_str("MULTI_SEARCH").unwrap(),
            OpenSearchOperation::MULTISEARCH
        );
        assert_eq!(
            OpenSearchOperation::from_str("PING").unwrap(),
            OpenSearchOperation::PING
        );
        // Case insensitive
        assert_eq!(
            OpenSearchOperation::from_str("index").unwrap(),
            OpenSearchOperation::INDEX
        );
        assert_eq!(
            OpenSearchOperation::from_str("Search").unwrap(),
            OpenSearchOperation::SEARCH
        );
        // Unknown operations are rejected at config time
        let err = OpenSearchOperation::from_str("CUSTOM_OP").unwrap_err();
        assert!(
            err.to_string().contains("unknown OpenSearch operation"),
            "expected unknown operation error, got: {}",
            err
        );
    }

    #[test]
    fn test_operation_display() {
        assert_eq!(OpenSearchOperation::INDEX.to_string(), "INDEX");
        assert_eq!(OpenSearchOperation::SEARCH.to_string(), "SEARCH");
        assert_eq!(OpenSearchOperation::GET.to_string(), "GET");
        assert_eq!(OpenSearchOperation::DELETE.to_string(), "DELETE");
        assert_eq!(OpenSearchOperation::EXISTS.to_string(), "EXISTS");
        assert_eq!(OpenSearchOperation::UPDATE.to_string(), "UPDATE");
        assert_eq!(OpenSearchOperation::BULK.to_string(), "BULK");
        assert_eq!(OpenSearchOperation::MULTIGET.to_string(), "MULTIGET");
        assert_eq!(OpenSearchOperation::DELETEINDEX.to_string(), "DELETE_INDEX");
        assert_eq!(OpenSearchOperation::MULTISEARCH.to_string(), "MULTI_SEARCH");
        assert_eq!(OpenSearchOperation::PING.to_string(), "PING");
        assert_eq!(
            OpenSearchOperation::UNKNOWN("CUSTOM".to_string()).to_string(),
            "CUSTOM"
        );

        // Roundtrip
        for s in &[
            "INDEX",
            "SEARCH",
            "GET",
            "DELETE",
            "EXISTS",
            "UPDATE",
            "BULK",
            "MULTIGET",
            "DELETE_INDEX",
            "MULTI_SEARCH",
            "PING",
        ] {
            let op = OpenSearchOperation::from_str(s).unwrap();
            assert_eq!(op.to_string(), *s);
        }
    }

    // --- OpenSearchEndpointConfig tests ---

    #[test]
    fn test_endpoint_config_basic() {
        let cfg = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/myindex?operation=INDEX",
        )
        .unwrap();
        assert_eq!(cfg.host, Some("localhost".to_string()));
        assert_eq!(cfg.port, Some(9200));
        assert_eq!(cfg.index_name, "myindex");
        assert_eq!(cfg.operation, OpenSearchOperation::INDEX);
        assert!(!cfg.is_tls);
    }

    #[test]
    fn test_endpoint_config_https() {
        let cfg = OpenSearchEndpointConfig::from_uri("opensearchs://host:443/idx?operation=SEARCH")
            .unwrap();
        assert_eq!(cfg.host, Some("host".to_string()));
        assert_eq!(cfg.port, Some(443));
        assert_eq!(cfg.index_name, "idx");
        assert_eq!(cfg.operation, OpenSearchOperation::SEARCH);
        assert!(cfg.is_tls);
    }

    #[test]
    fn test_endpoint_config_defaults() {
        // operation defaults to SEARCH when not specified
        let cfg =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/myindex").unwrap();
        assert_eq!(cfg.operation, OpenSearchOperation::SEARCH);
    }

    #[test]
    fn test_endpoint_config_with_auth() {
        let cfg = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/myindex?operation=GET&username=admin&password=secret",
        )
        .unwrap();
        assert_eq!(cfg.username, Some("admin".to_string()));
        assert_eq!(cfg.password, Some("secret".to_string()));
        assert_eq!(cfg.operation, OpenSearchOperation::GET);
    }

    #[test]
    fn test_endpoint_config_with_search_pagination() {
        let cfg = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/myindex?operation=SEARCH&size=25&from=100",
        )
        .unwrap();
        assert_eq!(cfg.size, Some(25));
        assert_eq!(cfg.from, Some(100));
    }

    #[test]
    fn test_endpoint_config_host_only_no_port() {
        let cfg =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost/myindex?operation=GET")
                .unwrap();
        assert_eq!(cfg.host, Some("localhost".to_string()));
        assert_eq!(cfg.port, None);
        assert_eq!(cfg.operation, OpenSearchOperation::GET);
    }

    #[test]
    fn test_endpoint_config_wrong_scheme() {
        let result = OpenSearchEndpointConfig::from_uri("http://localhost:9200/myindex");
        assert!(result.is_err());
    }

    #[test]
    fn test_endpoint_config_missing_index() {
        let result = OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200");
        assert!(result.is_err());
    }

    // --- merge_with_global tests ---

    #[test]
    fn test_merge_with_global() {
        let ep = OpenSearchEndpointConfig::from_uri("opensearch://localhost/myindex?operation=GET")
            .unwrap();
        assert_eq!(ep.host, Some("localhost".to_string()));
        assert_eq!(ep.port, None);
        assert_eq!(ep.username, None);
        assert_eq!(ep.password, None);

        let global = OpenSearchConfig::default()
            .with_port(9200)
            .with_host("global-host")
            .with_default_operation(OpenSearchOperation::SEARCH);

        let merged = ep.merge_with_global(&global);

        assert_eq!(merged.host, Some("localhost".to_string()));
        assert_eq!(merged.port, Some(9200));
        assert_eq!(merged.username, None);
        assert_eq!(merged.password, None);
        assert_eq!(merged.operation, OpenSearchOperation::GET);
    }

    #[test]
    fn test_merge_with_global_fills_all_nones() {
        let ep =
            OpenSearchEndpointConfig::from_uri("opensearch:///myindex?operation=SEARCH").unwrap();
        assert_eq!(ep.host, None);
        assert_eq!(ep.port, None);
        assert_eq!(ep.index_name, "myindex");

        let global = OpenSearchConfig::default()
            .with_host("es-server")
            .with_port(9300)
            .with_username("admin")
            .with_password("secret");

        let merged = ep.merge_with_global(&global);
        assert_eq!(merged.host, Some("es-server".to_string()));
        assert_eq!(merged.port, Some(9300));
        assert_eq!(merged.username, Some("admin".to_string()));
        assert_eq!(merged.password, Some("secret".to_string()));
    }

    #[test]
    fn test_merge_with_global_default_operation_fallback() {
        // Build an endpoint config programmatically with UNKNOWN operation to test
        // the merge_with_global fallback path.
        let ep = OpenSearchEndpointConfig {
            host: Some("localhost".to_string()),
            port: Some(9200),
            username: None,
            password: None,
            index_name: "myindex".to_string(),
            operation: OpenSearchOperation::UNKNOWN("UNKNOWN_OP".to_string()),
            is_tls: false,
            timeout_ms: None,
            max_bulk_bytes: None,
            size: None,
            from: None,
            retry: NetworkRetryPolicy::default(),
            retry_set_from_uri: false,
        };

        let global = OpenSearchConfig::default().with_default_operation(OpenSearchOperation::INDEX);

        let merged = ep.merge_with_global(&global);
        assert_eq!(merged.operation, OpenSearchOperation::INDEX);
    }

    // --- base_url tests ---

    #[test]
    fn test_base_url_http() {
        let cfg = OpenSearchEndpointConfig::from_uri(
            "opensearch://es-host:9200/myindex?operation=SEARCH",
        )
        .unwrap();
        assert_eq!(cfg.base_url(), "http://es-host:9200");
    }

    #[test]
    fn test_base_url_https() {
        let cfg = OpenSearchEndpointConfig::from_uri(
            "opensearchs://es-host:443/myindex?operation=SEARCH",
        )
        .unwrap();
        assert_eq!(cfg.base_url(), "https://es-host:443");
    }

    #[test]
    fn test_base_url_defaults() {
        // No host/port → uses defaults in base_url()
        let cfg =
            OpenSearchEndpointConfig::from_uri("opensearch:///myindex?operation=SEARCH").unwrap();
        assert_eq!(cfg.host, None);
        assert_eq!(cfg.port, None);
        assert_eq!(cfg.base_url(), "http://localhost:9200");
    }

    // --- OpenSearchConfig tests ---

    #[test]
    fn test_config_defaults() {
        let cfg = OpenSearchConfig::default();
        assert_eq!(cfg.host, "localhost");
        assert_eq!(cfg.port, 9200);
        assert!(cfg.username.is_none());
        assert!(cfg.password.is_none());
        assert!(cfg.default_operation.is_none());
        assert!(cfg.index_name.is_none());
        assert_eq!(cfg.timeout_ms, Some(30_000));
        assert!(cfg.max_bulk_bytes.is_none());
        assert!(cfg.size.is_none());
        assert!(cfg.from.is_none());
    }

    #[test]
    fn test_config_builder() {
        let cfg = OpenSearchConfig::default()
            .with_host("es-prod")
            .with_port(9200)
            .with_default_operation(OpenSearchOperation::BULK)
            .with_username("admin")
            .with_password("secret")
            .with_index_name("idx")
            .with_timeout_ms(5000)
            .with_max_bulk_bytes(32_000)
            .with_size(10)
            .with_from(20);
        assert_eq!(cfg.host, "es-prod");
        assert_eq!(cfg.port, 9200);
        assert_eq!(cfg.default_operation, Some(OpenSearchOperation::BULK));
        assert_eq!(cfg.username, Some("admin".to_string()));
        assert_eq!(cfg.password, Some("secret".to_string()));
        assert_eq!(cfg.index_name, Some("idx".to_string()));
        assert_eq!(cfg.timeout_ms, Some(5000));
        assert_eq!(cfg.max_bulk_bytes, Some(32_000));
        assert_eq!(cfg.size, Some(10));
        assert_eq!(cfg.from, Some(20));
    }

    #[test]
    fn test_opensearch_config_debug_redacts_password() {
        let cfg = OpenSearchConfig::default()
            .with_host("es-prod")
            .with_password("hunter2");
        let debug_output = format!("{:?}", cfg);
        assert!(
            !debug_output.contains("hunter2"),
            "debug output must not contain the real password: {}",
            debug_output
        );
        assert!(
            debug_output.contains("<redacted>"),
            "debug output must contain <redacted>: {}",
            debug_output
        );
    }

    #[test]
    fn test_opensearch_endpoint_config_debug_redacts_password() {
        let cfg = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/myindex?operation=GET&username=admin&password=hunter2",
        )
        .unwrap();
        let debug_output = format!("{:?}", cfg);
        assert!(
            !debug_output.contains("hunter2"),
            "debug output must not contain the real password: {}",
            debug_output
        );
        assert!(
            debug_output.contains("<redacted>"),
            "debug output must contain <redacted>: {}",
            debug_output
        );
    }

    #[test]
    fn test_opensearch_config_debug_no_password_shows_none() {
        let cfg = OpenSearchConfig::default();
        let debug_output = format!("{:?}", cfg);
        assert!(
            !debug_output.contains("<redacted>"),
            "debug output must not contain <redacted> when password is None: {}",
            debug_output
        );
    }

    #[test]
    fn test_opensearch_endpoint_config_debug_no_password_shows_none() {
        let cfg =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/myindex?operation=GET")
                .unwrap();
        let debug_output = format!("{:?}", cfg);
        assert!(
            !debug_output.contains("<redacted>"),
            "debug output must not contain <redacted> when password is None: {}",
            debug_output
        );
    }

    #[test]
    fn test_index_name_null_bytes_rejected() {
        let result = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/my%00index?operation=SEARCH",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_index_name_dotdot_rejected() {
        let result = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/my..index?operation=SEARCH",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_index_name_uppercase_rejected() {
        let result = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/MyIndex?operation=SEARCH",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_index_name_special_chars_rejected() {
        let result = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/my@index?operation=SEARCH",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_index_name_valid_lowercase_with_digits_hyphens_underscores() {
        let cfg = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/my-index_01?operation=SEARCH",
        )
        .unwrap();
        assert_eq!(cfg.index_name, "my-index_01");
    }

    // ── OS-012: Unknown operation rejected at config time ─────────────────────

    #[test]
    fn test_unknown_operation_rejected_at_config_time() {
        let result = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/myindex?operation=UNKNOWN",
        );
        assert!(result.is_err(), "expected Err for unknown operation");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("unknown OpenSearch operation"),
            "expected unknown operation error, got: {}",
            err
        );
    }

    // ── OS-013: NetworkRetryPolicy config ──────────────────────────────────

    #[test]
    fn retry_policy_parses_from_uri() {
        let cfg = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/index?retryMaxAttempts=3&retryInitialDelayMs=500",
        )
        .expect("parse");
        assert_eq!(cfg.retry.max_attempts, 3);
        assert_eq!(
            cfg.retry.initial_delay,
            std::time::Duration::from_millis(500)
        );
        assert!(cfg.retry.enabled);
    }

    #[test]
    fn retry_policy_defaults_when_uri_has_no_retry_params() {
        let cfg =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/index").expect("parse");
        assert!(cfg.retry.enabled);
    }

    #[test]
    fn retry_policy_overrides_via_global_config() {
        let ep =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/index").expect("parse");
        let global = OpenSearchConfig::default().with_retry(NetworkRetryPolicy {
            max_attempts: 5,
            initial_delay: std::time::Duration::from_millis(2000),
            ..NetworkRetryPolicy::default()
        });
        let merged = ep.merge_with_global(&global);
        assert_eq!(merged.retry.max_attempts, 5);
        assert_eq!(
            merged.retry.initial_delay,
            std::time::Duration::from_millis(2000)
        );
    }

    #[test]
    fn retry_policy_parse_full_uri_params() {
        let cfg = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/index?retryEnabled=false&retryMaxAttempts=7&retryInitialDelayMs=1000&retryMultiplier=3.0&retryMaxDelayMs=60000&retryJitter=0.5",
        )
        .expect("parse");
        assert!(!cfg.retry.enabled);
        assert_eq!(cfg.retry.max_attempts, 7);
        assert_eq!(
            cfg.retry.initial_delay,
            std::time::Duration::from_millis(1000)
        );
        assert!((cfg.retry.multiplier - 3.0).abs() < f64::EPSILON);
        assert_eq!(cfg.retry.max_delay, std::time::Duration::from_millis(60000));
        assert!((cfg.retry.jitter_factor - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn global_config_has_retry_default() {
        let cfg = OpenSearchConfig::default();
        assert!(cfg.retry.enabled);
    }

    #[test]
    fn retry_policy_from_uri_survives_merge_with_global_default() {
        let ep = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/i?retryMaxAttempts=10&retryInitialDelayMs=500",
        )
        .expect("parse");
        let merged = ep.merge_with_global(&OpenSearchConfig::default());
        assert_eq!(merged.retry.max_attempts, 10);
        assert_eq!(
            merged.retry.initial_delay,
            std::time::Duration::from_millis(500)
        );
    }

    #[test]
    fn retry_policy_falls_back_to_global_when_uri_has_no_retry_params() {
        let mut global = OpenSearchConfig::default();
        global.retry.max_attempts = 7;
        let ep =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/i").expect("parse");
        let merged = ep.merge_with_global(&global);
        assert_eq!(merged.retry.max_attempts, 7);
    }
}
