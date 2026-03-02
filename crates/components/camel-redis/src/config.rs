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

#[derive(Debug, Clone)]
pub struct RedisConfig {
    pub host: String,
    pub port: u16,
    pub command: RedisCommand,
    pub channels: Vec<String>,
    pub key: Option<String>,
    pub timeout: u64,
    pub password: Option<String>,
    pub db: u8,
}

impl RedisConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;

        if parts.scheme != "redis" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'redis', got '{}'",
                parts.scheme
            )));
        }

        // Parse host and port from path (format: //host:port or //host)
        let (host, port) = if parts.path.starts_with("//") {
            let path = &parts.path[2..]; // Remove leading //
            let (host_part, port_part) = match path.split_once(':') {
                Some((h, p)) => (h, p),
                None => (path, "6379"),
            };
            let port = port_part.parse().unwrap_or(6379);
            (host_part.to_string(), port)
        } else {
            ("localhost".to_string(), 6379)
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

    pub fn redis_url(&self) -> String {
        if let Some(password) = &self.password {
            format!(
                "redis://:{}@{}:{}/{}",
                password, self.host, self.port, self.db
            )
        } else {
            format!("redis://{}:{}/{}", self.host, self.port, self.db)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let c = RedisConfig::from_uri("redis://localhost:6379").unwrap();
        assert_eq!(c.host, "localhost");
        assert_eq!(c.port, 6379);
        assert_eq!(c.command, RedisCommand::Set);
        assert!(c.channels.is_empty());
        assert!(c.key.is_none());
        assert_eq!(c.timeout, 1);
        assert!(c.password.is_none());
        assert_eq!(c.db, 0);
    }

    #[test]
    fn test_config_wrong_scheme() {
        assert!(RedisConfig::from_uri("http://localhost:6379").is_err());
    }

    #[test]
    fn test_config_command() {
        let c = RedisConfig::from_uri("redis://localhost:6379?command=GET").unwrap();
        assert_eq!(c.command, RedisCommand::Get);
    }

    #[test]
    fn test_config_subscribe() {
        let c = RedisConfig::from_uri("redis://localhost:6379?command=SUBSCRIBE&channels=foo,bar")
            .unwrap();
        assert_eq!(c.command, RedisCommand::Subscribe);
        assert_eq!(c.channels, vec!["foo".to_string(), "bar".to_string()]);
    }

    #[test]
    fn test_config_blpop() {
        let c = RedisConfig::from_uri("redis://localhost:6379?command=BLPOP&key=jobs&timeout=5")
            .unwrap();
        assert_eq!(c.command, RedisCommand::Blpop);
        assert_eq!(c.key, Some("jobs".to_string()));
        assert_eq!(c.timeout, 5);
    }

    #[test]
    fn test_config_auth_db() {
        let c = RedisConfig::from_uri("redis://localhost:6379?password=secret&db=2").unwrap();
        assert_eq!(c.password, Some("secret".to_string()));
        assert_eq!(c.db, 2);
    }

    #[test]
    fn test_redis_url() {
        let c = RedisConfig::from_uri("redis://localhost:6379?password=secret&db=2").unwrap();
        assert_eq!(c.redis_url(), "redis://:secret@localhost:6379/2");
    }

    #[test]
    fn test_redis_url_no_auth() {
        let c = RedisConfig::from_uri("redis://localhost:6379").unwrap();
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
}
