use std::str::FromStr;
use std::time::Duration;

use camel_component_api::CamelError;
use camel_component_api::NetworkRetryPolicy;
use camel_component_api::{UriComponents, UriConfig, parse_uri};
use tracing::warn;

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

/// Transaction mode for SQL operations.
///
/// - `Auto`: Each statement auto-commits (default, current behavior).
/// - `Managed`: Explicit transaction boundaries (future; currently logs a warning
///   and falls back to Auto).
///
// TODO(SQL-002): managed transaction mode — implement explicit transaction boundaries
#[derive(Debug, Clone, PartialEq, Default)]
pub enum TransactionMode {
    /// Auto-commit each statement (default).
    #[default]
    Auto,
    /// Managed transactions — not yet implemented.
    Managed,
}

impl FromStr for TransactionMode {
    type Err = CamelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Auto" => Ok(TransactionMode::Auto),
            "Managed" => Ok(TransactionMode::Managed),
            _ => Err(CamelError::InvalidUri(format!(
                "Unknown transaction mode: {}. Expected 'Auto' or 'Managed'",
                s
            ))),
        }
    }
}

impl std::fmt::Display for TransactionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransactionMode::Auto => write!(f, "Auto"),
            TransactionMode::Managed => write!(f, "Managed"),
        }
    }
}

/// Processing strategy for SQL consumers.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ProcessingStrategy {
    /// Process rows directly in the polling task (default).
    #[default]
    Direct,
    /// Schedule processing via a separate task (deferred execution).
    Scheduled,
}

impl FromStr for ProcessingStrategy {
    type Err = CamelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Direct" => Ok(ProcessingStrategy::Direct),
            "Scheduled" => Ok(ProcessingStrategy::Scheduled),
            _ => Err(CamelError::InvalidUri(format!(
                "Unknown processing strategy: {}. Expected 'Direct' or 'Scheduled'",
                s
            ))),
        }
    }
}

impl std::fmt::Display for ProcessingStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessingStrategy::Direct => write!(f, "Direct"),
            ProcessingStrategy::Scheduled => write!(f, "Scheduled"),
        }
    }
}

/// Poll strategy for SQL consumers.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum PollStrategy {
    /// Poll sequentially with delay between polls (default).
    #[default]
    Sequential,
    /// Poll in bursts — execute multiple queries in rapid succession.
    Burst,
}

impl FromStr for PollStrategy {
    type Err = CamelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Sequential" => Ok(PollStrategy::Sequential),
            "Burst" => Ok(PollStrategy::Burst),
            _ => Err(CamelError::InvalidUri(format!(
                "Unknown poll strategy: {}. Expected 'Sequential' or 'Burst'",
                s
            ))),
        }
    }
}

