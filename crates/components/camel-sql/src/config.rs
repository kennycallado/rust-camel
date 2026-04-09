use std::str::FromStr;

use camel_component_api::CamelError;
use camel_component_api::{UriComponents, UriConfig, parse_uri};

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
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SqlGlobalConfig {
    pub max_connections: u32,
    pub min_connections: u32,
    pub idle_timeout_secs: u64,
    pub max_lifetime_secs: u64,
}

impl Default for SqlGlobalConfig {
    fn default() -> Self {
        Self {
            max_connections: 5,
            min_connections: 1,
            idle_timeout_secs: 300,
            max_lifetime_secs: 1800,
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
}

/// Configuration for SQL component endpoints.
///
/// URI format: `sql:<query>?db_url=<url>&param1=val1&param2=val2`
///
/// The query can be inline SQL or a file reference with `file:` prefix:
/// - `sql:SELECT * FROM users?db_url=...` - inline SQL
/// - `sql:file:/path/to/query.sql?db_url=...` - read SQL from file
#[derive(Debug, Clone)]
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
    /// If true, don't execute the query (dry run). Default: false.
    pub noop: bool,

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
    }

    /// Resolve any remaining None fields with built-in defaults.
    pub fn resolve_defaults(&mut self) {
        let defaults = SqlGlobalConfig::default();
        self.apply_defaults(&defaults);
    }
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
            .map(|v| v.chars().next().unwrap())
            .unwrap_or('#');
        let noop = params
            .get("noop")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

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
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let use_iterator = params
            .get("useIterator")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);
        let expected_update_count = params
            .get("expectedUpdateCount")
            .and_then(|v| v.parse().ok());
        let break_batch_on_consume_fail = params
            .get("breakBatchOnConsumeFail")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        // Producer parameters
        let batch = params
            .get("batch")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let use_message_body_for_sql = params
            .get("useMessageBodyForSql")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

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
            noop,
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
        };
        c.apply_defaults(&global);
        assert_eq!(c.max_connections, Some(10));
        assert_eq!(c.min_connections, Some(2));
        assert_eq!(c.idle_timeout_secs, Some(600));
        assert_eq!(c.max_lifetime_secs, Some(3600));
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
            .with_max_lifetime_secs(3600);
        assert_eq!(c.max_connections, 20);
        assert_eq!(c.min_connections, 3);
        assert_eq!(c.idle_timeout_secs, 600);
        assert_eq!(c.max_lifetime_secs, 3600);
    }
}
