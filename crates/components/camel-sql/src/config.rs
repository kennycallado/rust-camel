use std::str::FromStr;

use camel_api::CamelError;
use camel_endpoint::parse_uri;

/// Output type for SQL query results.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlOutputType {
    /// Return all rows as a list.
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

/// Configuration for SQL component endpoints.
#[derive(Debug, Clone)]
pub struct SqlConfig {
    // Connection
    pub db_url: String,
    pub max_connections: u32,
    pub min_connections: u32,
    pub idle_timeout_secs: u64,
    pub max_lifetime_secs: u64,

    // Query
    pub query: String,
    pub output_type: SqlOutputType,
    pub placeholder: char,
    pub separator: char,
    pub noop: bool,

    // Consumer (polling)
    pub delay_ms: u64,
    pub initial_delay_ms: u64,
    pub max_messages_per_poll: Option<i32>,
    pub on_consume: Option<String>,
    pub on_consume_failed: Option<String>,
    pub on_consume_batch_complete: Option<String>,
    pub route_empty_result_set: bool,
    pub use_iterator: bool,
    pub expected_update_count: Option<i64>,
    pub break_batch_on_consume_fail: bool,

    // Producer
    pub batch: bool,
    pub use_message_body_for_sql: bool,
}

impl SqlConfig {
    /// Parse configuration from a Camel-style URI.
    ///
    /// URI format: `sql:<query>?db_url=<url>&param1=val1&param2=val2`
    ///
    /// # Errors
    ///
    /// Returns `CamelError::InvalidUri` if the scheme is not "sql".
    /// Returns `CamelError::Config` if `db_url` parameter is missing.
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let components = parse_uri(uri)?;

        // Validate scheme
        if components.scheme != "sql" {
            return Err(CamelError::InvalidUri(format!(
                "Expected scheme 'sql', got '{}'",
                components.scheme
            )));
        }

        // Extract required db_url
        let db_url = components
            .params
            .get("db_url")
            .ok_or_else(|| CamelError::Config("db_url parameter is required".to_string()))?
            .clone();

        // Helper functions for parameter extraction
        let get_param = |name: &str| -> Option<&String> { components.params.get(name) };

        let get_u32 = |name: &str, default: u32| -> u32 {
            get_param(name)
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };

        let get_u64 = |name: &str, default: u64| -> u64 {
            get_param(name)
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };

        let get_i32 = |name: &str| -> Option<i32> { get_param(name).and_then(|v| v.parse().ok()) };

        let get_i64 = |name: &str| -> Option<i64> { get_param(name).and_then(|v| v.parse().ok()) };

        let get_bool = |name: &str, default: bool| -> bool {
            get_param(name)
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(default)
        };

        let get_char = |name: &str, default: char| -> char {
            get_param(name)
                .filter(|v| !v.is_empty())
                .map(|v| v.chars().next().unwrap())
                .unwrap_or(default)
        };

        let get_string = |name: &str| -> Option<String> { get_param(name).cloned() };

        // Build config with defaults
        Ok(SqlConfig {
            // Connection
            db_url,
            max_connections: get_u32("maxConnections", 5),
            min_connections: get_u32("minConnections", 1),
            idle_timeout_secs: get_u64("idleTimeoutSecs", 300),
            max_lifetime_secs: get_u64("maxLifetimeSecs", 1800),

            // Query
            query: components.path,
            output_type: get_param("outputType")
                .map(|s| s.parse())
                .transpose()?
                .unwrap_or(SqlOutputType::SelectList),
            placeholder: get_char("placeholder", '#'),
            separator: get_char("separator", ','),
            noop: get_bool("noop", false),

            // Consumer
            delay_ms: get_u64("delay", 500),
            initial_delay_ms: get_u64("initialDelay", 1000),
            max_messages_per_poll: get_i32("maxMessagesPerPoll"),
            on_consume: get_string("onConsume"),
            on_consume_failed: get_string("onConsumeFailed"),
            on_consume_batch_complete: get_string("onConsumeBatchComplete"),
            route_empty_result_set: get_bool("routeEmptyResultSet", false),
            use_iterator: get_bool("useIterator", true),
            expected_update_count: get_i64("expectedUpdateCount"),
            break_batch_on_consume_fail: get_bool("breakBatchOnConsumeFail", false),

            // Producer
            batch: get_bool("batch", false),
            use_message_body_for_sql: get_bool("useMessageBodyForSql", false),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let c = SqlConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        assert_eq!(c.query, "select 1");
        assert_eq!(c.db_url, "postgres://localhost/test");
        assert_eq!(c.max_connections, 5);
        assert_eq!(c.min_connections, 1);
        assert_eq!(c.idle_timeout_secs, 300);
        assert_eq!(c.max_lifetime_secs, 1800);
        assert_eq!(c.output_type, SqlOutputType::SelectList);
        assert_eq!(c.placeholder, '#');
        assert_eq!(c.separator, ',');
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
    fn test_config_wrong_scheme() {
        assert!(SqlConfig::from_uri("redis://localhost:6379").is_err());
    }

    #[test]
    fn test_config_missing_db_url() {
        assert!(SqlConfig::from_uri("sql:select 1").is_err());
    }

    #[test]
    fn test_config_output_type_select_one() {
        let c = SqlConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&outputType=SelectOne",
        )
        .unwrap();
        assert_eq!(c.output_type, SqlOutputType::SelectOne);
    }

    #[test]
    fn test_config_output_type_stream_list() {
        let c = SqlConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&outputType=StreamList",
        )
        .unwrap();
        assert_eq!(c.output_type, SqlOutputType::StreamList);
    }

    #[test]
    fn test_config_consumer_options() {
        let c = SqlConfig::from_uri(
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
    fn test_config_producer_options() {
        let c = SqlConfig::from_uri(
            "sql:insert into t values (#)?db_url=postgres://localhost/test&batch=true&useMessageBodyForSql=true&noop=true"
        ).unwrap();
        assert!(c.batch);
        assert!(c.use_message_body_for_sql);
        assert!(c.noop);
    }

    #[test]
    fn test_config_pool_options() {
        let c = SqlConfig::from_uri(
            "sql:select 1?db_url=postgres://localhost/test&maxConnections=20&minConnections=3&idleTimeoutSecs=600&maxLifetimeSecs=3600"
        ).unwrap();
        assert_eq!(c.max_connections, 20);
        assert_eq!(c.min_connections, 3);
        assert_eq!(c.idle_timeout_secs, 600);
        assert_eq!(c.max_lifetime_secs, 3600);
    }

    #[test]
    fn test_config_query_with_special_chars() {
        let c = SqlConfig::from_uri(
            "sql:select * from users where name = :#name and age > #?db_url=postgres://localhost/test",
        )
        .unwrap();
        assert_eq!(
            c.query,
            "select * from users where name = :#name and age > #"
        );
    }

    #[test]
    fn test_output_type_from_str() {
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
}