impl std::fmt::Display for PollStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PollStrategy::Sequential => write!(f, "Sequential"),
            PollStrategy::Burst => write!(f, "Burst"),
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
    /// Retry policy for transient database connection failures.
    #[serde(default)]
    pub retry: NetworkRetryPolicy,
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
            .field("retry", &self.retry)
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
            retry: NetworkRetryPolicy::default(),
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

    pub fn with_retry(mut self, value: NetworkRetryPolicy) -> Self {
        self.retry = value;
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
/// the file is NOT read synchronously during `from_uri()`. Instead, the file path is
/// stored in `source_path` and the query is resolved asynchronously via `resolve_file_query()`
/// during async initialization (producer pool init or consumer start). This avoids
/// blocking I/O in the synchronous URI parsing path.
///
/// **Security note:** `Debug` implementation redacts the `db_url` (which may contain credentials)
/// and `ssl_key` path. Use `redact_db_url()` for safe logging of database URLs.
#[derive(Clone)]
pub struct SqlEndpointConfig {
    // Connection
    /// Database connection URL (optional when datasource_name is set).
    pub db_url: String,
    /// Named datasource reference (from CamelConfig.datasources).
    pub datasource_name: Option<String>,
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
    pub use_placeholder: bool,
    /// If true, don't execute the query (dry run). Default: false.
    pub noop: bool,
    /// Separator for IN clause expansion. Default: ", ".
    pub in_separator: String,

    // SQL-005: always populate statement even if body is null/empty
    /// If true, always bind parameters even if the exchange body is null/empty
    /// (uses empty defaults). Default: false.
    pub always_populate_statement: bool,

    // SQL-011: allow named parameters
    /// If true, recognize `:name` style placeholders and map them from exchange
    /// headers or body fields. Default: true.
    pub allow_named_parameters: bool,

    // SQL-016: fetch size hint
    /// Fetch size hint for query results. None = driver default.
    pub fetch_size: Option<u32>,

    // SQL-002: transaction mode
    /// Transaction mode for SQL operations. Default: Auto.
    pub transaction_mode: TransactionMode,

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
    /// Bridge poll errors into route error handling. Default: false.
    pub bridge_error_handler: bool,

    // SQL-015: repeat count for consumer polling
    /// When set, the consumer only polls up to `repeat_count` times before stopping.
    /// None = poll indefinitely (default).
    pub repeat_count: Option<u32>,

    // SQL-017: processing strategy
    /// Processing strategy for consumer. Default: Direct.
    pub processing_strategy: ProcessingStrategy,

    // SQL-018: poll strategy
    /// Poll strategy for consumer. Default: Sequential.
    pub poll_strategy: PollStrategy,

    // Producer
    /// Enable batch mode. Default: false.
    pub batch: bool,
    /// Use message body for SQL. Default: false.
    pub use_message_body_for_sql: bool,
    /// Allow queries to be sourced from exchange headers (`CamelSql.Query`) or body.
    /// Default `false` — dynamic queries are SQLi risk. Set `true` for backward compat.
    pub allow_dynamic_query: bool,

    // SSL/TLS
    /// SSL mode for the connection. None = use global default.
    pub ssl_mode: Option<String>,
    /// Path to SSL root certificate. None = use global default.
    pub ssl_root_cert: Option<String>,
    /// Path to SSL client certificate. None = use global default.
    pub ssl_cert: Option<String>,
    /// Path to SSL client key. None = use global default.
    pub ssl_key: Option<String>,

    /// Retry policy for transient database connection failures.
    pub retry: NetworkRetryPolicy,

    /// Whether `retry` was explicitly set via URI params. Used by
    /// [`apply_defaults`] to decide whether URI values win over
    /// the global config. Internal tracking flag, not serialized.
    retry_set_from_uri: bool,
}

impl std::fmt::Debug for SqlEndpointConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlEndpointConfig")
            .field("db_url", &redact_db_url(&self.db_url))
            .field("datasource_name", &self.datasource_name)
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
            .field("always_populate_statement", &self.always_populate_statement)
            .field("allow_named_parameters", &self.allow_named_parameters)
            .field("fetch_size", &self.fetch_size)
            .field("transaction_mode", &self.transaction_mode)
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
            .field("bridge_error_handler", &self.bridge_error_handler)
            .field("repeat_count", &self.repeat_count)
            .field("processing_strategy", &self.processing_strategy)
            .field("poll_strategy", &self.poll_strategy)
            .field("batch", &self.batch)
            .field("use_message_body_for_sql", &self.use_message_body_for_sql)
            .field("allow_dynamic_query", &self.allow_dynamic_query)
            .field("ssl_mode", &self.ssl_mode)
            .field("ssl_root_cert", &self.ssl_root_cert)
            .field("ssl_cert", &self.ssl_cert)
            .field("ssl_key", &redacted_opt(&self.ssl_key))
            .field("retry", &self.retry)
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
        // retry: URI wins when set_from_uri, else global fills the gap
        if !self.retry_set_from_uri {
            self.retry = defaults.retry.clone();
        }
    }

    /// Resolve any remaining None fields with built-in defaults.
    pub fn resolve_defaults(&mut self) {
        let defaults = SqlGlobalConfig::default();
        self.apply_defaults(&defaults);
    }

    /// Asynchronously read the SQL query from the file referenced by `source_path`.
    ///
    /// This is the async replacement for the blocking `std::fs::read_to_string` that
    /// was previously called in `from_uri()`. Must be invoked during async init
    /// (producer pool init or consumer start) — never in a synchronous context.
    ///
    /// After this call, `self.query` contains the file content (trimmed) and
    /// `self.source_path` is cleared to prevent re-reading.
    pub async fn resolve_file_query(&mut self) -> Result<(), CamelError> {
        if let Some(file_path) = self.source_path.take() {
            let contents = tokio::fs::read_to_string(&file_path).await.map_err(|e| {
                CamelError::Config(format!("Failed to read SQL file '{}': {}", file_path, e))
            })?;
            self.query = contents.trim().to_string();
            // Keep source_path as Some so tests can still verify the original path
            self.source_path = Some(file_path);
        }
        Ok(())
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
    enrich_db_url_with_ssl_params(
        db_url,
        config.ssl_mode.as_deref(),
        config.ssl_root_cert.as_deref(),
        config.ssl_cert.as_deref(),
        config.ssl_key.as_deref(),
    )
}

