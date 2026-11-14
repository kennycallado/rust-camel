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
- **Health Check**: Async PING-based probe for Redis connectivity

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-redis = "*"
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
| `group` | - | Consumer-group name for `XREADGROUP` |
| `consumer` | - | Consumer name within the group |
| `count` | server default | Maximum entries returned per `XREADGROUP` read; must be positive |
| `block` | `10000` | `XREADGROUP` block timeout (milliseconds); `0` blocks indefinitely |
| `streamRecoverEnabled` | `false` | Recover pending stream entries (opt-in) |
| `streamRecoverMinIdleMs` | `60000` | Minimum pending idle time for recovery, ms (`1..=3600000`) |
| `streamRecoverBatch` | `10` | Maximum pending entries claimed per recovery pass (`1..=100`) |
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

// SET: Store the exchange body under a key
let set_route = RouteBuilder::from("direct:set")
    .set_header("CamelRedis.Key", Value::String("mykey".into()))
    .set_body("hello")
    .to("redis://localhost:6379?command=SET")
    .build()?;

// GET: Retrieve a value
let get_route = RouteBuilder::from("direct:get")
    .set_header("CamelRedis.Key", Value::String("mykey".into()))
    .to("redis://localhost:6379?command=GET")
    .build()?;
```

### Hash Operations

```rust
// HSET: Set hash field
let route = RouteBuilder::from("direct:hset")
    .set_header("CamelRedis.Key", Value::String("user:123".into()))
    .set_header("CamelRedis.Field", Value::String("name".into()))
    .set_body(Body::Text("Alice".into()))
    .to("redis://localhost:6379?command=HSET")
    .build()?;

// HGETALL: Get all hash fields
let route = RouteBuilder::from("direct:hgetall")
    .set_header("CamelRedis.Key", Value::String("user:123".into()))
    .to("redis://localhost:6379?command=HGETALL")
    .build()?;
```

### List Operations

```rust
// LPUSH: Add to list
let route = RouteBuilder::from("direct:push")
    .set_header("CamelRedis.Key", Value::String("mylist".into()))
    .set_body(Body::Text("item1".into()))
    .to("redis://localhost:6379?command=LPUSH")
    .build()?;

// LRANGE: Get list range
let route = RouteBuilder::from("direct:range")
    .set_header("CamelRedis.Key", Value::String("mylist".into()))
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

Queue-mode consumers pop on a connection built with `set_response_timeout(None)`
and `set_connection_timeout(None)`, so the driver sets no response deadline.
The pop `timeout`, the transient retry budget, and route cancellation bound the
pop; `connection_timeout_secs` bounds only the TCP connect. Producer endpoints
whose command is `BLPOP` or `BRPOP` build the same deadline-free connections.

#### Delivery guarantee

The queue consumer gives **at-most-once** delivery. It supports `BLPOP` and
`BRPOP`; both go through the same `run_queue_consumer` path and the same
`deliver` closure.

The command pops the item from the Redis list **before** the route processes it.
The consumer then passes the exchange to the pipeline with `ctx.send` and does
not wait for the route outcome. There is no ack and no re-queue.

Consequence: if the route fails, or the process crashes after the pop, the item
is gone from the list. The only way to keep it is for the route itself to
preserve it. Outcome-gated delivery (for example an `LMOVE` reliable-queue mode)
is **not implemented** and is out of scope.

#### Dead-letter pattern

Because delivery is at-most-once, preserve failed items in the route. Use the
route dead-letter channel (`RouteBuilder::dead_letter_channel`) and send the
failed exchange to a second route that pushes it onto a dead-letter list.

The consumer sets the header `CamelRedis.Key` to the source list (`jobs`). The
dead-letter route must set `CamelRedis.Key` itself; otherwise `LPUSH` would
write the item back to the source list. The value comes from the exchange body.

