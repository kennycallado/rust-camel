use camel_component_api::CamelError;
use camel_component_api::parse_uri;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
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

// --- RedisConfig (global defaults) ---

/// Global Redis configuration defaults.
///
/// This struct holds component-level defaults that can be set via YAML config
/// and applied to endpoint configurations when specific values aren't provided.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RedisConfig {
    pub host: String,
    pub port: u16,
    /// Redis password for authentication. Default: None.
    pub password: Option<String>,
    /// Enable TLS connections. Default: false.
    pub tls: bool,
    /// Optional path to a CA certificate file for TLS verification. Default: None.
    pub tls_ca_cert: Option<String>,
    /// Cluster node URLs. Empty means single-node mode. Feature-gated behind `cluster`.
    #[cfg(feature = "cluster")]
    pub cluster_nodes: Vec<String>,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 6379,
            password: None,
            tls: false,
            tls_ca_cert: None,
            #[cfg(feature = "cluster")]
            cluster_nodes: Vec::new(),
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

    pub fn with_password(mut self, v: impl Into<String>) -> Self {
        self.password = Some(v.into());
        self
    }

    pub fn with_tls(mut self, v: bool) -> Self {
        self.tls = v;
        self
    }

    pub fn with_tls_ca_cert(mut self, v: impl Into<String>) -> Self {
        self.tls_ca_cert = Some(v.into());
        self
    }

    /// Set cluster node URLs. Feature-gated behind `cluster`.
    #[cfg(feature = "cluster")]
    pub fn with_cluster_nodes(mut self, nodes: Vec<String>) -> Self {
        self.cluster_nodes = nodes;
        self
    }

    /// Build the Redis connection URL from this config.
    ///
    /// Passwords are percent-encoded to handle special characters (`@`, `:`, `/`).
    /// Uses `rediss://` scheme when `tls` is true.
    pub fn build_url(&self) -> Result<String, CamelError> {
        // Validate TLS feature availability
        self.validate_tls()?;

        let scheme = if self.tls { "rediss" } else { "redis" };
        let auth = if let Some(password) = &self.password {
            let encoded = utf8_percent_encode(password, NON_ALPHANUMERIC).to_string();
            format!(":{}@", encoded)
        } else {
            String::new()
        };
        Ok(format!(
            "{}://{}{}:{}/0",
            scheme, auth, self.host, self.port
        ))
    }

    /// Validate TLS configuration.
    ///
    /// Returns an error when `tls` is true but the `redis` crate was not compiled
    /// with TLS support (requires one of: `tls-rustls-webpki-roots`,
    /// `tls-rustls-native-certs`, or `tls-native-tls` feature).
    pub fn validate_tls(&self) -> Result<(), CamelError> {
        if self.tls && !cfg!(feature = "tls") {
            return Err(CamelError::Config(
                "TLS requires redis/tls feature (enable one of: tls-rustls-webpki-roots, tls-rustls-native-certs, tls-native-tls)".into(),
            ));
        }
        Ok(())
    }
}

// --- ClusterConfig (REDIS-012) ---

/// Configuration for Redis Cluster mode.
///
/// Feature-gated behind the `cluster` feature flag.
/// When `nodes` is non-empty, the component should connect to a Redis Cluster
/// instead of a single node.
///
/// TODO(REDIS-012): cluster connection pooling not yet implemented
#[cfg(feature = "cluster")]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClusterConfig {
    /// Cluster node URLs (e.g. `["redis://node1:6379", "redis://node2:6379"]`).
    pub nodes: Vec<String>,
    /// Whether to route read queries to replica nodes. Default: false.
    pub readonly_replicas: bool,
}

#[cfg(feature = "cluster")]
impl ClusterConfig {
    /// Create a new ClusterConfig with the given node URLs.
    pub fn new(nodes: Vec<String>) -> Self {
        Self {
            nodes,
            readonly_replicas: false,
        }
    }

    /// Enable routing read queries to replica nodes.
    pub fn with_readonly_replicas(mut self) -> Self {
        self.readonly_replicas = true;
        self
    }

    /// Returns true if cluster mode is enabled (nodes is non-empty).
    pub fn is_cluster_mode(&self) -> bool {
        !self.nodes.is_empty()
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
/// - `ssl` - Use TLS connection. `None` means not set (falls back to global config or false).
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

    /// Use TLS connection. `None` means not explicitly set in URI.
    /// Filled by `apply_defaults()` from global config TLS setting.
    /// After resolution, use `is_ssl_enabled()` for the effective value.
    pub ssl: Option<bool>,
}

impl RedisEndpointConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;

