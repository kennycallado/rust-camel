use camel_api::CamelError;
use camel_endpoint::parse_uri;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq)]
pub enum RedisCommand {
    // String operations
    Set,
    Get,
    Getset,
    Setnx,
    Setex,
    Mget,
    Mset,
    Incr,
    Incrby,
    Decr,
    Decrby,
    Append,
    Strlen,

    // Key operations
    Exists,
    Del,
    Expire,
    Expireat,
    Pexpire,
    Pexpireat,
    Ttl,
    Keys,
    Rename,
    Renamenx,
    Type,
    Persist,
    Move,
    Sort,

    // List operations
    Lpush,
    Rpush,
    Lpushx,
    Rpushx,
    Lpop,
    Rpop,
    Blpop,
    Brpop,
    Llen,
    Lrange,
    Lindex,
    Linsert,
    Lset,
    Lrem,
    Ltrim,
    Rpoplpush,

    // Hash operations
    Hset,
    Hget,
    Hsetnx,
    Hmset,
    Hmget,
    Hdel,
    Hexists,
    Hlen,
    Hkeys,
    Hvals,
    Hgetall,
    Hincrby,

    // Set operations
    Sadd,
    Srem,
    Smembers,
    Scard,
    Sismember,
    Spop,
    Smove,
    Sinter,
    Sunion,
    Sdiff,
    Sinterstore,
    Sunionstore,
    Sdiffstore,
    Srandmember,

    // Sorted set operations
    Zadd,
    Zrem,
    Zrange,
    Zrevrange,
    Zrank,
    Zrevrank,
    Zscore,
    Zcard,
    Zincrby,
    Zcount,
    Zrangebyscore,
    Zrevrangebyscore,
    Zremrangebyrank,
    Zremrangebyscore,
    Zunionstore,
    Zinterstore,

    // Pub/Sub operations
    Publish,
    Subscribe,
    Psubscribe,

    // Other operations
    Ping,
    Echo,
}

impl FromStr for RedisCommand {
    type Err = CamelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            // String operations
            "SET" => Ok(RedisCommand::Set),
            "GET" => Ok(RedisCommand::Get),
            "GETSET" => Ok(RedisCommand::Getset),
            "SETNX" => Ok(RedisCommand::Setnx),
            "SETEX" => Ok(RedisCommand::Setex),
            "MGET" => Ok(RedisCommand::Mget),
            "MSET" => Ok(RedisCommand::Mset),
            "INCR" => Ok(RedisCommand::Incr),
            "INCRBY" => Ok(RedisCommand::Incrby),
            "DECR" => Ok(RedisCommand::Decr),
            "DECRBY" => Ok(RedisCommand::Decrby),
            "APPEND" => Ok(RedisCommand::Append),
            "STRLEN" => Ok(RedisCommand::Strlen),

            // Key operations
            "EXISTS" => Ok(RedisCommand::Exists),
            "DEL" => Ok(RedisCommand::Del),
            "EXPIRE" => Ok(RedisCommand::Expire),
            "EXPIREAT" => Ok(RedisCommand::Expireat),
            "PEXPIRE" => Ok(RedisCommand::Pexpire),
            "PEXPIREAT" => Ok(RedisCommand::Pexpireat),
            "TTL" => Ok(RedisCommand::Ttl),
            "KEYS" => Ok(RedisCommand::Keys),
            "RENAME" => Ok(RedisCommand::Rename),
            "RENAMENX" => Ok(RedisCommand::Renamenx),
            "TYPE" => Ok(RedisCommand::Type),
            "PERSIST" => Ok(RedisCommand::Persist),
            "MOVE" => Ok(RedisCommand::Move),
            "SORT" => Ok(RedisCommand::Sort),

            // List operations
            "LPUSH" => Ok(RedisCommand::Lpush),
            "RPUSH" => Ok(RedisCommand::Rpush),
            "LPUSHX" => Ok(RedisCommand::Lpushx),
            "RPUSHX" => Ok(RedisCommand::Rpushx),
            "LPOP" => Ok(RedisCommand::Lpop),
            "RPOP" => Ok(RedisCommand::Rpop),
            "BLPOP" => Ok(RedisCommand::Blpop),
            "BRPOP" => Ok(RedisCommand::Brpop),
            "LLEN" => Ok(RedisCommand::Llen),
            "LRANGE" => Ok(RedisCommand::Lrange),
            "LINDEX" => Ok(RedisCommand::Lindex),
            "LINSERT" => Ok(RedisCommand::Linsert),
            "LSET" => Ok(RedisCommand::Lset),
            "LREM" => Ok(RedisCommand::Lrem),
            "LTRIM" => Ok(RedisCommand::Ltrim),
            "RPOPLPUSH" => Ok(RedisCommand::Rpoplpush),