```rust
use camel_api::Body;
use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};

let jobs = RouteBuilder::from("redis://localhost:6379?command=BLPOP&key=jobs&timeout=30")
    .route_id("jobs")
    .dead_letter_channel("direct:jobs-dead")
    .process(|ex| async move {
        // Process the job. Returning Err sends the failed exchange to the DLC.
        Ok(ex)
    })
    .build()?;

let jobs_dead = RouteBuilder::from("direct:jobs-dead")
    .route_id("jobs-dead")
    .set_header("CamelRedis.Key", Value::String("jobs:dead".into()))
    .set_body_fn(|ex| {
        // The body is the popped item (Text). Build the value from it.
        Body::Text(ex.input.body.as_text().unwrap_or_default().to_string())
    })
    .to("redis://localhost:6379?command=LPUSH")
    .build()?;
```

### Streams Consumer Group

```rust
// XREADGROUP: read new entries for a consumer group
let route = RouteBuilder::from(
    "redis://localhost:6379?command=XREADGROUP&key=events&group=workers&consumer=worker-1",
)
.log("Received entry", camel_processor::LogLevel::Info)
.build()?;
```

`key`, `group`, and `consumer` are required. The Consumer delivers one Exchange
per entry through `send_and_wait` and issues `XACK` only after the pipeline
accepts the entry. A failed delivery logs at `warn!`, leaves the entry pending,
and processing continues.

Each delivered Exchange carries `CamelRedis.Stream`, `CamelRedis.StreamId`,
`CamelRedis.StreamGroup`, `CamelRedis.StreamConsumer`, and
`CamelRedis.StreamDeliveryCount`. The delivery count is the number of deliveries
the consumer group has made for the entry, including the current one: a first
`>` delivery carries `1`, and a recovered entry carries the post-claim count read
from Redis after the claim.

#### Pending-entry recovery

Recovery is opt-in with `streamRecoverEnabled=true`. When enabled, a pass runs
after the first successful connect and then at most once per
`streamRecoverMinIdleMs`, measured from the start of the previous pass. The pass
runs on a dedicated recovery connection and claims entries idle for at least
`streamRecoverMinIdleMs`, in batches of at most `streamRecoverBatch` entries per
pass. Claimed entries use the same delivery path as `>` reads and are
acknowledged only after successful delivery. A failed delivery stays pending for
a later pass. Shutdown stops an in-progress pass promptly.

### With Authentication

```rust
let route = RouteBuilder::from("direct:auth")
    .to("redis://localhost:6379?command=GET&password=secret&db=2")
    .build()?;
```

## Exchange Headers

### Input Headers (for commands)

The table below lists common headers. See the
[Redis component guide](../../../docs/src/components/redis.md) for numeric and geospatial header rules.
The [command handlers](src/commands/) define the inputs for each command.

| Header | Description |
|--------|-------------|
| `CamelRedis.Key` | Redis key |
| `CamelRedis.Field` | Hash field (for H* commands) |
| `CamelRedis.Channel` | Pub/Sub channel |
| `CamelRedis.Score` | Sorted set score |
| `CamelRedis.Start` | Range start |
| `CamelRedis.End` | Range end |
| `CamelRedis.Timeout` | Blocking timeout |

