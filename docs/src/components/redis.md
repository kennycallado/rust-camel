# Redis

The Redis component executes Redis commands and subscribes to Redis channels. One crate covers both directions. The Producer sends the Exchange body to Redis as a command argument. The Consumer subscribes to Pub/Sub channels or blocks on a list key. The `redis` URI scheme uses plaintext. The `rediss` URI scheme uses TLS.

The redis-example wires a string producer, a Pub/Sub consumer, a queue consumer, and a Pub/Sub producer against a testcontainers Redis instance:

```rust,ignore
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_redis::RedisComponent;

ctx.register_component("redis", Box::new(RedisComponent::new()));

// Producer: timer writes a key every 3s
let string_producer = RouteBuilder::from("timer:tick?period=3000&repeatCount=3")
    .route_id("redis-string-producer")
    .set_header("CamelRedis.Key", Value::String("greeting".into()))
    .set_header(
        "CamelRedis.Value",
        Value::String("hello from rust-camel!".into()),
    )
    .to("redis://127.0.0.1:6379?command=SET")
    .to("log:info?showHeaders=true")
    .build()?;
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: redis-string-producer
    from: "timer:tick?period=3000&repeatCount=3"
    steps:
      - set_header:
          CamelRedis.Key: "greeting"
      - set_header:
          CamelRedis.Value: "hello from rust-camel!"
      - to: "redis://127.0.0.1:6379?command=SET"
      - to: "log:info?showHeaders=true"
```

The example reads the Redis port from a testcontainers container. Substitute your real broker address in `redis://`.

</details>

```rust,ignore
// Consumer: BRPOP blocks on a list key, one Exchange per popped item
let queue_consumer = RouteBuilder::from(
    "redis://127.0.0.1:6379?command=BRPOP&key=demo-queue&timeout=2",
)
.route_id("redis-queue-consumer")
.to("log:info?showAll=true")
.build()?;

// Consumer: SUBSCRIBE receives published messages as Exchanges
let pubsub_consumer = RouteBuilder::from(
    "redis://127.0.0.1:6379?command=SUBSCRIBE&channels=demo-channel",
)
.route_id("redis-pubsub-consumer")
.to("log:info?showAll=true")
.build()?;
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: redis-queue-consumer
    from: "redis://127.0.0.1:6379?command=BRPOP&key=demo-queue&timeout=2"
    steps:
      - to: "log:info?showAll=true"
  - id: redis-pubsub-consumer
    from: "redis://127.0.0.1:6379?command=SUBSCRIBE&channels=demo-channel"
    steps:
      - to: "log:info?showAll=true"
```

</details>

## URI

```text
redis://host:port?command=<cmd>[&key=<key>][&channels=<list>][&timeout=<secs>][&password=<pwd>][&db=<n>][&ssl=<bool>]
```

| Parameter | Required | Default | Description |
| --- | --- | --- | --- |
| `command` | no | `SET` | Redis command to execute |
| `key` | per-command | none | Redis key for the operation |
| `channels` | Pub/Sub | empty | Comma-separated channel names |
| `timeout` | blocking | `1` | Blocking timeout in seconds |
| `password` | no | none | Redis password |
| `db` | no | `0` | Redis database number (0-255) |
| `ssl` | no | auto | Force TLS on or off |

The `command` parameter picks the Redis command at Endpoint creation. Exchange data never becomes a command name. Dynamic values like keys, fields, values, channels, and scores cross the trust boundary as length-prefixed Redis protocol arguments. Argument contents cannot inject a second command or change the selected command (CONTEXT "Trust boundary"). Missing required headers return `CamelError`.

## Commands

The component exposes 80+ commands across eight groups. The enum is exhaustive: an unknown command fails URI parsing with `CamelError::InvalidUri`. The component does not expose `EVAL`, `EVALSHA`, or script-loading commands. Script injection through the public surface is not possible.