            // Hash operations
            "HSET" => Ok(RedisCommand::Hset),
            "HGET" => Ok(RedisCommand::Hget),
            "HSETNX" => Ok(RedisCommand::Hsetnx),
            "HMSET" => Ok(RedisCommand::Hmset),
            "HMGET" => Ok(RedisCommand::Hmget),
            "HDEL" => Ok(RedisCommand::Hdel),
            "HEXISTS" => Ok(RedisCommand::Hexists),
            "HLEN" => Ok(RedisCommand::Hlen),
            "HKEYS" => Ok(RedisCommand::Hkeys),
            "HVALS" => Ok(RedisCommand::Hvals),
            "HGETALL" => Ok(RedisCommand::Hgetall),
            "HINCRBY" => Ok(RedisCommand::Hincrby),

            // Set operations
            "SADD" => Ok(RedisCommand::Sadd),
            "SREM" => Ok(RedisCommand::Srem),
            "SMEMBERS" => Ok(RedisCommand::Smembers),
            "SCARD" => Ok(RedisCommand::Scard),
            "SISMEMBER" => Ok(RedisCommand::Sismember),
            "SPOP" => Ok(RedisCommand::Spop),
            "SMOVE" => Ok(RedisCommand::Smove),
            "SINTER" => Ok(RedisCommand::Sinter),
            "SUNION" => Ok(RedisCommand::Sunion),
            "SDIFF" => Ok(RedisCommand::Sdiff),
            "SINTERSTORE" => Ok(RedisCommand::Sinterstore),
            "SUNIONSTORE" => Ok(RedisCommand::Sunionstore),
            "SDIFFSTORE" => Ok(RedisCommand::Sdiffstore),
            "SRANDMEMBER" => Ok(RedisCommand::Srandmember),

            // Sorted set operations
            "ZADD" => Ok(RedisCommand::Zadd),
            "ZREM" => Ok(RedisCommand::Zrem),
            "ZRANGE" => Ok(RedisCommand::Zrange),
            "ZREVRANGE" => Ok(RedisCommand::Zrevrange),
            "ZRANK" => Ok(RedisCommand::Zrank),
            "ZREVRANK" => Ok(RedisCommand::Zrevrank),
            "ZSCORE" => Ok(RedisCommand::Zscore),
            "ZCARD" => Ok(RedisCommand::Zcard),
            "ZINCRBY" => Ok(RedisCommand::Zincrby),
            "ZCOUNT" => Ok(RedisCommand::Zcount),
            "ZRANGEBYSCORE" => Ok(RedisCommand::Zrangebyscore),
            "ZREVRANGEBYSCORE" => Ok(RedisCommand::Zrevrangebyscore),
            "ZREMRANGEBYRANK" => Ok(RedisCommand::Zremrangebyrank),
            "ZREMRANGEBYSCORE" => Ok(RedisCommand::Zremrangebyscore),
            "ZUNIONSTORE" => Ok(RedisCommand::Zunionstore),
            "ZINTERSTORE" => Ok(RedisCommand::Zinterstore),

            // Pub/Sub operations
            "PUBLISH" => Ok(RedisCommand::Publish),
            "SUBSCRIBE" => Ok(RedisCommand::Subscribe),
            "PSUBSCRIBE" => Ok(RedisCommand::Psubscribe),

            // Other operations
            "PING" => Ok(RedisCommand::Ping),
            "ECHO" => Ok(RedisCommand::Echo),

            _ => Err(CamelError::InvalidUri(format!(
                "Unknown Redis command: {}",
                s
            ))),
        }
    }
}

// --- RedisConfig (global defaults, no serde) ---

