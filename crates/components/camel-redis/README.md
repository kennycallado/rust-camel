# camel-component-redis

> Redis component for rust-camel integration framework

## Overview

The Redis component provides comprehensive Redis integration for rust-camel, supporting **85+ Redis commands** across all major data structures. It enables both producer (command execution) and consumer (pub/sub, list blocking) patterns.

## Features

- **85+ Redis Commands**: Strings, Lists, Hashes, Sets, Sorted Sets, Pub/Sub, Keys
- **Producer Mode**: Execute Redis commands and receive responses
- **Consumer Mode**: Subscribe to channels, blocking list pops
- **Connection Options**: Host, port, password, database selection
- **Async Native**: Built on `tokio` and async Redis client

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-redis = "0.2"
```

## URI Format

```
redis://host:port[?options]
```

## URI Options

| Option | Default | Description |
|--------|---------|-------------|
| `command` | `SET` | Redis command to execute |
| `channels` | - | Pub/Sub channels (comma-separated) |
| `key` | - | Redis key for operations |
| `timeout` | `1` | Timeout for blocking operations (seconds) |
| `password` | - | Redis password |
| `db` | `0` | Redis database number |

## Supported Commands

### String Operations (13 commands)
`SET`, `GET`, `GETSET`, `SETNX`, `SETEX`, `MGET`, `MSET`, `INCR`, `INCRBY`, `DECR`, `DECRBY`, `APPEND`, `STRLEN`

### Key Operations (14 commands)
`EXISTS`, `DEL`, `EXPIRE`, `EXPIREAT`, `PEXPIRE`, `PEXPIREAT`, `TTL`, `KEYS`, `RENAME`, `RENAMENX`, `TYPE`, `PERSIST`, `MOVE`, `SORT`

### List Operations (17 commands)
`LPUSH`, `RPUSH`, `LPUSHX`, `RPUSHX`, `LPOP`, `RPOP`, `BLPOP`, `BRPOP`, `LLEN`, `LRANGE`, `LINDEX`, `LINSERT`, `LSET`, `LREM`, `LTRIM`, `RPOPLPUSH`

### Hash Operations (11 commands)
`HSET`, `HGET`, `HSETNX`, `HMSET`, `HMGET`, `HDEL`, `HEXISTS`, `HLEN`, `HKEYS`, `HVALS`, `HGETALL`, `HINCRBY`

### Set Operations (14 commands)
`SADD`, `SREM`, `SMEMBERS`, `SCARD`, `SISMEMBER`, `SPOP`, `SMOVE`, `SINTER`, `SUNION`, `SDIFF`, `SINTERSTORE`, `SUNIONSTORE`, `SDIFFSTORE`, `SRANDMEMBER`

### Sorted Set Operations (17 commands)
`ZADD`, `ZREM`, `ZRANGE`, `ZREVRANGE`, `ZRANK`, `ZREVRANK`, `ZSCORE`, `ZCARD`, `ZINCRBY`, `ZCOUNT`, `ZRANGEBYSCORE`, `ZREVRANGEBYSCORE`, `ZREMRANGEBYRANK`, `ZREMRANGEBYSCORE`, `ZUNIONSTORE`, `ZINTERSTORE`

### Pub/Sub Operations (3 commands)
`PUBLISH`, `SUBSCRIBE`, `PSUBSCRIBE`

### Other Operations (2 commands)
`PING`, `ECHO`

## Usage

### String Operations

```rust
use camel_builder::RouteBuilder;
use camel_component_redis::RedisComponent;

let mut ctx = CamelContext::new();
ctx.register_component("redis", Box::new(RedisComponent::new()));

// SET: Store a value
let set_route = RouteBuilder::from("direct:set")
    .set_header("CamelRedisKey", Value::String("mykey".into()))
    .to("redis://localhost:6379?command=SET")
    .build()?;

// GET: Retrieve a value
let get_route = RouteBuilder::from("direct:get")
    .set_header("CamelRedisKey", Value::String("mykey".into()))
    .to("redis://localhost:6379?command=GET")
    .build()?;
