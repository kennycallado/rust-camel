use camel_component_api::CamelError;
use camel_component_api::parse_uri;
use std::convert::TryFrom;
use std::fmt;
use std::str::FromStr;

// --- OpenSearchOperation enum ---

/// OpenSearch operations supported by this component.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(try_from = "String")]
pub enum OpenSearchOperation {
    INDEX,
    SEARCH,
    GET,
    DELETE,
    UPDATE,
    BULK,
    MULTIGET,
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
            "UPDATE" => Ok(OpenSearchOperation::UPDATE),
            "BULK" => Ok(OpenSearchOperation::BULK),
            "MULTIGET" => Ok(OpenSearchOperation::MULTIGET),
            other => Ok(OpenSearchOperation::UNKNOWN(other.to_string())),
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
            OpenSearchOperation::UPDATE => write!(f, "UPDATE"),
            OpenSearchOperation::BULK => write!(f, "BULK"),
            OpenSearchOperation::MULTIGET => write!(f, "MULTIGET"),
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

        Ok(Self {
            host,
            port,
            username,
            password,
            index_name,
            operation,
            is_tls,
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
        }
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
        // Case insensitive
        assert_eq!(
            OpenSearchOperation::from_str("index").unwrap(),
            OpenSearchOperation::INDEX
        );
        assert_eq!(
            OpenSearchOperation::from_str("Search").unwrap(),
            OpenSearchOperation::SEARCH
        );
        // Unknown captures unrecognized values
        match OpenSearchOperation::from_str("CUSTOM_OP").unwrap() {
            OpenSearchOperation::UNKNOWN(s) => assert_eq!(s, "CUSTOM_OP"),
            other => panic!("expected UNKNOWN, got {:?}", other),
        }
    }

    #[test]
    fn test_operation_display() {
        assert_eq!(OpenSearchOperation::INDEX.to_string(), "INDEX");
        assert_eq!(OpenSearchOperation::SEARCH.to_string(), "SEARCH");
        assert_eq!(OpenSearchOperation::GET.to_string(), "GET");
        assert_eq!(OpenSearchOperation::DELETE.to_string(), "DELETE");
        assert_eq!(OpenSearchOperation::UPDATE.to_string(), "UPDATE");
        assert_eq!(OpenSearchOperation::BULK.to_string(), "BULK");
        assert_eq!(OpenSearchOperation::MULTIGET.to_string(), "MULTIGET");
        assert_eq!(
            OpenSearchOperation::UNKNOWN("CUSTOM".to_string()).to_string(),
            "CUSTOM"
        );

        // Roundtrip
        for s in &[
            "INDEX", "SEARCH", "GET", "DELETE", "UPDATE", "BULK", "MULTIGET",
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
        let ep = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/myindex?operation=UNKNOWN_OP",
        )
        .unwrap();
        assert!(matches!(ep.operation, OpenSearchOperation::UNKNOWN(_)));

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
    }

    #[test]
    fn test_config_builder() {
        let cfg = OpenSearchConfig::default()
            .with_host("es-prod")
            .with_port(9200)
            .with_default_operation(OpenSearchOperation::BULK)
            .with_username("admin")
            .with_password("secret");
        assert_eq!(cfg.host, "es-prod");
        assert_eq!(cfg.port, 9200);
        assert_eq!(cfg.default_operation, Some(OpenSearchOperation::BULK));
        assert_eq!(cfg.username, Some("admin".to_string()));
        assert_eq!(cfg.password, Some("secret".to_string()));
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
}