/// Global Redis configuration defaults.
///
/// This struct holds component-level defaults that can be set via YAML config
/// and applied to endpoint configurations when specific values aren't provided.
#[derive(Debug, Clone, PartialEq)]
pub struct RedisConfig {
    pub host: String,
    pub port: u16,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 6379,
        }
    }
}

impl RedisConfig {
    pub fn with_host(mut self, v: impl Into<String>) -> Self {
        self.host = v.into();
        self
    }

    pub fn with_port(mut self, v: u16) -> Self {
        self.port = v;
        self
    }
}

// --- RedisEndpointConfig (parsed from URI) ---

/// Configuration parsed from a Redis URI.
///
/// Format: `redis://host:port?command=GET&...` or `redis://?command=GET` (no host/port)
///
/// # Fields with Global Defaults (Option<T>)
///
/// These fields can be set via global defaults in `Camel.toml`. They are `Option<T>`
/// to distinguish between "not set by URI" (`None`) and "explicitly set by URI" (`Some(v)`).
/// After calling `apply_defaults()` + `resolve_defaults()`, all are guaranteed `Some`.
///
/// - `host` - Redis server hostname
/// - `port` - Redis server port
///
/// # Fields Without Global Defaults
///
/// These fields are per-endpoint only and have no global defaults:
///
/// - `command` - Redis command to execute (default: SET)
/// - `channels` - Channels for pub/sub operations (default: empty)
/// - `key` - Key for operations that require it (default: None)
/// - `timeout` - Timeout in seconds for blocking operations (default: 1)
/// - `password` - Redis password for authentication (default: None)
/// - `db` - Redis database number (default: 0)
#[derive(Debug, Clone)]
pub struct RedisEndpointConfig {
    /// Redis server hostname. `None` if not set in URI.
    /// Filled by `apply_defaults()` from global config, then `resolve_defaults()`.
    pub host: Option<String>,

    /// Redis server port. `None` if not set in URI.
    /// Filled by `apply_defaults()` from global config, then `resolve_defaults()`.
    pub port: Option<u16>,

    /// Redis command to execute. Default: SET.
    pub command: RedisCommand,

    /// Channels for pub/sub operations. Default: empty.
    pub channels: Vec<String>,

    /// Key for operations that require it. Default: None.
    pub key: Option<String>,

    /// Timeout in seconds for blocking operations. Default: 1.
    pub timeout: u64,

    /// Redis password for authentication. Default: None.
    pub password: Option<String>,

    /// Redis database number. Default: 0.
    pub db: u8,
}