```

### Hash Operations

```rust
// HSET: Set hash field
let route = RouteBuilder::from("direct:hset")
    .set_header("CamelRedisKey", Value::String("user:123".into()))
    .set_header("CamelRedisField", Value::String("name".into()))
    .set_body(Body::Text("Alice".into()))
    .to("redis://localhost:6379?command=HSET")
    .build()?;

// HGETALL: Get all hash fields
let route = RouteBuilder::from("direct:hgetall")
    .set_header("CamelRedisKey", Value::String("user:123".into()))
    .to("redis://localhost:6379?command=HGETALL")
    .build()?;
```

### List Operations

```rust
// LPUSH: Add to list
let route = RouteBuilder::from("direct:push")
    .set_header("CamelRedisKey", Value::String("mylist".into()))
    .set_body(Body::Text("item1".into()))
    .to("redis://localhost:6379?command=LPUSH")
    .build()?;

// LRANGE: Get list range
let route = RouteBuilder::from("direct:range")
    .set_header("CamelRedisKey", Value::String("mylist".into()))
    .to("redis://localhost:6379?command=LRANGE")
    .build()?;
```

### Pub/Sub (Consumer)

```rust
// Subscribe to channels
let route = RouteBuilder::from("redis://localhost:6379?command=SUBSCRIBE&channels=events,notifications")
    .log("Received message", camel_processor::LogLevel::Info)
    .to("direct:process")
    .build()?;
```

### Blocking List Consumer

```rust
// BLPOP: Block until item available
let route = RouteBuilder::from("redis://localhost:6379?command=BLPOP&key=jobqueue&timeout=0")
    .process(|ex| async move {
        // Process job from queue
        Ok(ex)
    })
    .build()?;
```

### With Authentication

```rust
let route = RouteBuilder::from("direct:auth")
    .to("redis://localhost:6379?command=GET&password=secret&db=2")
    .build()?;
```

## Exchange Headers

### Input Headers (for commands)

| Header | Description |
|--------|-------------|
| `CamelRedisKey` | Redis key |
| `CamelRedisField` | Hash field (for H* commands) |
| `CamelRedisChannel` | Pub/Sub channel |
| `CamelRedisScore` | Sorted set score |
| `CamelRedisStart` | Range start |
| `CamelRedisEnd` | Range end |
| `CamelRedisTimeout` | Blocking timeout |

### Output Headers (from responses)

| Header | Description |
|--------|-------------|
| `CamelRedisResult` | Command result |

## Example: Caching Layer

```rust
use camel_builder::RouteBuilder;
use camel_component_redis::RedisComponent;
use camel_core::CamelContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ctx = CamelContext::new();
    ctx.register_component("redis", Box::new(RedisComponent::new()));

    // Cache-aside pattern
    let route = RouteBuilder::from("direct:get-user")
        // Try cache first
        .set_header("CamelRedisKey", Value::String("user:123".into()))
        .to("redis://localhost:6379?command=GET")
        .filter(|ex| ex.input.body.as_text().is_none())
            // Cache miss - fetch from DB
            .process(|ex| async move {
                // Fetch from database
                let user = fetch_user_from_db(123).await;
                let mut ex = ex;
                ex.input.body = Body::Text(serde_json::to_string(&user)?);
                Ok(ex)
            })
            // Store in cache
            .set_header("CamelRedisKey", Value::String("user:123".into()))
            .to("redis://localhost:6379?command=SET")
        .end_filter()
        .build()?;

    ctx.add_route(route).await?;
    ctx.start().await?;

    Ok(())
}
```

## Example: Job Queue

```rust
// Producer: Push jobs
let producer = RouteBuilder::from("direct:submit-job")
    .set_header("CamelRedisKey", Value::String("jobs".into()))
    .to("redis://localhost:6379?command=RPUSH")
    .build()?;

// Consumer: Process jobs
let consumer = RouteBuilder::from("redis://localhost:6379?command=BLPOP&key=jobs&timeout=30")
    .process(|ex| async move {
        // Process the job
        let job = ex.input.body.as_text().unwrap_or("");
        println!("Processing job: {}", job);
        Ok(ex)
    })
    .build()?;
```

## Documentation

- [API Documentation](https://docs.rs/camel-component-redis)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
