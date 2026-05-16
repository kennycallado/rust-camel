use std::str::FromStr;

use camel_component_api::CamelError;
use camel_component_api::{UriComponents, UriConfig, parse_uri};

/// Redaction helper: returns `Some("***")` if the option is `Some`, otherwise `None`.
fn redacted_opt(opt: &Option<String>) -> Option<&'static str> {
    if opt.is_some() { Some("***") } else { None }
}

/// Redacts the user:password portion of a database URL for safe display.
/// Returns `"scheme://***@host/db"` for URLs with userinfo, or the original URL otherwise.
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

/// Output type for SQL query results.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SqlOutputType {
    /// Return all rows as a list.
    #[default]
    SelectList,
    /// Return a single row (first result).
    SelectOne,
    /// Stream results as an async iterator.
    StreamList,
}

impl FromStr for SqlOutputType {
    type Err = CamelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "SelectList" => Ok(SqlOutputType::SelectList),
            "SelectOne" => Ok(SqlOutputType::SelectOne),
            "StreamList" => Ok(SqlOutputType::StreamList),
            _ => Err(CamelError::InvalidUri(format!(
                "Unknown output type: {}",
                s
            ))),
        }
    }
}

/// Global configuration for SQL component.
///
/// This struct supports serde deserialization with defaults and builder methods.
/// It holds pool configuration that can be applied as defaults to endpoints.
///
/// **Security note:** `Debug` implementation redacts sensitive fields (SSL key paths).
#[derive(Clone, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SqlGlobalConfig {
    pub max_connections: u32,
    pub min_connections: u32,
    pub idle_timeout_secs: u64,
    pub max_lifetime_secs: u64,
    // SSL/TLS
    pub ssl_mode: Option<String>,
    pub ssl_root_cert: Option<String>,
    pub ssl_cert: Option<String>,
    pub ssl_key: Option<String>,
}

impl std::fmt::Debug for SqlGlobalConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlGlobalConfig")
            .field("max_connections", &self.max_connections)
            .field("min_connections", &self.min_connections)
            .field("idle_timeout_secs", &self.idle_timeout_secs)
            .field("max_lifetime_secs", &self.max_lifetime_secs)
            .field("ssl_mode", &self.ssl_mode)
            .field("ssl_root_cert", &self.ssl_root_cert)
            .field("ssl_cert", &self.ssl_cert)
            .field("ssl_key", &redacted_opt(&self.ssl_key))
            .finish()
    }
}

impl Default for SqlGlobalConfig {
    fn default() -> Self {
        Self {
            max_connections: 5,
            min_connections: 1,
            idle_timeout_secs: 300,
            max_lifetime_secs: 1800,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
        }
    }
}

impl SqlGlobalConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_connections(mut self, value: u32) -> Self {
        self.max_connections = value;
        self
    }

    pub fn with_min_connections(mut self, value: u32) -> Self {
        self.min_connections = value;
        self
    }

    pub fn with_idle_timeout_secs(mut self, value: u64) -> Self {
        self.idle_timeout_secs = value;
        self
    }

    pub fn with_max_lifetime_secs(mut self, value: u64) -> Self {
        self.max_lifetime_secs = value;
        self
    }

    pub fn with_ssl_mode(mut self, value: impl Into<String>) -> Self {
        self.ssl_mode = Some(value.into());
        self
    }

    pub fn with_ssl_root_cert(mut self, value: impl Into<String>) -> Self {
        self.ssl_root_cert = Some(value.into());
        self
    }

    pub fn with_ssl_cert(mut self, value: impl Into<String>) -> Self {
        self.ssl_cert = Some(value.into());
        self
    }

    pub fn with_ssl_key(mut self, value: impl Into<String>) -> Self {
        self.ssl_key = Some(value.into());
        self
    }
}

/// Configuration for SQL component endpoints.
///
/// URI format: `sql:<query>?db_url=<url>&param1=val1&param2=val2`
///
/// The query can be inline SQL or a file reference with `file:` prefix:
/// - `sql:SELECT * FROM users?db_url=...` - inline SQL
/// - `sql:file:/path/to/query.sql?db_url=...` - read SQL from file
///
/// **Note on file-based queries (SQL-014):** When the query path starts with `file:`,
/// the SQL is read synchronously via `std::fs::read_to_string` during endpoint creation.
/// This is a blocking I/O call, but it occurs in the synchronous `from_uri` / `create_endpoint`
/// path — not in the async hot path (producer `call()` or consumer `poll_database()`).
/// The file content is cached in `config.query` after parsing, so no runtime file reads occur.
///
/// **Security note:** `Debug` implementation redacts the `db_url` (which may contain credentials)
/// and `ssl_key` path. Use `redact_db_url()` for safe logging of database URLs.
#[derive(Clone)]
pub struct SqlEndpointConfig {
    // Connection
    /// Database connection URL (required).
    pub db_url: String,
    /// Maximum connections in the pool. None = use global default.
    pub max_connections: Option<u32>,
    /// Minimum connections in the pool. None = use global default.
    pub min_connections: Option<u32>,
    /// Idle timeout in seconds. None = use global default.
    pub idle_timeout_secs: Option<u64>,
    /// Maximum connection lifetime in seconds. None = use global default.
    pub max_lifetime_secs: Option<u64>,