The payload of a writing command comes from the exchange body, not from a
header. See [Value payload](#value-payload).

### Command Results (from responses)

Command results are written to the message body, not to an exchange header. Most
results are JSON (`Body::Json`). `GET` replies with `Body::Text` when the value
is valid UTF-8, `Body::Bytes` when it is not, and `Body::Json(null)` for a
missing key. `MGET` keeps the JSON array of strings and nulls when every element
is nil or valid UTF-8. When any element is binary, `MGET` replies with
`Body::Bytes` that holds the RESP encoding of the reply array. All other getters
keep JSON replies.

See [Value payload](#value-payload) for the body codec, the map and stream
rules, and the removed payload headers ([ADR-0098](../../../docs/adr/0098-redis-value-contract.md)).

### Value payload

A writing command takes its payload from the exchange body only. No value
header exists.

| Body variant | Payload bytes |
|--------------|---------------|
| `Body::Text`, `Body::Xml` | UTF-8 bytes |
| `Body::Json` holding a string | the raw characters, with no JSON quoting |
| `Body::Json` holding any other value | compact JSON |
| `Body::Bytes` | the bytes verbatim |
| `Body::Empty`, `Body::Stream`, `Body::Json(null)` | rejected before send |

`HMSET` and `MSET` take a JSON-object body, or a `Body::Text`/UTF-8
`Body::Bytes` body that parses as a JSON object. Each entry value follows the
scalar rule. A null entry fails before send. Binary map values are a documented
limit, because the map body is JSON or UTF-8 text.

`XADD` keeps its one-`body`-field rule. A JSON-object body contributes one
field value that holds the compact JSON of the object. It is never split into
one field per key.

#### Removed headers and migration

These headers are removed. A route that still sets one fails before any frame
is sent, on the command set in the table.

| Removed header | Command set |
|----------------|-------------|
| `CamelRedis.Value` | `SET`, `SETEX`, `SETNX`, `GETSET`, `APPEND`, `LPUSH`, `RPUSH`, `LPUSHX`, `RPUSHX`, `LINSERT`, `LSET`, `LREM`, `SADD`, `SREM`, `SISMEMBER`, `SMOVE`, `ZADD`, `ZREM`, `ZRANK`, `ZREVRANK`, `ZSCORE`, `ZINCRBY`, `HSET`, `HSETNX`, `PUBLISH` |
| `CamelRedis.Values` | `HMSET`, `MSET` |
| `CamelRedis.Member` | `GEOADD` (payload only) |

The error text is `"<header> was removed in 0.69; put the value in the body"`.
`CamelRedis.Member`, `CamelRedis.Member2`, and `CamelRedis.Members` keep their
addressing-operand roles on `GEOPOS`, `GEODIST`, `GEOSEARCH`, and `GEOHASH`.

To migrate, move the header value into the body before the Redis step:

```rust
// Before
let route = RouteBuilder::from("direct:store")
    .set_header("CamelRedis.Key", Value::String("mykey".into()))
    .set_header("CamelRedis.Value", Value::String(payload.clone()))
    .to("redis://localhost:6379?command=SET")
    .build()?;

// After
let route = RouteBuilder::from("direct:store")
    .set_header("CamelRedis.Key", Value::String("mykey".into()))
    .set_body(payload)
    .to("redis://localhost:6379?command=SET")
    .build()?;
```

A route that needs a fixed value and the original body for logging keeps the
body in a property, then sets the fixed value as the body. The `enrich` EIP is
the alternative when the payload comes from a second source.

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
        .set_header("CamelRedis.Key", Value::String("user:123".into()))
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
            .set_header("CamelRedis.Key", Value::String("user:123".into()))
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
// Producer: Push the job body onto the list
let producer = RouteBuilder::from("direct:submit-job")
    .set_header("CamelRedis.Key", Value::String("jobs".into()))
    .set_body("job-42")
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

This example processes jobs but does not preserve them: a failed job is lost.
See "Delivery guarantee" and "Dead-letter pattern" above. A production job queue
should add a dead-letter channel so failed jobs survive for inspection and
replay.

## Global Configuration

Configure default Redis connection settings in `Camel.toml` that apply to all Redis endpoints:

```toml
[default.components.redis]
host = "localhost"       # Redis host (default: localhost)
port = 6379              # Redis port (default: 6379)
```

URI parameters always override global defaults:

```rust
// Uses global host/port (localhost:6379)
.to("redis://?command=GET")

// Overrides port from global config
.to("redis://:6380?command=GET")

// Full override with different host
.to("redis://redis-prod:6379?command=GET")
```

### Profile-Specific Configuration

```toml
[default.components.redis]
host = "localhost"
port = 6379

[production.components.redis]
host = "redis-prod.internal"
port = 6379
```

## Health Check

The `camel-component-redis` component registers an async health check via `AsyncHealthCheck`.

- **Probe**: Sends a Redis `PING` command and validates the response
- **Healthy**: Redis responds to PING with PONG
- **Degraded**: PING fails or times out

Health checks are exposed via the health server:

```toml
[observability.health]
enabled = true
port = 8080
```

## Documentation

- [API Documentation](https://docs.rs/camel-component-redis)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