        // Support both `redis://` and `rediss://` (TLS) schemes
        if parts.scheme != "redis" && parts.scheme != "rediss" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'redis' or 'rediss', got '{}'",
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
                let port = port_part
                    .map(|p| {
                        let n = p.parse::<u16>().map_err(|_| {
                            CamelError::InvalidUri(format!(
                                "invalid port '{}': expected 1-65535",
                                p
                            ))
                        })?;
                        if n == 0 {
                            return Err(CamelError::InvalidUri(format!(
                                "invalid port '{}': expected 1-65535",
                                p
                            )));
                        }
                        Ok(n)
                    })
                    .transpose()?;
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

        // Parse timeout (default to 1 second if absent, error if present but invalid)
        let timeout = match parts.params.get("timeout") {
            Some(s) => s.parse::<u64>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "invalid timeout '{}': expected non-negative integer",
                    s
                ))
            })?,
            None => 1,
        };

        // Parse password
        let password = parts.params.get("password").cloned();

        // Parse db (default to 0 if absent, error if present but invalid)
        let db = match parts.params.get("db") {
            Some(s) => s.parse::<u8>().map_err(|_| {
                CamelError::InvalidUri(format!("invalid db '{}': expected integer 0-255", s))
            })?,
            None => 0,
        };

        // Parse ssl: `rediss://` scheme → Some(true), explicit `ssl=` param → Some(bool), absent → None
        // If ssl param is present but not a valid boolean, error instead of silently defaulting
        let ssl = if parts.scheme == "rediss" {
            Some(true)
        } else {
            parts
                .params
                .get("ssl")
                .map(|s| match s.to_lowercase().as_str() {
                    "true" => Ok(true),
                    "false" => Ok(false),
                    _ => Err(CamelError::InvalidUri(format!(
                        "invalid ssl '{}': expected 'true' or 'false'",
                        s
                    ))),
                })
                .transpose()?
        };

        Ok(Self {
            host,
            port,
            command,
            channels,
            key,
            timeout,
            password,
            db,
            ssl,
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
        // TLS from global config applies only when ssl was not explicitly set in URI
        if self.ssl.is_none() && defaults.tls {
            self.ssl = Some(true);
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
        // Default ssl to false if not set
        if self.ssl.is_none() {
            self.ssl = Some(false);
        }
    }

    /// Returns the effective SSL setting after defaults have been applied.
    ///
    /// Panics if `resolve_defaults()` has not been called yet (ssl is `None`).
    pub fn is_ssl_enabled(&self) -> bool {
        self.ssl.unwrap_or(false)
    }

    /// Build the Redis connection URL.
    ///
    /// After `resolve_defaults()`, host and port are guaranteed `Some`.
    /// Passwords are URL-encoded to handle special characters (`@`, `:`, `/`).
    /// Uses `rediss://` scheme when `ssl` is true.
    pub fn redis_url(&self) -> String {
        let host = self.host.as_deref().unwrap_or("localhost");
        let port = self.port.unwrap_or(6379);
        let scheme = if self.is_ssl_enabled() {
            "rediss"
        } else {
            "redis"
        };

        if let Some(password) = &self.password {
            let encoded = utf8_percent_encode(password, NON_ALPHANUMERIC).to_string();
            format!("{}://:{}@{}:{}/{}", scheme, encoded, host, port, self.db) // allow-secret
        } else {
            format!("{}://{}:{}/{}", scheme, host, port, self.db) // allow-secret
        }
    }

    /// Build a safe version of the Redis URL with password redacted.
    ///
    /// Returns the full URL from `redis_url()` if no password is set,
    /// otherwise replaces the password with `***`.
    pub fn redis_url_safe(&self) -> String {
        let host = self.host.as_deref().unwrap_or("localhost");
        let port = self.port.unwrap_or(6379);
        let scheme = if self.is_ssl_enabled() {
            "rediss"
        } else {
            "redis"
        };

        match &self.password {
            Some(_) => format!("{}://:***@{}:{}/{}", scheme, host, port, self.db), // allow-secret
            None => self.redis_url(),
        }
    }

    /// Return a concise, secret-safe endpoint identifier for tracing.
    ///
    /// Format: `rediss://host:port/db` or `redis://host:port/db` — never contains credentials.
    pub fn safe_endpoint(&self) -> String {
        let host = self.host.as_deref().unwrap_or("localhost");
        let port = self.port.unwrap_or(6379);
        let scheme = if self.is_ssl_enabled() {
            "rediss"
        } else {
            "redis"
        };
        format!("{}://{}:{}/{}", scheme, host, port, self.db)
    }
}

// ── Transient error detection ────────────────────────────────────────────────

/// Returns true if the error is a transient transport failure that may be
/// resolved by reconnecting (e.g. connection reset, timeout, I/O error).
///
/// Business errors (WRONGTYPE, NOSCRIPT, etc.) and config errors are NOT transient.
pub fn is_transient_redis_error(err: &CamelError) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("connection")
        || msg.contains("io error")
        || msg.contains("timed out")
        || msg.contains("broken pipe")
        || msg.contains("connection reset")
        || msg.contains("eof")
        || msg.contains("refused")
}