    // Query
    /// The SQL query (from URI path or file).
    pub query: String,
    /// Path to the file containing the SQL query (when using `file:` prefix).
    pub source_path: Option<String>,
    /// Output type for query results. Default: SelectList.
    pub output_type: SqlOutputType,
    /// Placeholder character for parameters. Default: '#'.
    pub placeholder: char,
    /// If true, process parameter placeholders in queries. Default: true.
    /// When false, queries are executed as-is without template parsing.
    pub use_placeholder: bool,
    /// If true, don't execute the query (dry run). Default: false.
    pub noop: bool,
    /// Separator for IN clause expansion. Default: ", ".
    pub in_separator: String,

    // Consumer (polling)
    /// Delay between polls in milliseconds. Default: 500.
    pub delay_ms: u64,
    /// Initial delay before first poll in milliseconds. Default: 1000.
    pub initial_delay_ms: u64,
    /// Maximum messages per poll.
    pub max_messages_per_poll: Option<i32>,
    /// SQL to execute after consuming each message.
    pub on_consume: Option<String>,
    /// SQL to execute if consumption fails.
    pub on_consume_failed: Option<String>,
    /// SQL to execute after consuming a batch.
    pub on_consume_batch_complete: Option<String>,
    /// Route empty result sets. Default: false.
    pub route_empty_result_set: bool,
    /// Use iterator for results. Default: true.
    pub use_iterator: bool,
    /// Expected number of rows affected.
    pub expected_update_count: Option<i64>,
    /// Break batch on consume failure. Default: false.
    pub break_batch_on_consume_fail: bool,

    // Producer
    /// Enable batch mode. Default: false.
    pub batch: bool,
    /// Use message body for SQL. Default: false.
    pub use_message_body_for_sql: bool,

    // SSL/TLS
    /// SSL mode for the connection. None = use global default.
    pub ssl_mode: Option<String>,
    /// Path to SSL root certificate. None = use global default.
    pub ssl_root_cert: Option<String>,
    /// Path to SSL client certificate. None = use global default.
    pub ssl_cert: Option<String>,
    /// Path to SSL client key. None = use global default.
    pub ssl_key: Option<String>,
}

impl std::fmt::Debug for SqlEndpointConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlEndpointConfig")
            .field("db_url", &redact_db_url(&self.db_url))
            .field("max_connections", &self.max_connections)
            .field("min_connections", &self.min_connections)
            .field("idle_timeout_secs", &self.idle_timeout_secs)
            .field("max_lifetime_secs", &self.max_lifetime_secs)
            .field("query", &self.query)
            .field("source_path", &self.source_path)
            .field("output_type", &self.output_type)
            .field("placeholder", &self.placeholder)
            .field("use_placeholder", &self.use_placeholder)
            .field("noop", &self.noop)
            .field("in_separator", &self.in_separator)
            .field("delay_ms", &self.delay_ms)
            .field("initial_delay_ms", &self.initial_delay_ms)
            .field("max_messages_per_poll", &self.max_messages_per_poll)
            .field("on_consume", &self.on_consume)
            .field("on_consume_failed", &self.on_consume_failed)
            .field("on_consume_batch_complete", &self.on_consume_batch_complete)
            .field("route_empty_result_set", &self.route_empty_result_set)
            .field("use_iterator", &self.use_iterator)
            .field("expected_update_count", &self.expected_update_count)
            .field(
                "break_batch_on_consume_fail",
                &self.break_batch_on_consume_fail,
            )
            .field("batch", &self.batch)
            .field("use_message_body_for_sql", &self.use_message_body_for_sql)
            .field("ssl_mode", &self.ssl_mode)
            .field("ssl_root_cert", &self.ssl_root_cert)
            .field("ssl_cert", &self.ssl_cert)
            .field("ssl_key", &redacted_opt(&self.ssl_key))
            .finish()
    }
}

impl SqlEndpointConfig {
    /// Apply defaults from global config, filling None fields without overriding.
    pub fn apply_defaults(&mut self, defaults: &SqlGlobalConfig) {
        if self.max_connections.is_none() {
            self.max_connections = Some(defaults.max_connections);
        }
        if self.min_connections.is_none() {
            self.min_connections = Some(defaults.min_connections);
        }
        if self.idle_timeout_secs.is_none() {
            self.idle_timeout_secs = Some(defaults.idle_timeout_secs);
        }
        if self.max_lifetime_secs.is_none() {
            self.max_lifetime_secs = Some(defaults.max_lifetime_secs);
        }
        if self.ssl_mode.is_none() {
            self.ssl_mode = defaults.ssl_mode.clone();
        }
        if self.ssl_root_cert.is_none() {
            self.ssl_root_cert = defaults.ssl_root_cert.clone();
        }
        if self.ssl_cert.is_none() {
            self.ssl_cert = defaults.ssl_cert.clone();
        }
        if self.ssl_key.is_none() {
            self.ssl_key = defaults.ssl_key.clone();
        }
    }