impl RedisEndpointConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;

        if parts.scheme != "redis" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'redis', got '{}'",
                parts.scheme
            )));
        }

        // Parse host and port from path (format: //host:port or //host or empty)
        // Use Option to distinguish "not set in URI" from "set in URI"
        let (host, port) = if parts.path.starts_with("//") {
            let path = &parts.path[2..]; // Remove leading //
            if path.is_empty() {
                // redis://?command=GET → no host, no port
                (None, None)
            } else {
                let (host_part, port_part) = match path.split_once(':') {
                    Some((h, p)) => (h, Some(p)),
                    None => (path, None),
                };
                let host = Some(host_part.to_string());
                let port = port_part.and_then(|p| p.parse().ok());
                (host, port)
            }
        } else {
            // No // prefix means no host/port in URI
            (None, None)
        };

        // Parse command (default to SET)
        let command = parts
            .params
            .get("command")
            .map(|s| RedisCommand::from_str(s))
            .transpose()?
            .unwrap_or(RedisCommand::Set);

        // Parse channels (comma-separated)
        let channels = parts
            .params
            .get("channels")
            .map(|s| s.split(',').map(String::from).collect())
            .unwrap_or_default();

        // Parse key
        let key = parts.params.get("key").cloned();

        // Parse timeout (default to 1 second)
        let timeout = parts
            .params
            .get("timeout")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);

        // Parse password
        let password = parts.params.get("password").cloned();

        // Parse db (default to 0)
        let db = parts
            .params
            .get("db")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        Ok(Self {
            host,
            port,
            command,
            channels,
            key,
            timeout,
            password,
            db,
        })
    }

    /// Apply global defaults to any `None` fields.
    ///
    /// This method fills in default values from the provided `RedisConfig` for
    /// fields that are `None` (not set in URI). It's intended to be called after
    /// parsing a URI when global component defaults should be applied.
    pub fn apply_defaults(&mut self, defaults: &RedisConfig) {
        if self.host.is_none() {
            self.host = Some(defaults.host.clone());
        }
        if self.port.is_none() {
            self.port = Some(defaults.port);
        }
    }

    /// Resolve any remaining `None` fields to hardcoded defaults.
    ///
    /// This should be called after `apply_defaults()` to ensure all fields
    /// that can have global defaults are guaranteed to be `Some`.
    pub fn resolve_defaults(&mut self) {
        let defaults = RedisConfig::default();
        if self.host.is_none() {
            self.host = Some(defaults.host);
        }
        if self.port.is_none() {
            self.port = Some(defaults.port);
        }
    }

    /// Build the Redis connection URL.
    ///
    /// After `resolve_defaults()`, host and port are guaranteed `Some`.
    pub fn redis_url(&self) -> String {
        let host = self.host.as_deref().unwrap_or("localhost");
        let port = self.port.unwrap_or(6379);

        if let Some(password) = &self.password {
            format!("redis://:{}@{}:{}/{}", password, host, port, self.db)
        } else {
            format!("redis://{}:{}/{}", host, port, self.db)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let c = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        assert_eq!(c.host, Some("localhost".to_string()));
        assert_eq!(c.port, Some(6379));
        assert_eq!(c.command, RedisCommand::Set);
        assert!(c.channels.is_empty());
        assert!(c.key.is_none());
        assert_eq!(c.timeout, 1);
        assert!(c.password.is_none());
        assert_eq!(c.db, 0);
    }

    #[test]
    fn test_config_no_host_port() {
        // URI with no host/port in path
        let c = RedisEndpointConfig::from_uri("redis://?command=GET").unwrap();
        assert_eq!(c.host, None);
        assert_eq!(c.port, None);
        assert_eq!(c.command, RedisCommand::Get);
    }

    #[test]
    fn test_config_host_only() {
        // URI with host but no port
        let c = RedisEndpointConfig::from_uri("redis://redis-server?command=GET").unwrap();
        assert_eq!(c.host, Some("redis-server".to_string()));
        assert_eq!(c.port, None);
    }

    #[test]
    fn test_config_host_and_port() {
        let c = RedisEndpointConfig::from_uri("redis://localhost:6380?command=GET").unwrap();
        assert_eq!(c.host, Some("localhost".to_string()));
        assert_eq!(c.port, Some(6380));
        assert_eq!(c.command, RedisCommand::Get);
    }

    #[test]
    fn test_config_wrong_scheme() {
        assert!(RedisEndpointConfig::from_uri("http://localhost:6379").is_err());
    }

    #[test]
    fn test_config_command() {
        let c = RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET").unwrap();
        assert_eq!(c.command, RedisCommand::Get);
    }

    #[test]
    fn test_config_subscribe() {
        let c = RedisEndpointConfig::from_uri(
            "redis://localhost:6379?command=SUBSCRIBE&channels=foo,bar",
        )
        .unwrap();
        assert_eq!(c.command, RedisCommand::Subscribe);
        assert_eq!(c.channels, vec!["foo".to_string(), "bar".to_string()]);
    }

    #[test]
    fn test_config_blpop() {
        let c = RedisEndpointConfig::from_uri(
            "redis://localhost:6379?command=BLPOP&key=jobs&timeout=5",
        )
        .unwrap();
        assert_eq!(c.command, RedisCommand::Blpop);
        assert_eq!(c.key, Some("jobs".to_string()));
        assert_eq!(c.timeout, 5);
    }

    #[test]
    fn test_config_auth_db() {
        let c =
            RedisEndpointConfig::from_uri("redis://localhost:6379?password=secret&db=2").unwrap();
        assert_eq!(c.password, Some("secret".to_string()));
        assert_eq!(c.db, 2);
    }

    #[test]
    fn test_redis_url() {
        let mut c =
            RedisEndpointConfig::from_uri("redis://localhost:6379?password=secret&db=2").unwrap();
        c.resolve_defaults();
        assert_eq!(c.redis_url(), "redis://:secret@localhost:6379/2");
    }

    #[test]
    fn test_redis_url_no_auth() {
        let mut c = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        c.resolve_defaults();
        assert_eq!(c.redis_url(), "redis://localhost:6379/0");
    }

    #[test]
    fn test_command_from_str() {
        assert!(RedisCommand::from_str("SET").is_ok());
        assert_eq!(RedisCommand::from_str("SET").unwrap(), RedisCommand::Set);
        assert!(RedisCommand::from_str("get").is_ok());
        assert_eq!(RedisCommand::from_str("get").unwrap(), RedisCommand::Get);
        assert!(RedisCommand::from_str("UNKNOWN").is_err());
    }

    // --- RedisConfig tests ---

    #[test]
    fn test_redis_config_defaults() {
        let cfg = RedisConfig::default();
        assert_eq!(cfg.host, "localhost");
        assert_eq!(cfg.port, 6379);
    }

    #[test]
    fn test_redis_config_builder() {
        let cfg = RedisConfig::default()
            .with_host("redis-prod")
            .with_port(6380);
        assert_eq!(cfg.host, "redis-prod");
        assert_eq!(cfg.port, 6380);
    }

    // --- apply_defaults tests ---

    #[test]
    fn test_apply_defaults_fills_none_fields() {
        let mut config = RedisEndpointConfig::from_uri("redis://?command=GET").unwrap();
        assert_eq!(config.host, None);
        assert_eq!(config.port, None);

        let defaults = RedisConfig::default()
            .with_host("redis-server")
            .with_port(6380);
        config.apply_defaults(&defaults);

        assert_eq!(config.host, Some("redis-server".to_string()));
        assert_eq!(config.port, Some(6380));
    }

    #[test]
    fn test_apply_defaults_preserves_values() {
        let mut config = RedisEndpointConfig::from_uri("redis://custom:7000?command=GET").unwrap();
        assert_eq!(config.host, Some("custom".to_string()));
        assert_eq!(config.port, Some(7000));

        let defaults = RedisConfig::default()
            .with_host("should-not-override")
            .with_port(9999);
        config.apply_defaults(&defaults);

        // Existing values should be preserved
        assert_eq!(config.host, Some("custom".to_string()));
        assert_eq!(config.port, Some(7000));
    }

    #[test]
    fn test_apply_defaults_partial_none() {
        // Host set, port not set
        let mut config = RedisEndpointConfig::from_uri("redis://myhost?command=GET").unwrap();
        assert_eq!(config.host, Some("myhost".to_string()));
        assert_eq!(config.port, None);

        let defaults = RedisConfig::default()
            .with_host("default-host")
            .with_port(6380);
        config.apply_defaults(&defaults);

        // Host preserved, port filled
        assert_eq!(config.host, Some("myhost".to_string()));
        assert_eq!(config.port, Some(6380));
    }

    // --- resolve_defaults tests ---

    #[test]
    fn test_resolve_defaults_fills_remaining_nones() {
        let mut config = RedisEndpointConfig::from_uri("redis://?command=GET").unwrap();
        assert_eq!(config.host, None);
        assert_eq!(config.port, None);

        config.resolve_defaults();

        assert_eq!(config.host, Some("localhost".to_string()));
        assert_eq!(config.port, Some(6379));
    }

    #[test]
    fn test_resolve_defaults_preserves_existing() {
        let mut config = RedisEndpointConfig::from_uri("redis://custom:7000?command=GET").unwrap();
        config.resolve_defaults();

        // Already set values should be preserved
        assert_eq!(config.host, Some("custom".to_string()));
        assert_eq!(config.port, Some(7000));
    }

    // --- Critical test: catches the original defect where from_uri filled absent
    //     params with hardcoded defaults, preventing apply_defaults from working.
    #[test]
    fn test_apply_defaults_with_from_uri_no_host_param() {
        // Parse URI without host/port
        let mut ep = RedisEndpointConfig::from_uri("redis://?command=GET").unwrap();
        // At this point, ep.host and ep.port should be None
        assert!(ep.host.is_none(), "host should be None when not in URI");
        assert!(ep.port.is_none(), "port should be None when not in URI");

        let defaults = RedisConfig::default().with_host("redis-prod");
        ep.apply_defaults(&defaults);
        // Custom default should be applied
        assert_eq!(
            ep.host,
            Some("redis-prod".to_string()),
            "custom default host should be applied"
        );
    }
}