// ── Capped exponential backoff ──────────────────────────────────────────────

/// Computes the backoff delay for a given attempt number using capped exponential
/// backoff: `min(2^attempt * 100ms, max_delay)`.
///
/// - `attempt`: zero-based retry attempt number
/// - `base_ms`: base delay in milliseconds (default 100)
/// - `max_delay`: maximum delay cap
pub fn backoff_delay(
    attempt: u32,
    base_ms: u64,
    max_delay: std::time::Duration,
) -> std::time::Duration {
    let exponent = attempt.min(10);
    let delay_ms = base_ms.saturating_mul(1u64 << exponent);
    std::time::Duration::from_millis(delay_ms).min(max_delay)
}

// ── Command idempotency classification ──────────────────────────────────────

/// Returns true if the command is idempotent (safe to retry without risk of
/// duplicate side effects). Read commands and some writes (SET, DEL) are idempotent.
pub fn is_idempotent_command(cmd: &RedisCommand) -> bool {
    matches!(
        cmd,
        // Read-only commands
        RedisCommand::Get
            | RedisCommand::Mget
            | RedisCommand::Exists
            | RedisCommand::Ttl
            | RedisCommand::Keys
            | RedisCommand::Type
            | RedisCommand::Llen
            | RedisCommand::Lrange
            | RedisCommand::Lindex
            | RedisCommand::Scard
            | RedisCommand::Smembers
            | RedisCommand::Sismember
            | RedisCommand::Srandmember
            | RedisCommand::Zrange
            | RedisCommand::Zrevrange
            | RedisCommand::Zrank
            | RedisCommand::Zrevrank
            | RedisCommand::Zscore
            | RedisCommand::Zcard
            | RedisCommand::Zcount
            | RedisCommand::Zrangebyscore
            | RedisCommand::Zrevrangebyscore
            | RedisCommand::Hget
            | RedisCommand::Hmget
            | RedisCommand::Hexists
            | RedisCommand::Hlen
            | RedisCommand::Hkeys
            | RedisCommand::Hvals
            | RedisCommand::Hgetall
            | RedisCommand::Strlen
            | RedisCommand::Ping
            | RedisCommand::Echo
            // Idempotent writes (same result if retried)
            | RedisCommand::Set
            | RedisCommand::Del
            | RedisCommand::Expire
            | RedisCommand::Expireat
            | RedisCommand::Pexpire
            | RedisCommand::Pexpireat
            | RedisCommand::Persist
            | RedisCommand::Rename
            | RedisCommand::Renamenx
            | RedisCommand::Move
    )
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
        // Password without special chars is unchanged
        assert_eq!(c.redis_url(), "redis://:secret@localhost:6379/2");
    }

    #[test]
    fn test_redis_url_no_auth() {
        let mut c = RedisEndpointConfig::from_uri("redis://localhost:6379").unwrap();
        c.resolve_defaults();
        assert_eq!(c.redis_url(), "redis://localhost:6379/0");
    }

    // REDIS-015: Password with special characters is URL-encoded
    #[test]
    fn test_redis_url_encodes_password_with_at_sign() {
        // Use a config with raw password containing @
        let c = RedisEndpointConfig {
            host: Some("localhost".to_string()),
            port: Some(6379),
            command: RedisCommand::Set,
            channels: vec![],
            key: None,
            timeout: 1,
            password: Some("pass@word".to_string()),
            db: 0,
            ssl: Some(false),
        };
        let url = c.redis_url();
        assert!(
            url.contains("%40"),
            "password with @ should be encoded: {}",
            url
        );
        assert!(
            !url.contains("pass@word"),
            "raw @ should not appear in URL: {}",
            url
        );
    }

    #[test]
    fn test_redis_url_encodes_password_with_colon() {
        let c = RedisEndpointConfig {
            host: Some("localhost".to_string()),
            port: Some(6379),
            command: RedisCommand::Set,
            channels: vec![],
            key: None,
            timeout: 1,
            password: Some("pass:word".to_string()),
            db: 0,
            ssl: Some(false),
        };
        let url = c.redis_url();
        assert!(
            url.contains("%3A"),
            "password with : should be encoded: {}",
            url
        );
    }

    #[test]
    fn test_redis_url_encodes_password_with_slash() {
        let c = RedisEndpointConfig {
            host: Some("localhost".to_string()),
            port: Some(6379),
            command: RedisCommand::Set,
            channels: vec![],
            key: None,
            timeout: 1,
            password: Some("pass/word".to_string()),
            db: 0,
            ssl: Some(false),
        };
        let url = c.redis_url();
        assert!(
            url.contains("%2F"),
            "password with / should be encoded: {}",
            url
        );
    }

    // REDIS-006: TLS support via rediss:// scheme and ssl=true param
    #[test]
    fn test_rediss_scheme_enables_ssl() {
        let c = RedisEndpointConfig::from_uri("rediss://localhost:6379?command=GET").unwrap();
        assert_eq!(c.ssl, Some(true), "rediss:// scheme should enable SSL");
    }

    #[test]
    fn test_ssl_true_param_enables_ssl() {
        let c =
            RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET&ssl=true").unwrap();
        assert_eq!(c.ssl, Some(true), "ssl=true should enable SSL");
    }

    #[test]
    fn test_ssl_false_param_does_not_enable_ssl() {
        let c =
            RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET&ssl=false").unwrap();
        assert_eq!(c.ssl, Some(false), "ssl=false should not enable SSL");
    }

    #[test]
    fn test_redis_url_uses_rediss_scheme_when_ssl_enabled() {
        let c = RedisEndpointConfig {
            host: Some("localhost".to_string()),
            port: Some(6379),
            command: RedisCommand::Set,
            channels: vec![],
            key: None,
            timeout: 1,
            password: None,
            db: 0,
            ssl: Some(true),
        };
        let url = c.redis_url();
        assert!(
            url.starts_with("rediss://"),
            "SSL config should use rediss:// scheme: {}",
            url
        );
    }

    #[test]
    fn test_redis_url_uses_redis_scheme_when_ssl_disabled() {
        let c = RedisEndpointConfig {
            host: Some("localhost".to_string()),
            port: Some(6379),
            command: RedisCommand::Set,
            channels: vec![],
            key: None,
            timeout: 1,
            password: None,
            db: 0,
            ssl: Some(false),
        };
        let url = c.redis_url();
        assert!(
            url.starts_with("redis://"),
            "non-SSL config should use redis:// scheme: {}",
            url
        );
    }

    #[test]
    fn test_redis_url_safe_uses_correct_scheme_for_ssl() {
        let c_ssl = RedisEndpointConfig {
            host: Some("localhost".to_string()),
            port: Some(6379),
            command: RedisCommand::Set,
            channels: vec![],
            key: None,
            timeout: 1,
            password: Some("secret".to_string()),
            db: 0,
            ssl: Some(true),
        };
        let safe = c_ssl.redis_url_safe();
        assert!(
            safe.starts_with("rediss://"),
            "safe URL should use rediss:// for SSL: {}",
            safe
        );
        assert!(
            !safe.contains("secret"),
            "safe URL must not contain password"
        );

        let c_no_ssl = RedisEndpointConfig {
            host: Some("localhost".to_string()),
            port: Some(6379),
            command: RedisCommand::Set,
            channels: vec![],
            key: None,
            timeout: 1,
            password: Some("secret".to_string()),
            db: 0,
            ssl: Some(false),
        };
        let safe = c_no_ssl.redis_url_safe();
        assert!(
            safe.starts_with("redis://"),
            "safe URL should use redis:// for non-SSL: {}",
            safe
        );
    }

    // P3/P4: safe_endpoint helper
    #[test]
    fn test_safe_endpoint_no_credentials() {
        let c = RedisEndpointConfig {
            host: Some("localhost".to_string()),
            port: Some(6379),
            command: RedisCommand::Set,
            channels: vec![],
            key: None,
            timeout: 1,
            password: Some("supersecret".to_string()),
            db: 2,
            ssl: Some(false),
        };
        let endpoint = c.safe_endpoint();
        assert!(
            !endpoint.contains("supersecret"),
            "safe endpoint must not contain password"
        );
        assert!(
            endpoint.contains("localhost"),
            "safe endpoint should contain host"
        );
        assert!(
            endpoint.contains("6379"),
            "safe endpoint should contain port"
        );
    }

    #[test]
    fn test_safe_endpoint_uses_correct_scheme() {
        let c_ssl = RedisEndpointConfig {
            host: Some("localhost".to_string()),
            port: Some(6380),
            command: RedisCommand::Set,
            channels: vec![],
            key: None,
            timeout: 1,
            password: None,
            db: 0,
            ssl: Some(true),
        };
        assert!(c_ssl.safe_endpoint().starts_with("rediss://"));

        let c_plain = RedisEndpointConfig {
            host: Some("localhost".to_string()),
            port: Some(6380),
            command: RedisCommand::Set,
            channels: vec![],
            key: None,
            timeout: 1,
            password: None,
            db: 0,
            ssl: Some(false),
        };
        assert!(c_plain.safe_endpoint().starts_with("redis://"));
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
        assert!(cfg.password.is_none());
        assert!(!cfg.tls);
        assert!(cfg.tls_ca_cert.is_none());
    }

    #[test]
    fn test_redis_config_builder() {
        let cfg = RedisConfig::default()
            .with_host("redis-prod")
            .with_port(6380)
            .with_password("secret")
            .with_tls(true)
            .with_tls_ca_cert("/path/to/ca.pem");
        assert_eq!(cfg.host, "redis-prod");
        assert_eq!(cfg.port, 6380);
        assert_eq!(cfg.password, Some("secret".to_string()));
        assert!(cfg.tls);
        assert_eq!(cfg.tls_ca_cert, Some("/path/to/ca.pem".to_string()));
    }

    // REDIS-015: Password with special characters is percent-encoded in build_url()
    #[test]
    fn test_redis_url_with_special_char_password() {
        // password "p@ss!" should be percent-encoded in URL
        let config = RedisConfig {
            host: "localhost".into(),
            port: 6379,
            password: Some("p@ss!".into()),
            ..RedisConfig::default()
        };
        let url = config.build_url().unwrap();
        // @ is percent-encoded as %40
        assert!(
            url.contains("%40"),
            "password with @ should be encoded: {}",
            url
        );
    }

    #[test]
    fn test_redis_build_url_encodes_colon_in_password() {
        let config = RedisConfig {
            host: "localhost".into(),
            port: 6379,
            password: Some("pass:word".into()),
            ..RedisConfig::default()
        };
        let url = config.build_url().unwrap();
        assert!(
            url.contains("%3A"),
            "password with : should be encoded: {}",
            url
        );
    }

    #[test]
    fn test_redis_build_url_no_password() {
        let config = RedisConfig {
            host: "localhost".into(),
            port: 6380,
            password: None,
            ..RedisConfig::default()
        };
        let url = config.build_url().unwrap();
        assert_eq!(url, "redis://localhost:6380/0");
    }

    #[test]
    fn test_redis_build_url_uses_rediss_when_tls_enabled() {
        let config = RedisConfig {
            host: "localhost".into(),
            port: 6379,
            tls: true,
            ..RedisConfig::default()
        };
        // Without the tls feature, build_url returns an error
        let result = config.build_url();
        // Since tls feature is not enabled in Cargo.toml, expect error
        assert!(
            result.is_err(),
            "build_url should error when tls=true but feature not enabled"
        );
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("TLS requires"),
            "error should mention TLS requirement: {}",
            err
        );
    }

    // REDIS-006: TLS validation
    #[test]
    fn test_tls_without_feature_returns_error() {
        // When tls=true but redis/tls feature not available, expect Err
        let config = RedisConfig {
            tls: true,
            ..RedisConfig::default()
        };
        let result = config.validate_tls();
        // Without the tls feature flag, this should return an error
        assert!(
            result.is_err(),
            "validate_tls should error when tls=true but feature not enabled"
        );
        let err = result.unwrap_err();
        assert!(err.to_string().contains("TLS requires redis/tls feature"));
    }

    #[test]
    fn test_validate_tls_ok_when_disabled() {
        let config = RedisConfig {
            tls: false,
            ..RedisConfig::default()
        };
        assert!(config.validate_tls().is_ok());
    }

    #[test]
    fn test_apply_defaults_propagates_tls() {
        let mut config = RedisEndpointConfig::from_uri("redis://?command=GET").unwrap();
        assert_eq!(config.ssl, None);

        let defaults = RedisConfig::default().with_tls(true);
        config.apply_defaults(&defaults);

        assert_eq!(
            config.ssl,
            Some(true),
            "TLS from global defaults should enable ssl on endpoint"
        );
    }

    #[test]
    fn test_apply_defaults_preserves_explicit_ssl_false() {
        // When ssl is explicitly set via URI (ssl=false), global tls default should NOT override
        let mut config =
            RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET&ssl=false").unwrap();
        assert_eq!(config.ssl, Some(false));

        let defaults = RedisConfig::default().with_tls(true);
        config.apply_defaults(&defaults);

        // ssl=false was explicit in URI, should be preserved
        assert_eq!(
            config.ssl,
            Some(false),
            "explicit ssl=false should not be overridden by global tls=true"
        );
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
    fn test_redis_url_safe_redacts_password() {
        let config =
            RedisEndpointConfig::from_uri("redis://localhost:6379?password=mysecret&db=2").unwrap();
        let safe = config.redis_url_safe();
        assert!(
            !safe.contains("mysecret"),
            "safe URL must not contain password"
        );
        assert!(
            safe.contains("***"),
            "safe URL should contain redaction marker"
        );
        assert!(safe.contains("localhost"), "safe URL should contain host");
        assert!(safe.contains("6379"), "safe URL should contain port");
    }

    #[test]
    fn test_redis_url_safe_no_password() {
        let config = RedisEndpointConfig::from_uri("redis://localhost:6379?db=0").unwrap();
        let safe = config.redis_url_safe();
        assert_eq!(safe, config.redis_url());
    }

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

    // REDIS-002: Transient error detection
    #[test]
    fn test_is_transient_redis_error_detects_connection_errors() {
        assert!(is_transient_redis_error(&CamelError::ProcessorError(
            "Connection refused".into()
        )));
        assert!(is_transient_redis_error(&CamelError::ProcessorError(
            "connection reset by peer".into()
        )));
        assert!(is_transient_redis_error(&CamelError::ProcessorError(
            "IO error: broken pipe".into()
        )));
        assert!(is_transient_redis_error(&CamelError::ProcessorError(
            "timed out".into()
        )));
        assert!(is_transient_redis_error(&CamelError::ProcessorError(
            "EOF".into()
        )));
    }

    #[test]
    fn test_is_transient_redis_error_rejects_business_errors() {
        assert!(!is_transient_redis_error(&CamelError::ProcessorError(
            "WRONGTYPE".into()
        )));
        assert!(!is_transient_redis_error(&CamelError::ProcessorError(
            "NOSCRIPT".into()
        )));
        assert!(!is_transient_redis_error(&CamelError::InvalidUri(
            "bad uri".into()
        )));
        assert!(!is_transient_redis_error(&CamelError::Config(
            "bad config".into()
        )));
    }

    // REDIS-002: Capped exponential backoff
    #[test]
    fn test_backoff_delay_grows_exponentially() {
        let max = std::time::Duration::from_secs(5);
        let d0 = backoff_delay(0, 100, max);
        let d1 = backoff_delay(1, 100, max);
        let d2 = backoff_delay(2, 100, max);
        assert_eq!(d0, std::time::Duration::from_millis(100));
        assert_eq!(d1, std::time::Duration::from_millis(200));
        assert_eq!(d2, std::time::Duration::from_millis(400));
    }

    #[test]
    fn test_backoff_delay_is_capped() {
        let max = std::time::Duration::from_secs(2);
        let d10 = backoff_delay(10, 100, max);
        assert_eq!(d10, max);
    }

    // REDIS-002: Idempotency classification
    #[test]
    fn test_is_idempotent_command_read_commands() {
        assert!(is_idempotent_command(&RedisCommand::Get));
        assert!(is_idempotent_command(&RedisCommand::Exists));
        assert!(is_idempotent_command(&RedisCommand::Ttl));
        assert!(is_idempotent_command(&RedisCommand::Ping));
    }

    #[test]
    fn test_is_idempotent_command_idempotent_writes() {
        assert!(is_idempotent_command(&RedisCommand::Set));
        assert!(is_idempotent_command(&RedisCommand::Del));
        assert!(is_idempotent_command(&RedisCommand::Expire));
    }

    #[test]
    fn test_is_idempotent_command_non_idempotent() {
        assert!(!is_idempotent_command(&RedisCommand::Incr));
        assert!(!is_idempotent_command(&RedisCommand::Decr));
        assert!(!is_idempotent_command(&RedisCommand::Lpush));
        assert!(!is_idempotent_command(&RedisCommand::Sadd));
        assert!(!is_idempotent_command(&RedisCommand::Zadd));
        assert!(!is_idempotent_command(&RedisCommand::Publish));
    }

    // --- Strict validation tests ---

    #[test]
    fn test_invalid_port_returns_error() {
        let result = RedisEndpointConfig::from_uri("redis://localhost:abc?command=GET");
        assert!(result.is_err(), "non-numeric port should error");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid port"),
            "error should mention invalid port: {}",
            msg
        );
    }

    #[test]
    fn test_out_of_range_port_returns_error() {
        // u16 max is 65535, but a value like 99999 won't fit
        let result = RedisEndpointConfig::from_uri("redis://localhost:99999?command=GET");
        assert!(result.is_err(), "out-of-range port should error");
    }

    #[test]
    fn test_invalid_timeout_returns_error() {
        let result =
            RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET&timeout=abc");
        assert!(result.is_err(), "non-numeric timeout should error");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid timeout"),
            "error should mention invalid timeout: {}",
            msg
        );
    }

    #[test]
    fn test_negative_timeout_returns_error() {
        // A negative number won't parse as u64
        let result = RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET&timeout=-1");
        assert!(result.is_err(), "negative timeout should error");
    }

    #[test]
    fn test_invalid_db_returns_error() {
        let result = RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET&db=abc");
        assert!(result.is_err(), "non-numeric db should error");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid db"),
            "error should mention invalid db: {}",
            msg
        );
    }

    #[test]
    fn test_out_of_range_db_returns_error() {
        // db is u8 (0-255), 300 won't fit
        let result = RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET&db=300");
        assert!(result.is_err(), "out-of-range db should error");
    }

    #[test]
    fn test_invalid_ssl_returns_error() {
        let result = RedisEndpointConfig::from_uri("redis://localhost:6379?command=GET&ssl=yes");
        assert!(result.is_err(), "non-boolean ssl should error");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid ssl"),
            "error should mention invalid ssl: {}",
            msg
        );
    }

    #[test]
    fn test_valid_params_still_work() {
        // Sanity check that valid params still parse correctly after strict validation
        let c = RedisEndpointConfig::from_uri(
            "redis://localhost:6380?command=GET&timeout=10&db=3&ssl=true",
        )
        .unwrap();
        assert_eq!(c.port, Some(6380));
        assert_eq!(c.timeout, 10);
        assert_eq!(c.db, 3);
        assert_eq!(c.ssl, Some(true));
    }

    #[test]
    fn test_absent_params_use_defaults() {
        // When params are absent, defaults should be used (not error)
        let c = RedisEndpointConfig::from_uri("redis://localhost?command=GET").unwrap();
        assert_eq!(c.timeout, 1);
        assert_eq!(c.db, 0);
        assert_eq!(c.ssl, None);
    }

    // --- REDIS-012: Cluster config tests ---
    #[cfg(feature = "cluster")]
    mod cluster_tests {
        use super::*;

        #[test]
        fn test_cluster_config_default_is_empty() {
            let cfg = ClusterConfig::default();
            assert!(cfg.nodes.is_empty());
            assert!(!cfg.readonly_replicas);
            assert!(!cfg.is_cluster_mode());
        }

        #[test]
        fn test_cluster_config_new_with_nodes() {
            let cfg = ClusterConfig::new(vec![
                "redis://node1:6379".to_string(),
                "redis://node2:6379".to_string(),
            ]);
            assert_eq!(cfg.nodes.len(), 2);
            assert!(cfg.is_cluster_mode());
        }

        #[test]
        fn test_cluster_config_with_readonly_replicas() {
            let cfg =
                ClusterConfig::new(vec!["redis://node1:6379".to_string()]).with_readonly_replicas();
            assert!(cfg.readonly_replicas);
        }
    }
}