| Group | Commands |
| --- | --- |
| String | `SET`, `GET`, `GETSET`, `SETNX`, `SETEX`, `MGET`, `MSET`, `INCR`, `INCRBY`, `DECR`, `DECRBY`, `APPEND`, `STRLEN` |
| Key | `EXISTS`, `DEL`, `EXPIRE`, `EXPIREAT`, `PEXPIRE`, `PEXPIREAT`, `TTL`, `KEYS`, `RENAME`, `RENAMENX`, `TYPE`, `PERSIST`, `MOVE`, `SORT` |
| List | `LPUSH`, `RPUSH`, `LPUSHX`, `RPUSHX`, `LPOP`, `RPOP`, `BLPOP`, `BRPOP`, `LLEN`, `LRANGE`, `LINDEX`, `LINSERT`, `LSET`, `LREM`, `LTRIM`, `RPOPLPUSH` |
| Hash | `HSET`, `HGET`, `HSETNX`, `HMSET`, `HMGET`, `HDEL`, `HEXISTS`, `HLEN`, `HKEYS`, `HVALS`, `HGETALL`, `HINCRBY` |
| Set | `SADD`, `SREM`, `SMEMBERS`, `SCARD`, `SISMEMBER`, `SPOP`, `SMOVE`, `SINTER`, `SUNION`, `SDIFF`, `SINTERSTORE`, `SUNIONSTORE`, `SDIFFSTORE`, `SRANDMEMBER` |
| Sorted set | `ZADD`, `ZREM`, `ZRANGE`, `ZREVRANGE`, `ZRANK`, `ZREVRANK`, `ZSCORE`, `ZCARD`, `ZINCRBY`, `ZCOUNT`, `ZRANGEBYSCORE`, `ZREVRANGEBYSCORE`, `ZREMRANGEBYRANK`, `ZREMRANGEBYSCORE`, `ZUNIONSTORE`, `ZINTERSTORE` |
| Pub/Sub | `PUBLISH`, `SUBSCRIBE`, `PSUBSCRIBE` |
| Other | `PING`, `ECHO` |

## Producer

`redis://host:port?command=GET` sends the Exchange body to Redis. The Producer holds a single multiplexed connection per Endpoint. The connection opens lazily on the first call and stays open for the route lifetime.

The `command` parameter picks one of the 80+ commands listed above. The Exchange body, headers, and the URI parameters supply the command arguments. Different commands read different headers. For example, `HSET` reads `CamelRedis.Key` and `CamelRedis.Value`. `LRANGE` reads `CamelRedis.Start` and `CamelRedis.End`. Missing required headers return `CamelError`. A send failure returns `Err` to the route `ErrorHandler`.

The Producer is a Tower `Service<Exchange>`. It composes with any pipeline step and reports per-route metrics.

## Consumer

`redis://host:port?command=SUBSCRIBE&channels=foo,bar` subscribes to one or more Pub/Sub channels. The Consumer submits one Exchange per published message. The `CamelRedis.Channel` header carries the channel name. `CamelRedis.Pattern` carries the matched pattern for `PSUBSCRIBE`.

`redis://host:port?command=BLPOP&key=jobs&timeout=5` blocks on a list key and submits one Exchange per popped item. The `CamelRedis.Key` header carries the list key. The `timeout` parameter is the block duration in seconds. Use `BLPOP` for left pop and `BRPOP` for right pop.

The Consumer's mode comes from the URI command. `SUBSCRIBE` and `PSUBSCRIBE` use Pub/Sub mode. `BLPOP` and `BRPOP` use queue mode. A command that fits neither returns an error at consumer creation. The component does not silently fall back to BLPOP (REDIS-003).

## Security

The component supports TLS through the `rediss://` URI scheme or the `ssl=true` parameter. The two are equivalent. TLS auto-enables for non-loopback hosts when the `redis` crate is compiled with a TLS feature (`tls-rustls-webpki-roots`, `tls-rustls-native-certs`, or `tls-native-tls`). The component logs a `tracing::warn!` when auto-enabling TLS. A missing feature gate stops startup with the required `cargo add` command.

The component redacts passwords in `Debug` output. Passwords with special characters (`@`, `:`, `/`) are percent-encoded in the connection URL. The `safe_endpoint()` helper returns a credential-free identifier for tracing.

## Connection handling

The Producer holds a single multiplexed connection per Endpoint. The Consumer holds one connection for Pub/Sub mode and one for queue mode. Each connection has a 10-second timeout by default. Transient transport errors trigger reconnect with the configured `NetworkRetryPolicy`. The route `ErrorHandler` owns the operational signal for non-transient errors.

The component registers an async health check that sends a `PING` command. The probe is healthy when Redis responds with `PONG` and degraded when PING fails or times out.

## Error handling

The Consumer logs at `error!` for channel-closed conditions on Pub/Sub and BLPOP send paths and for retry-exhaustion. Each site reports a typed metric before the log line. The Producer logs send failures at `warn!`. Per-message non-transient Redis errors log at `error!` with a typed metric. The route handler owns the operational signal for transient producer errors.

**Reference**: [Redis crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-redis/CONTEXT.md). Example source: [`examples/redis-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/redis-example).