    /// Resolve any remaining None fields with built-in defaults.
    pub fn resolve_defaults(&mut self) {
        let defaults = SqlGlobalConfig::default();
        self.apply_defaults(&defaults);
    }
}

struct SslParamMapping {
    pg_key: &'static str,
    mysql_key: &'static str,
}

const SSL_MAPPINGS: &[(&str, SslParamMapping)] = &[
    (
        "sslMode",
        SslParamMapping {
            pg_key: "sslmode",
            mysql_key: "ssl-mode",
        },
    ),
    (
        "sslRootCert",
        SslParamMapping {
            pg_key: "sslrootcert",
            mysql_key: "ssl-ca",
        },
    ),
    (
        "sslCert",
        SslParamMapping {
            pg_key: "sslcert",
            mysql_key: "ssl-cert",
        },
    ),
    (
        "sslKey",
        SslParamMapping {
            pg_key: "sslkey",
            mysql_key: "ssl-key",
        },
    ),
];

pub fn enrich_db_url_with_ssl(
    db_url: &str,
    config: &SqlEndpointConfig,
) -> Result<String, CamelError> {
    let ssl_params: Vec<(&str, &str)> = [
        config.ssl_mode.as_deref().map(|v| ("sslMode", v)),
        config.ssl_root_cert.as_deref().map(|v| ("sslRootCert", v)),
        config.ssl_cert.as_deref().map(|v| ("sslCert", v)),
        config.ssl_key.as_deref().map(|v| ("sslKey", v)),
    ]
    .into_iter()
    .flatten()
    .collect();

    if ssl_params.is_empty() {
        return Ok(db_url.to_string());
    }

    let mut parsed = url::Url::parse(db_url).map_err(|e| {
        CamelError::InvalidUri(format!(
            "Cannot parse database URL for SSL enrichment: {}",
            e
        ))
    })?;

    let scheme = parsed.scheme();
    if scheme != "postgres" && scheme != "postgresql" && scheme != "mysql" {
        return Ok(db_url.to_string());
    }
    let is_mysql = scheme == "mysql";

    let mut query_pairs = parsed.query_pairs().collect::<Vec<_>>();
    for (camel_name, value) in &ssl_params {
        if let Some((_, mapping)) = SSL_MAPPINGS.iter().find(|(name, _)| *name == *camel_name) {
            let driver_key = if is_mysql {
                mapping.mysql_key
            } else {
                mapping.pg_key
            };

            if let Some(pos) = query_pairs.iter().position(|(k, _)| k == driver_key) {
                query_pairs[pos].1 = (*value).into();
            } else {
                query_pairs.push((driver_key.into(), (*value).into()));
            }
        }
    }

    {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in query_pairs {
            serializer.append_pair(&k, &v);
        }
        parsed.set_query(Some(&serializer.finish()));
    }

    Ok(parsed.to_string())
}

impl UriConfig for SqlEndpointConfig {
    fn scheme() -> &'static str {
        "sql"
    }

    fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        Self::from_components(parts)
    }

    fn from_components(parts: UriComponents) -> Result<Self, CamelError> {
        // Validate scheme
        if parts.scheme != Self::scheme() {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme '{}' but got '{}'",
                Self::scheme(),
                parts.scheme
            )));
        }

        let params = &parts.params;

        // Handle file: prefix for query
        let (query, source_path) = if parts.path.starts_with("file:") {
            let file_path = parts.path.trim_start_matches("file:").to_string();
            let contents = std::fs::read_to_string(&file_path).map_err(|e| {
                CamelError::Config(format!("Failed to read SQL file '{}': {}", file_path, e))
            })?;
            (contents.trim().to_string(), Some(file_path))
        } else {
            (parts.path.clone(), None)
        };

        // Required parameter: db_url
        let db_url = params
            .get("db_url")
            .ok_or_else(|| CamelError::Config("db_url parameter is required".to_string()))?
            .clone();

        // Connection parameters - None when not set by URI param
        let max_connections = params.get("maxConnections").and_then(|v| v.parse().ok());
        let min_connections = params.get("minConnections").and_then(|v| v.parse().ok());
        let idle_timeout_secs = params.get("idleTimeoutSecs").and_then(|v| v.parse().ok());
        let max_lifetime_secs = params.get("maxLifetimeSecs").and_then(|v| v.parse().ok());

        // Query parameters
        let output_type = params
            .get("outputType")
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or_default();
        let placeholder = params
            .get("placeholder")
            .filter(|v| !v.is_empty())
            .map(|v| {
                if v.chars().count() != 1 {
                    return Err(CamelError::InvalidUri(format!(
                        "placeholder must be exactly one character, got '{}'",
                        v
                    )));
                }
                Ok(v.chars().next().unwrap())
            })
            .transpose()?
            .unwrap_or('#');