pub(crate) fn enrich_db_url_with_ssl_params(
    db_url: &str,
    ssl_mode: Option<&str>,
    ssl_root_cert: Option<&str>,
    ssl_cert: Option<&str>,
    ssl_key: Option<&str>,
) -> Result<String, CamelError> {
    let mut parsed = url::Url::parse(db_url).map_err(|e| {
        CamelError::InvalidUri(format!(
            "Cannot parse database URL for SSL enrichment: {}",
            e
        ))
    })?;

    let scheme = parsed.scheme();
    if scheme.starts_with("sqlite") {
        if ssl_mode.is_some() || ssl_root_cert.is_some() || ssl_cert.is_some() || ssl_key.is_some()
        {
            warn!(
                "SSL options configured for SQLite database URL, but SQLite does not support SSL/TLS; ignoring sslMode/sslRootCert/sslCert/sslKey"
            );
        }
        return Ok(db_url.to_string());
    }

    if scheme != "postgres" && scheme != "postgresql" && scheme != "mysql" {
        return Ok(db_url.to_string());
    }
    let is_mysql = scheme == "mysql";

    // Compute effective ssl_mode with per-driver default when none is specified.
    let effective_ssl_mode: &str = ssl_mode.unwrap_or(if is_mysql { "prefer" } else { "require" });

    let ssl_params: Vec<(&str, &str)> = [
        Some(("sslMode", effective_ssl_mode)),
        ssl_root_cert.map(|v| ("sslRootCert", v)),
        ssl_cert.map(|v| ("sslCert", v)),
        ssl_key.map(|v| ("sslKey", v)),
    ]
    .into_iter()
    .flatten()
    .collect();

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

    // Add TCP connect timeout (10s) if not already set.
    // This is the sqlx URL-level connect_timeout, NOT the pool acquire_timeout.
    if !query_pairs.iter().any(|(k, _)| k == "connect_timeout") {
        query_pairs.push(("connect_timeout".into(), "10".into()));
    }

    {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &query_pairs {
            serializer.append_pair(k, v);
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
        // SQL-014: defer file reading to async init path to avoid blocking I/O
        // in the synchronous URI parsing path. Store the path; resolve_file_query()
        // must be called during async initialization (producer pool init or consumer start).
        let (query, source_path) = if parts.path.starts_with("file:") {
            let file_path = parts.path.trim_start_matches("file:").to_string();
            (String::new(), Some(file_path))
        } else {
            (parts.path.clone(), None)
        };

        // Optional parameter: db_url (required when datasource is not set)
        let db_url = params.get("db_url").cloned().unwrap_or_default();

        // Named datasource reference (from CamelConfig.datasources)
        let datasource_name = params.get("datasource").cloned();

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
                if !v.is_ascii() {
                    return Err(CamelError::InvalidUri(
                        "placeholder must be a single ASCII character".to_string(),
                    ));
                }
                Ok(v.chars().next().unwrap()) // allow-unwrap
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

        // SQL-005: alwaysPopulateStatement
        let always_populate_statement = params
            .get("alwaysPopulateStatement")
            .map(|v| parse_bool_param("alwaysPopulateStatement", v))
            .transpose()?
            .unwrap_or(false);

        // SQL-011: allowNamedParameters
        let allow_named_parameters = params
            .get("allowNamedParameters")
            .map(|v| parse_bool_param("allowNamedParameters", v))
            .transpose()?
            .unwrap_or(true);

        // SQL-016: fetchSize
        let fetch_size = params.get("fetchSize").and_then(|v| v.parse().ok());

        // SQL-002: transactionMode
        let transaction_mode = params
            .get("transactionMode")
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or_default();

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
        let bridge_error_handler = params
            .get("bridgeErrorHandler")
            .map(|v| parse_bool_param("bridgeErrorHandler", v))
            .transpose()?
            .unwrap_or(false);

        // SQL-015: repeatCount
        let repeat_count = params.get("repeatCount").and_then(|v| v.parse().ok());

        // SQL-017: processingStrategy
        let processing_strategy = params
            .get("processingStrategy")
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or_default();

        // SQL-018: pollStrategy
        let poll_strategy = params
            .get("pollStrategy")
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or_default();

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
        let allow_dynamic_query = params
            .get("allowDynamicQuery")
            .map(|v| parse_bool_param("allowDynamicQuery", v))
            .transpose()?
            .unwrap_or(false);
        let ssl_mode = params.get("sslMode").cloned();
        let ssl_root_cert = params.get("sslRootCert").cloned();
        let ssl_cert = params.get("sslCert").cloned();
        let ssl_key = params.get("sslKey").cloned();

        // Parse retry policy from URI params
        let mut retry = NetworkRetryPolicy::default();
        let mut retry_set_from_uri = false;
        if let Some(raw) = params.get("retryEnabled") {
            retry.enabled = raw.parse::<bool>().map_err(|_| {
                CamelError::InvalidUri(format!("retryEnabled must be a boolean, got '{raw}'"))
            })?;
            retry_set_from_uri = true;
        }
        if let Some(raw) = params.get("retryMaxAttempts") {
            retry.max_attempts = raw.parse::<u32>().map_err(|_| {
                CamelError::InvalidUri(format!("retryMaxAttempts must be a u32, got '{raw}'"))
            })?;
            retry_set_from_uri = true;
        }
        if let Some(raw) = params.get("retryInitialDelayMs") {
            retry.initial_delay = Duration::from_millis(raw.parse::<u64>().map_err(|_| {
                CamelError::InvalidUri(format!("retryInitialDelayMs must be a u64, got '{raw}'"))
            })?);
            retry_set_from_uri = true;
        }
        if let Some(raw) = params.get("retryMultiplier") {
            retry.multiplier = raw.parse::<f64>().map_err(|_| {
                CamelError::InvalidUri(format!("retryMultiplier must be a f64, got '{raw}'"))
            })?;
            retry_set_from_uri = true;
        }
        if let Some(raw) = params.get("retryMaxDelayMs") {
            retry.max_delay = Duration::from_millis(raw.parse::<u64>().map_err(|_| {
                CamelError::InvalidUri(format!("retryMaxDelayMs must be a u64, got '{raw}'"))
            })?);
            retry_set_from_uri = true;
        }
        if let Some(raw) = params.get("retryJitter") {
            retry.jitter_factor = raw.parse::<f64>().map_err(|_| {
                CamelError::InvalidUri(format!("retryJitter must be a f64, got '{raw}'"))
            })?;
            retry_set_from_uri = true;
        }

        if datasource_name.is_none() && db_url.is_empty() {
            return Err(CamelError::Config(
                "either 'datasource' or 'db_url' parameter is required".to_string(),
            ));
        }

        if datasource_name.is_some() && !db_url.is_empty() {
            return Err(CamelError::InvalidUri(
                "'db_url' not allowed with named datasource — use 'datasource' alone".to_string(),
            ));
        }

        if datasource_name.is_some() {
            let overrides: Vec<&str> = {
                let mut v = Vec::new();
                if max_connections.is_some() {
                    v.push("maxConnections");
                }
                if min_connections.is_some() {
                    v.push("minConnections");
                }
                if idle_timeout_secs.is_some() {
                    v.push("idleTimeoutSecs");
                }
                if max_lifetime_secs.is_some() {
                    v.push("maxLifetimeSecs");
                }
                if ssl_mode.is_some() {
                    v.push("sslMode");
                }
                if ssl_root_cert.is_some() {
                    v.push("sslRootCert");
                }
                if ssl_cert.is_some() {
                    v.push("sslCert");
                }
                if ssl_key.is_some() {
                    v.push("sslKey");
                }
                v
            };
            if !overrides.is_empty() {
                return Err(CamelError::InvalidUri(format!(
                    "pool-affecting params not allowed with named datasource: {}",
                    overrides.join(", ")
                )));
            }
        }

        Ok(Self {
            db_url,
            datasource_name,
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
            always_populate_statement,
            allow_named_parameters,
            fetch_size,
            transaction_mode,
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
            bridge_error_handler,
            repeat_count,
            processing_strategy,
            poll_strategy,
            batch,
            use_message_body_for_sql,
            allow_dynamic_query,
            ssl_mode,
            ssl_root_cert,
            ssl_cert,
            ssl_key,
            retry,
            retry_set_from_uri,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::NetworkRetryPolicy;

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
        assert!(!c.allow_dynamic_query);
        assert!(c.ssl_mode.is_none());
        assert!(c.ssl_root_cert.is_none());
        assert!(c.ssl_cert.is_none());
        assert!(c.ssl_key.is_none());
        // SQL-005/SQL-011/SQL-016/SQL-002/SQL-015/SQL-017/SQL-018 defaults
        assert!(!c.always_populate_statement);
        assert!(c.allow_named_parameters);
        assert!(c.fetch_size.is_none());
        assert_eq!(c.transaction_mode, TransactionMode::Auto);
        assert!(c.repeat_count.is_none());
        assert_eq!(c.processing_strategy, ProcessingStrategy::Direct);
        assert_eq!(c.poll_strategy, PollStrategy::Sequential);
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
        assert!(!c.bridge_error_handler);
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

    // SQL-014: file-not-found is now detected during async resolve_file_query(), not from_uri
    #[tokio::test]
    async fn config_file_not_found() {
        let mut config = SqlEndpointConfig::from_uri(
            "sql:file:/nonexistent/path/query.sql?db_url=postgres://localhost/test",
        )
        .expect("from_uri should defer file reading");
        // from_uri no longer reads the file — source_path is set, query is empty
        assert_eq!(
            config.source_path,
            Some("/nonexistent/path/query.sql".to_string())
        );
        assert!(config.query.is_empty());

        // Error occurs during async resolution
        let result = config.resolve_file_query().await;
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("Failed to read SQL file") || msg.contains("nonexistent"));
    }

    // SQL-014: file query is now resolved asynchronously
    #[tokio::test]
    async fn config_file_query() {
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
        let mut c = SqlEndpointConfig::from_uri(&uri).unwrap();
        // query is empty until async resolution
        assert!(c.query.is_empty());
        assert_eq!(c.source_path, Some(tmp.to_string_lossy().into_owned()));

        // Resolve asynchronously
        c.resolve_file_query()
            .await
            .expect("file query should resolve");
        assert_eq!(c.query, "SELECT * FROM users");
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
            retry: NetworkRetryPolicy::default(),
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
            retry: NetworkRetryPolicy::default(),
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
    fn enrich_applies_postgres_defaults() {
        let mut c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        c.resolve_defaults();
        let url = enrich_db_url_with_ssl(&c.db_url, &c).unwrap();
        // Default ssl_mode and connect_timeout are applied at enrichment time
        assert!(
            url.contains("sslmode=require"),
            "expected sslmode=require, got: {}",
            url
        );
        assert!(
            url.contains("connect_timeout=10"),
            "expected connect_timeout=10, got: {}",
            url
        );
    }

    #[test]
    fn enrich_mysql_defaults_to_prefer() {
        let mut c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=mysql://localhost/test").unwrap();
        c.resolve_defaults();
        let url = enrich_db_url_with_ssl(&c.db_url, &c).unwrap();
        assert!(
            url.contains("ssl-mode=prefer"),
            "expected ssl-mode=prefer, got: {}",
            url
        );
        assert!(
            url.contains("connect_timeout=10"),
            "expected connect_timeout=10, got: {}",
            url
        );
    }

    #[test]
    fn enrich_preserves_explicit_connect_timeout() {
        let mut c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test?connect_timeout=5",
        )
        .unwrap();
        c.resolve_defaults();
        let url = enrich_db_url_with_ssl(&c.db_url, &c).unwrap();
        assert!(
            url.contains("connect_timeout=5"),
            "expected explicit connect_timeout=5 preserved, got: {}",
            url
        );
        assert!(
            url.contains("sslmode=require"),
            "expected sslmode=require, got: {}",
            url
        );
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
        let result =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test&noop=1");
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("noop"));
    }

    #[test]
    fn batch_rejects_invalid_value() {
        let result =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test&batch=yes");
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
            "sql:select 1?db_url=postgres://localhost/test&usePlaceholder=TRUE&noop=FALSE&batch=True&useIterator=False&bridgeErrorHandler=TRUE",
        )
        .unwrap();
        assert!(c.use_placeholder);
        assert!(!c.noop);
        assert!(c.batch);
        assert!(!c.use_iterator);
        assert!(c.bridge_error_handler);
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
    fn non_ascii_placeholder_rejected() {
        let result = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&placeholder=%C2%A2",
        );
        assert!(result.is_err());
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

    // SQL-014: file-based SQL config test (verifies async resolution and caching)
    #[tokio::test]
    async fn file_query_cached_in_config() {
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
        let mut c = SqlEndpointConfig::from_uri(&uri).unwrap();
        // Query is empty before async resolution
        assert!(c.query.is_empty());
        assert_eq!(c.source_path, Some(tmp.to_string_lossy().into_owned()));

        // Resolve asynchronously — query is cached in config
        c.resolve_file_query()
            .await
            .expect("resolve should succeed");
        assert_eq!(c.query, "SELECT * FROM cached_test");

        // Delete the file — config still has the query
        std::fs::remove_file(&tmp).ok();
        assert_eq!(c.query, "SELECT * FROM cached_test");
    }

    // --- H-03 audit sweep tests ---

    // SQL-005: alwaysPopulateStatement
    #[test]
    fn always_populate_statement_defaults_to_false() {
        let c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        assert!(!c.always_populate_statement);
    }

    #[test]
    fn always_populate_statement_from_uri() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&alwaysPopulateStatement=true",
        )
        .unwrap();
        assert!(c.always_populate_statement);
    }

    // SQL-011: allowNamedParameters
    #[test]
    fn allow_named_parameters_defaults_to_true() {
        let c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        assert!(c.allow_named_parameters);
    }

    #[test]
    fn allow_named_parameters_false_from_uri() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&allowNamedParameters=false",
        )
        .unwrap();
        assert!(!c.allow_named_parameters);
    }

    // SQL-016: fetchSize
    #[test]
    fn fetch_size_defaults_to_none() {
        let c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        assert!(c.fetch_size.is_none());
    }

    #[test]
    fn fetch_size_from_uri() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&fetchSize=1000",
        )
        .unwrap();
        assert_eq!(c.fetch_size, Some(1000));
    }

    // SQL-002: transactionMode
    #[test]
    fn transaction_mode_defaults_to_auto() {
        let c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        assert_eq!(c.transaction_mode, TransactionMode::Auto);
    }

    #[test]
    fn transaction_mode_managed_from_uri() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&transactionMode=Managed",
        )
        .unwrap();
        assert_eq!(c.transaction_mode, TransactionMode::Managed);
    }

    #[test]
    fn transaction_mode_invalid_rejected() {
        let result = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&transactionMode=Invalid",
        );
        assert!(result.is_err());
    }

    // SQL-015: repeatCount
    #[test]
    fn repeat_count_defaults_to_none() {
        let c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        assert!(c.repeat_count.is_none());
    }

    #[test]
    fn repeat_count_from_uri() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&repeatCount=10",
        )
        .unwrap();
        assert_eq!(c.repeat_count, Some(10));
    }

    // SQL-017: processingStrategy
    #[test]
    fn processing_strategy_defaults_to_direct() {
        let c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        assert_eq!(c.processing_strategy, ProcessingStrategy::Direct);
    }

    #[test]
    fn processing_strategy_scheduled_from_uri() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&processingStrategy=Scheduled",
        )
        .unwrap();
        assert_eq!(c.processing_strategy, ProcessingStrategy::Scheduled);
    }

    #[test]
    fn processing_strategy_invalid_rejected() {
        let result = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&processingStrategy=Invalid",
        );
        assert!(result.is_err());
    }

    // SQL-018: pollStrategy
    #[test]
    fn poll_strategy_defaults_to_sequential() {
        let c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        assert_eq!(c.poll_strategy, PollStrategy::Sequential);
    }

    #[test]
    fn poll_strategy_burst_from_uri() {
        let c = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&pollStrategy=Burst",
        )
        .unwrap();
        assert_eq!(c.poll_strategy, PollStrategy::Burst);
    }

    #[test]
    fn poll_strategy_invalid_rejected() {
        let result = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&pollStrategy=Invalid",
        );
        assert!(result.is_err());
    }

    // ── RetryPolicy (rc-ddl) ──────────────────────────────────────────────

    #[test]
    fn sql_endpoint_config_has_retry_policy() {
        let cfg = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=sqlite::memory:&retryMaxAttempts=3&retryInitialDelayMs=500",
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
    fn sql_endpoint_config_retry_defaults_when_unspecified() {
        let cfg =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=sqlite::memory:").expect("parse");
        // When URI has no retry params, retry defaults to NetworkRetryPolicy::default()
        assert!(cfg.retry.enabled);
        assert_eq!(cfg.retry.max_attempts, 10); // default
    }

    #[test]
    fn sql_global_config_has_retry_default() {
        let cfg = SqlGlobalConfig::default();
        assert!(cfg.retry.enabled);
    }

    #[test]
    fn retry_policy_parse_full_uri_params() {
        let cfg = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=sqlite::memory:&retryEnabled=false&retryMaxAttempts=7&retryInitialDelayMs=1000&retryMultiplier=3.0&retryMaxDelayMs=60000&retryJitter=0.5",
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
    fn retry_policy_from_uri_survives_apply_defaults_with_global() {
        let mut ep = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=sqlite::memory:&retryMaxAttempts=10&retryInitialDelayMs=500",
        )
        .expect("parse");
        let global = SqlGlobalConfig::default(); // global has default retry (max_attempts=10)
        ep.apply_defaults(&global);
        // URI values survive when retry_set_from_uri is true
        assert_eq!(ep.retry.max_attempts, 10);
        assert_eq!(
            ep.retry.initial_delay,
            std::time::Duration::from_millis(500)
        );
    }

    #[test]
    fn retry_policy_falls_back_to_global_when_uri_has_no_retry_params() {
        let mut ep =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=sqlite::memory:").expect("parse");
        let mut global = SqlGlobalConfig::default();
        global.retry.max_attempts = 7;
        ep.apply_defaults(&global);
        // When URI has no retry params, global fills the gap
        assert_eq!(ep.retry.max_attempts, 7);
    }

    #[test]
    fn from_uri_with_datasource_name() {
        let cfg = SqlEndpointConfig::from_uri("sql:SELECT 1?datasource=orders").unwrap();
        assert_eq!(cfg.datasource_name.as_deref(), Some("orders"));
        assert!(cfg.db_url.is_empty());
    }

    #[test]
    fn from_uri_with_datasource_and_behavior_override() {
        let cfg =
            SqlEndpointConfig::from_uri("sql:SELECT 1?datasource=orders&outputType=SelectOne")
                .unwrap();
        assert_eq!(cfg.datasource_name.as_deref(), Some("orders"));
    }

    #[test]
    fn from_uri_datasource_rejects_pool_override() {
        let result =
            SqlEndpointConfig::from_uri("sql:SELECT 1?datasource=orders&maxConnections=50");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("pool-affecting"));
    }

    #[test]
    fn from_uri_neither_datasource_nor_db_url_is_error() {
        let result = SqlEndpointConfig::from_uri("sql:SELECT 1");
        assert!(result.is_err());
    }

    #[test]
    fn from_uri_db_url_inline_still_works() {
        let cfg =
            SqlEndpointConfig::from_uri("sql:SELECT 1?db_url=postgres://localhost/test").unwrap();
        assert!(cfg.datasource_name.is_none());
        assert_eq!(cfg.db_url, "postgres://localhost/test");
    }

    #[test]
    fn from_uri_datasource_rejects_ssl_mode() {
        let result = SqlEndpointConfig::from_uri("sql:SELECT 1?datasource=orders&sslMode=require");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("pool-affecting"));
    }

    #[test]
    fn from_uri_datasource_rejects_ssl_root_cert() {
        let result =
            SqlEndpointConfig::from_uri("sql:SELECT 1?datasource=orders&sslRootCert=/ca.pem");
        assert!(result.is_err());
    }

    #[test]
    fn from_uri_datasource_rejects_db_url() {
        let result = SqlEndpointConfig::from_uri(
            "sql:SELECT 1?datasource=orders&db_url=postgres://evil:5432/pwned",
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("db_url") && msg.contains("datasource"));
    }
}