/// Parse a boolean URI parameter strictly.
///
/// Accepts only `"true"` or `"false"` (case-insensitive). Any other value
/// returns `CamelError::InvalidUri` to prevent silent misconfiguration.
fn parse_bool_param(name: &str, value: &str) -> Result<bool, CamelError> {
    if value.eq_ignore_ascii_case("true") {
        Ok(true)
    } else if value.eq_ignore_ascii_case("false") {
        Ok(false)
    } else {
        Err(CamelError::InvalidUri(format!(
            "{} must be 'true' or 'false', got '{}'",
            name, value
        )))
    }
}

        let use_placeholder = params
            .get("usePlaceholder")
            .map(|v| parse_bool_param("usePlaceholder", v))
            .transpose()?
            .unwrap_or(true);
        let noop = params
            .get("noop")
            .map(|v| parse_bool_param("noop", v))
            .transpose()?
            .unwrap_or(false);
        let in_separator = params
            .get("inSeparator")
            .map(|v| v.to_string())
            .unwrap_or_else(|| ", ".to_string());
        if in_separator.is_empty() {
            return Err(CamelError::InvalidUri(
                "inSeparator must not be empty".to_string(),
            ));
        }

        // Consumer parameters
        let delay_ms = params
            .get("delay")
            .and_then(|v| v.parse().ok())
            .unwrap_or(500);
        let initial_delay_ms = params
            .get("initialDelay")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1000);
        let max_messages_per_poll = params
            .get("maxMessagesPerPoll")
            .and_then(|v| v.parse().ok());
        let on_consume = params.get("onConsume").cloned();
        let on_consume_failed = params.get("onConsumeFailed").cloned();
        let on_consume_batch_complete = params.get("onConsumeBatchComplete").cloned();
        let route_empty_result_set = params
            .get("routeEmptyResultSet")
            .map(|v| parse_bool_param("routeEmptyResultSet", v))
            .transpose()?
            .unwrap_or(false);
        let use_iterator = params
            .get("useIterator")
            .map(|v| parse_bool_param("useIterator", v))
            .transpose()?
            .unwrap_or(true);
        let expected_update_count = params
            .get("expectedUpdateCount")
            .and_then(|v| v.parse().ok());
        let break_batch_on_consume_fail = params
            .get("breakBatchOnConsumeFail")
            .map(|v| parse_bool_param("breakBatchOnConsumeFail", v))
            .transpose()?
            .unwrap_or(false);

        // Producer parameters
        let batch = params
            .get("batch")
            .map(|v| parse_bool_param("batch", v))
            .transpose()?
            .unwrap_or(false);
        let use_message_body_for_sql = params
            .get("useMessageBodyForSql")
            .map(|v| parse_bool_param("useMessageBodyForSql", v))
            .transpose()?
            .unwrap_or(false);
        let ssl_mode = params.get("sslMode").cloned();
        let ssl_root_cert = params.get("sslRootCert").cloned();
        let ssl_cert = params.get("sslCert").cloned();
        let ssl_key = params.get("sslKey").cloned();

        Ok(Self {
            db_url,
            max_connections,
            min_connections,
            idle_timeout_secs,
            max_lifetime_secs,
            query,
            source_path,
            output_type,
            placeholder,
            use_placeholder,
            noop,
            in_separator,
            delay_ms,
            initial_delay_ms,
            max_messages_per_poll,
            on_consume,
            on_consume_failed,
            on_consume_batch_complete,
            route_empty_result_set,
            use_iterator,
            expected_update_count,
            break_batch_on_consume_fail,
            batch,
            use_message_body_for_sql,
            ssl_mode,
            ssl_root_cert,
            ssl_cert,
            ssl_key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let mut c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        c.resolve_defaults();
        assert_eq!(c.query, "select 1");
        assert_eq!(c.db_url, "postgres://localhost/test");
        assert_eq!(c.max_connections, Some(5));
        assert_eq!(c.min_connections, Some(1));
        assert_eq!(c.idle_timeout_secs, Some(300));
        assert_eq!(c.max_lifetime_secs, Some(1800));
        assert_eq!(c.output_type, SqlOutputType::SelectList);
        assert_eq!(c.placeholder, '#');
        assert!(!c.noop);
        assert_eq!(c.in_separator, ", ");
        assert_eq!(c.delay_ms, 500);
        assert_eq!(c.initial_delay_ms, 1000);
        assert!(c.max_messages_per_poll.is_none());
        assert!(c.on_consume.is_none());
        assert!(c.on_consume_failed.is_none());
        assert!(c.on_consume_batch_complete.is_none());
        assert!(!c.route_empty_result_set);
        assert!(c.use_iterator);
        assert!(c.expected_update_count.is_none());
        assert!(!c.break_batch_on_consume_fail);
        assert!(!c.batch);
        assert!(!c.use_message_body_for_sql);
        assert!(c.ssl_mode.is_none());
        assert!(c.ssl_root_cert.is_none());
        assert!(c.ssl_cert.is_none());
        assert!(c.ssl_key.is_none());
    }

    #[test]
    fn ssl_none_by_default() {
        let c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        assert!(c.ssl_mode.is_none());
        assert!(c.ssl_root_cert.is_none());
        assert!(c.ssl_cert.is_none());
        assert!(c.ssl_key.is_none());
    }

    #[test]
    fn ssl_mode_from_uri() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&sslMode=require",
        )
        .unwrap();
        assert_eq!(c.ssl_mode, Some("require".to_string()));
        assert!(c.ssl_root_cert.is_none());
    }

    #[test]
    fn ssl_all_params_from_uri() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&sslMode=require&sslRootCert=/ca.pem&sslCert=/cert.pem&sslKey=/key.pem",
        )
        .unwrap();
        assert_eq!(c.ssl_mode, Some("require".to_string()));
        assert_eq!(c.ssl_root_cert, Some("/ca.pem".to_string()));
        assert_eq!(c.ssl_cert, Some("/cert.pem".to_string()));
        assert_eq!(c.ssl_key, Some("/key.pem".to_string()));
    }

    #[test]
    fn ssl_global_applied_to_endpoint() {
        let mut c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        let global = SqlGlobalConfig::default()
            .with_ssl_mode("require")
            .with_ssl_root_cert("/etc/ssl/ca.pem");
        c.apply_defaults(&global);
        assert_eq!(c.ssl_mode, Some("require".to_string()));
        assert_eq!(c.ssl_root_cert, Some("/etc/ssl/ca.pem".to_string()));
        assert!(c.ssl_cert.is_none());
        assert!(c.ssl_key.is_none());
    }

    #[test]
    fn ssl_uri_overrides_global() {
        let mut c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&sslMode=verify-full",
        )
        .unwrap();
        let global = SqlGlobalConfig::default().with_ssl_mode("require");
        c.apply_defaults(&global);
        assert_eq!(c.ssl_mode, Some("verify-full".to_string()));
    }

    #[test]
    fn config_wrong_scheme() {
        assert!(SqlEndpointConfig::from_uri("redis://localhost:6379").is_err());
    }

    #[test]
    fn config_missing_db_url() {
        assert!(SqlEndpointConfig::from_uri("sql:select 1").is_err());
    }

    #[test]
    fn config_output_type_select_one() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&outputType=SelectOne",
        )
        .unwrap();
        assert_eq!(c.output_type, SqlOutputType::SelectOne);
    }

    #[test]
    fn config_output_type_stream_list() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&outputType=StreamList",
        )
        .unwrap();
        assert_eq!(c.output_type, SqlOutputType::StreamList);
    }

    #[test]
    fn in_separator_default() {
        let c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        assert_eq!(c.in_separator, ", ");
    }

    #[test]
    fn in_separator_from_uri() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&inSeparator=;",
        )
        .unwrap();
        assert_eq!(c.in_separator, ";");
    }

    #[test]
    fn in_separator_empty_rejected() {
        let result = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&inSeparator=",
        );
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("inSeparator") || msg.contains("empty"));
    }

    #[test]
    fn config_consumer_options() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select * from t?db_url=postgres://localhost/test&delay=2000&initialDelay=500&maxMessagesPerPoll=10&onConsume=update t set done=true where id=:#id&onConsumeFailed=update t set failed=true where id=:#id&onConsumeBatchComplete=delete from t where done=true&routeEmptyResultSet=true&useIterator=false&expectedUpdateCount=1&breakBatchOnConsumeFail=true"
        ).unwrap();
        assert_eq!(c.delay_ms, 2000);
        assert_eq!(c.initial_delay_ms, 500);
        assert_eq!(c.max_messages_per_poll, Some(10));
        assert_eq!(
            c.on_consume,
            Some("update t set done=true where id=:#id".to_string())
        );
        assert_eq!(
            c.on_consume_failed,
            Some("update t set failed=true where id=:#id".to_string())
        );
        assert_eq!(
            c.on_consume_batch_complete,
            Some("delete from t where done=true".to_string())
        );
        assert!(c.route_empty_result_set);
        assert!(!c.use_iterator);
        assert_eq!(c.expected_update_count, Some(1));
        assert!(c.break_batch_on_consume_fail);
    }

    #[test]
    fn config_producer_options() {
        let c = SqlEndpointConfig::from_uri(
            "sql:insert into t values (#)?db_url=postgres://localhost/test&batch=true&useMessageBodyForSql=true&noop=true"
        ).unwrap();
        assert!(c.batch);
        assert!(c.use_message_body_for_sql);
        assert!(c.noop);
    }

    #[test]
    fn config_pool_options() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&maxConnections=20&minConnections=3&idleTimeoutSecs=600&maxLifetimeSecs=3600"
        ).unwrap();
        assert_eq!(c.max_connections, Some(20));
        assert_eq!(c.min_connections, Some(3));
        assert_eq!(c.idle_timeout_secs, Some(600));
        assert_eq!(c.max_lifetime_secs, Some(3600));
    }

    #[test]
    fn config_query_with_special_chars() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select * from users where name = :#name and age > #?db_url=postgres://localhost/test",
        )
        .unwrap();
        assert_eq!(
            c.query,
            "select * from users where name = :#name and age > #"
        );
    }

    #[test]
    fn output_type_from_str() {
        assert_eq!(
            "SelectList".parse::<SqlOutputType>().unwrap(),
            SqlOutputType::SelectList
        );
        assert_eq!(
            "SelectOne".parse::<SqlOutputType>().unwrap(),
            SqlOutputType::SelectOne
        );
        assert_eq!(
            "StreamList".parse::<SqlOutputType>().unwrap(),
            SqlOutputType::StreamList
        );
        assert!("Invalid".parse::<SqlOutputType>().is_err());
    }

    #[test]
    fn config_file_not_found() {
        let result = SqlEndpointConfig::from_uri(
            "sql:file:/nonexistent/path/query.sql?db_url=postgres://localhost/test",
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{:?}", err);
        assert!(msg.contains("Failed to read SQL file") || msg.contains("nonexistent"));
    }

    #[test]
    fn config_file_query() {
        use std::io::Write;
        let unique_name = format!(
            "test_sql_query_{}.sql",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let mut tmp = std::env::temp_dir();
        tmp.push(unique_name);
        {
            let mut f = std::fs::File::create(&tmp).unwrap();
            writeln!(f, "SELECT * FROM users").unwrap();
        }
        let uri = format!(
            "sql:file:{}?db_url=postgres://localhost/test",
            tmp.display()
        );
        let c = SqlEndpointConfig::from_uri(&uri).unwrap();
        assert_eq!(c.query, "SELECT * FROM users");
        assert_eq!(c.source_path, Some(tmp.to_string_lossy().into_owned()));
        std::fs::remove_file(&tmp).ok();
    }

    // New tests for config contract
    #[test]
    fn pool_fields_none_when_not_set() {
        let c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        assert_eq!(c.max_connections, None);
        assert_eq!(c.min_connections, None);
        assert_eq!(c.idle_timeout_secs, None);
        assert_eq!(c.max_lifetime_secs, None);
    }

    #[test]
    fn apply_defaults_fills_none() {
        let mut c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        let global = SqlGlobalConfig {
            max_connections: 10,
            min_connections: 2,
            idle_timeout_secs: 600,
            max_lifetime_secs: 3600,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
        };
        c.apply_defaults(&global);
        assert_eq!(c.max_connections, Some(10));
        assert_eq!(c.min_connections, Some(2));
        assert_eq!(c.idle_timeout_secs, Some(600));
        assert_eq!(c.max_lifetime_secs, Some(3600));
        assert!(c.ssl_mode.is_none());
        assert!(c.ssl_root_cert.is_none());
        assert!(c.ssl_cert.is_none());
        assert!(c.ssl_key.is_none());
    }

    #[test]
    fn apply_defaults_does_not_override() {
        let mut c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&maxConnections=99&minConnections=5",
        )
        .unwrap();
        let global = SqlGlobalConfig {
            max_connections: 10,
            min_connections: 2,
            idle_timeout_secs: 600,
            max_lifetime_secs: 3600,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
        };
        c.apply_defaults(&global);
        // URI-set values should NOT be overridden
        assert_eq!(c.max_connections, Some(99));
        assert_eq!(c.min_connections, Some(5));
        // None fields should be filled from global
        assert_eq!(c.idle_timeout_secs, Some(600));
        assert_eq!(c.max_lifetime_secs, Some(3600));
    }

    #[test]
    fn resolve_defaults_fills_remaining() {
        let mut c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&maxConnections=7",
        )
        .unwrap();
        c.resolve_defaults();
        assert_eq!(c.max_connections, Some(7)); // from URI
        assert_eq!(c.min_connections, Some(1)); // from defaults
        assert_eq!(c.idle_timeout_secs, Some(300)); // from defaults
        assert_eq!(c.max_lifetime_secs, Some(1800)); // from defaults
    }

    #[test]
    fn global_config_builder() {
        let c = SqlGlobalConfig::default()
            .with_max_connections(20)
            .with_min_connections(3)
            .with_idle_timeout_secs(600)
            .with_max_lifetime_secs(3600)
            .with_ssl_mode("require")
            .with_ssl_root_cert("/ca.pem")
            .with_ssl_cert("/cert.pem")
            .with_ssl_key("/key.pem");
        assert_eq!(c.max_connections, 20);
        assert_eq!(c.min_connections, 3);
        assert_eq!(c.idle_timeout_secs, 600);
        assert_eq!(c.max_lifetime_secs, 3600);
        assert_eq!(c.ssl_mode, Some("require".to_string()));
        assert_eq!(c.ssl_root_cert, Some("/ca.pem".to_string()));
        assert_eq!(c.ssl_cert, Some("/cert.pem".to_string()));
        assert_eq!(c.ssl_key, Some("/key.pem".to_string()));
    }

    #[test]
    fn enrich_postgres_ssl_mode() {
        let mut c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&sslMode=require",
        )
        .unwrap();
        c.resolve_defaults();
        let url = enrich_db_url_with_ssl(&c.db_url, &c).unwrap();
        assert!(url.contains("sslmode=require"), "got: {}", url);
    }

    #[test]
    fn enrich_postgres_all_ssl() {
        let mut c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&sslMode=require&sslRootCert=/ca.pem&sslCert=/cert.pem&sslKey=/key.pem",
        )
        .unwrap();
        c.resolve_defaults();
        let url = enrich_db_url_with_ssl(&c.db_url, &c).unwrap();
        assert!(url.contains("sslmode=require"), "got: {}", url);
        assert!(url.contains("sslrootcert="), "got: {}", url);
        assert!(url.contains("sslcert="), "got: {}", url);
        assert!(url.contains("sslkey="), "got: {}", url);
    }

    #[test]
    fn enrich_mysql_ssl() {
        let mut c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=mysql://localhost/test&sslMode=require",
        )
        .unwrap();
        c.resolve_defaults();
        let url = enrich_db_url_with_ssl(&c.db_url, &c).unwrap();
        assert!(url.contains("ssl-mode=require"), "got: {}", url);
    }

    #[test]
    fn enrich_existing_query_params() {
        let mut c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test?existing=1&sslMode=require",
        )
        .unwrap();
        c.resolve_defaults();
        let url = enrich_db_url_with_ssl(&c.db_url, &c).unwrap();
        assert!(url.contains("existing=1"), "got: {}", url);
        assert!(url.contains("sslmode=require"), "got: {}", url);
    }

    #[test]
    fn enrich_override_existing() {
        let mut c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test?sslmode=allow&sslMode=require",
        )
        .unwrap();
        c.resolve_defaults();
        let url = enrich_db_url_with_ssl(&c.db_url, &c).unwrap();
        assert!(url.contains("sslmode=require"), "got: {}", url);
        assert!(!url.contains("sslmode=allow"), "got: {}", url);
    }

    #[test]
    fn enrich_no_params() {
        let mut c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        c.resolve_defaults();
        let url = enrich_db_url_with_ssl(&c.db_url, &c).unwrap();
        assert_eq!(url, "postgres://localhost/test");
    }

    #[test]
    fn enrich_url_encodes_paths() {
        let mut c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&sslRootCert=/path/to/my%20cert.pem",
        )
        .unwrap();
        c.resolve_defaults();
        let url = enrich_db_url_with_ssl(&c.db_url, &c).unwrap();
        assert!(url.contains("sslrootcert="), "got: {}", url);
    }

    #[test]
    fn enrich_unsupported_scheme_returns_unchanged() {
        let mut c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=sqlite://localhost/test.db&sslMode=require",
        )
        .unwrap();
        c.resolve_defaults();
        let url = enrich_db_url_with_ssl(&c.db_url, &c).unwrap();
        assert_eq!(url, "sqlite://localhost/test.db");
    }

    #[test]
    fn enrich_invalid_url_returns_error() {
        let mut c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&sslMode=require",
        )
        .unwrap();
        c.resolve_defaults();
        let result = enrich_db_url_with_ssl("://not-a-valid-url", &c);
        assert!(result.is_err());
    }

    // --- Phase B hardening tests ---

    // SQL-010: Debug output redacts credentials
    #[test]
    fn debug_redacts_db_url_with_password() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://user:secret123@localhost/test",
        )
        .unwrap();
        let debug_output = format!("{:?}", c);
        assert!(
            !debug_output.contains("secret123"),
            "Debug output must not contain password: {}",
            debug_output
        );
        assert!(
            debug_output.contains("***"),
            "Debug output must contain redacted marker: {}",
            debug_output
        );
    }

    #[test]
    fn debug_redacts_ssl_key() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&sslKey=/secret/key.pem",
        )
        .unwrap();
        let debug_output = format!("{:?}", c);
        assert!(
            !debug_output.contains("/secret/key.pem"),
            "Debug output must not contain ssl_key path: {}",
            debug_output
        );
    }

    #[test]
    fn debug_global_config_redacts_ssl_key() {
        let c = SqlGlobalConfig::default().with_ssl_key("/secret/key.pem");
        let debug_output = format!("{:?}", c);
        assert!(
            !debug_output.contains("/secret/key.pem"),
            "Debug output must not contain ssl_key path: {}",
            debug_output
        );
        assert!(
            debug_output.contains("***"),
            "Debug output must contain redacted marker: {}",
            debug_output
        );
    }

    #[test]
    fn redact_db_url_with_credentials() {
        assert_eq!(
            redact_db_url("postgres://user:pass@host/db"),
            "postgres://***:***@host/db"
        );
    }

    #[test]
    fn redact_db_url_without_credentials() {
        assert_eq!(redact_db_url("sqlite::memory:"), "sqlite::memory:");
    }

    #[test]
    fn redact_db_url_invalid_returns_original() {
        assert_eq!(redact_db_url("not-a-url"), "not-a-url");
    }

    // SQL-004: usePlaceholder parsing
    #[test]
    fn use_placeholder_defaults_to_true() {
        let c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        assert!(c.use_placeholder);
    }

    #[test]
    fn use_placeholder_false_from_uri() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&usePlaceholder=false",
        )
        .unwrap();
        assert!(!c.use_placeholder);
    }

    #[test]
    fn use_placeholder_true_from_uri() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&usePlaceholder=true",
        )
        .unwrap();
        assert!(c.use_placeholder);
    }

    // SQL-004: strict boolean parsing — invalid values rejected
    #[test]
    fn use_placeholder_rejects_invalid_value() {
        let result = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&usePlaceholder=1",
        );
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("usePlaceholder") && msg.contains("true") && msg.contains("false"));
    }

    #[test]
    fn use_placeholder_rejects_typo_tru() {
        let result = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&usePlaceholder=tru",
        );
        assert!(result.is_err());
    }

    #[test]
    fn use_placeholder_rejects_yes() {
        let result = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&usePlaceholder=yes",
        );
        assert!(result.is_err());
    }

    #[test]
    fn noop_rejects_invalid_value() {
        let result = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&noop=1",
        );
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("noop"));
    }

    #[test]
    fn batch_rejects_invalid_value() {
        let result = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&batch=yes",
        );
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("batch"));
    }

    #[test]
    fn route_empty_result_set_rejects_invalid_value() {
        let result = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&routeEmptyResultSet=on",
        );
        assert!(result.is_err());
    }

    #[test]
    fn use_iterator_rejects_invalid_value() {
        let result = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&useIterator=1",
        );
        assert!(result.is_err());
    }

    #[test]
    fn break_batch_on_consume_fail_rejects_invalid_value() {
        let result = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&breakBatchOnConsumeFail=yes",
        );
        assert!(result.is_err());
    }

    #[test]
    fn use_message_body_for_sql_rejects_invalid_value() {
        let result = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&useMessageBodyForSql=1",
        );
        assert!(result.is_err());
    }

    // Case-insensitive true/false still works
    #[test]
    fn boolean_params_case_insensitive() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&usePlaceholder=TRUE&noop=FALSE&batch=True&useIterator=False",
        )
        .unwrap();
        assert!(c.use_placeholder);
        assert!(!c.noop);
        assert!(c.batch);
        assert!(!c.use_iterator);
    }

    // SQL-022: multi-char placeholder rejected
    #[test]
    fn multi_char_placeholder_rejected() {
        let result = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&placeholder=##",
        );
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("placeholder") && msg.contains("one character"));
    }

    #[test]
    fn single_char_placeholder_accepted() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&placeholder=$",
        )
        .unwrap();
        assert_eq!(c.placeholder, '$');
    }

    #[test]
    fn empty_placeholder_falls_back_to_default() {
        // Empty string is filtered out by the original logic — falls back to '#'
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&placeholder=",
        )
        .unwrap();
        assert_eq!(c.placeholder, '#');
    }

    // SQL-014: file-based SQL config test (verifies file content is cached, no async read)
    #[test]
    fn file_query_cached_in_config() {
        use std::io::Write;
        let unique_name = format!(
            "test_sql_cached_{}.sql",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let mut tmp = std::env::temp_dir();
        tmp.push(unique_name);
        {
            let mut f = std::fs::File::create(&tmp).unwrap();
            writeln!(f, "SELECT * FROM cached_test").unwrap();
        }
        let uri = format!(
            "sql:file:{}?db_url=postgres://localhost/test",
            tmp.display()
        );
        let c = SqlEndpointConfig::from_uri(&uri).unwrap();
        // Query content is cached in config — no runtime file read needed
        assert_eq!(c.query, "SELECT * FROM cached_test");
        assert_eq!(c.source_path, Some(tmp.to_string_lossy().into_owned()));
        // Delete the file — config still has the query
        std::fs::remove_file(&tmp).ok();
        assert_eq!(c.query, "SELECT * FROM cached_test");
    }
}
