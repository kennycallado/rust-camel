# Redis

The Redis component executes Redis commands and subscribes to Redis channels. One crate covers both directions. The Producer sends the Exchange body to Redis as a command argument, or appends it to a stream with `XADD`. The Consumer subscribes to Pub/Sub channels, blocks on a list key, or reads a consumer group with `XREADGROUP`. The `redis` scheme is the route URI scheme. Enable TLS with `ssl = true` on the URI, or `tls_mode` in config; TLS auto-enables for non-loopback hosts. Sentinel clusters register under `redis-sentinel` and `rediss-sentinel`, where a `rediss://` node URL requires TLS.

The redis-example wires a string producer, a Pub/Sub consumer, a queue consumer, and a Pub/Sub producer against a testcontainers Redis instance:

```rust,ignore
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_redis::RedisComponent;

ctx.register_component("redis", Box::new(RedisComponent::new()));

// Producer: timer writes a key every 3s; the body is the value
let string_producer = RouteBuilder::from("timer:tick?period=3000&repeatCount=3")
    .route_id("redis-string-producer")
    .set_header("CamelRedis.Key", Value::String("greeting".into()))
    .set_body("hello from rust-camel!")
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
          key: "CamelRedis.Key"
          value: "greeting"
      - set_body: "hello from rust-camel!"
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
redis://host:port?command=<cmd>[&key=<key>][&channels=<list>][&timeout=<secs>][&group=<name>][&consumer=<name>][&count=<n>][&block=<ms>][&createGroup=<bool>][&groupStart=<latest|earliest>][&streamRecoverEnabled=<bool>][&streamRecoverMinIdleMs=<ms>][&streamRecoverBatch=<n>][&script=<source>][&scriptFile=<path>][&function=<name>][&idempotent=<bool>][&atomic=<bool>][&password=<pwd>][&db=<n>][&ssl=<bool>]
```

| Parameter | Required | Default | Description |
| --- | --- | --- | --- |
| `command` | no | `SET` | Redis command to execute |
| `key` | per-command | none | Redis key for the operation |
| `channels` | Pub/Sub | empty | Comma-separated channel names |
| `timeout` | queue Consumer | `1` | Blocking pop timeout in seconds |
| `group` | Streams Consumer | none | Consumer-group name for `XREADGROUP` |
| `consumer` | Streams Consumer | none | Consumer name within the group |
| `count` | no | server default | Maximum entries returned per `XREADGROUP` read; positive |
| `block` | no | `10000` | `XREADGROUP` block timeout in milliseconds; `0` blocks indefinitely |
| `createGroup` | no | `false` | Create the consumer group before the first read (Streams Consumer) |
| `groupStart` | with `createGroup` | none | Group start position, `latest` (`$`) or `earliest` (`0`) |
| `streamRecoverEnabled` | no | `false` | Recover pending stream entries (opt-in) |
| `streamRecoverMinIdleMs` | no | `60000` | Minimum pending idle time for recovery, in ms (`1..=3600000`) |
| `streamRecoverBatch` | no | `10` | Maximum pending entries claimed per recovery pass (`1..=100`) |
| `password` | no | none | Redis password |
| `db` | no | `0` | Redis database number (0-16383) |
| `ssl` | no | auto | Force TLS on or off |
| `script` | `eval`, `fcall` (one source) | none | Inline Lua script or function-library source |
| `scriptFile` | `eval`, `fcall` (one source) | none | Path to the script or function-library source; a relative path resolves against the route-file directory |
| `function` | `fcall` | none | Registered function name to invoke |
| `idempotent` | no | `false` | Declare the script replay-safe after an outcome-unknown transport failure |
| `atomic` | no | `true` | Run a `command=multi` batch inside one `MULTI`/`EXEC` transaction |

The `command` parameter picks the Redis command at Endpoint creation. For an ordinary single-command endpoint, exchange data never becomes a command name. Two bounded exceptions exist. A `command=multi` endpoint selects each entry command from the `CamelRedis.Batch` header, and the component validates every entry against the typed allowlist (see [Batch (multi)](#batch-multi)). An `eval` or `fcall` endpoint binds its keys and arguments from the Exchange body (see [Script commands](#script-commands)). Script text never comes from exchange data. Dynamic values like keys, fields, values, channels, and scores cross the trust boundary as length-prefixed Redis protocol arguments. Argument contents cannot inject a second command or change the selected command (CONTEXT "Trust boundary"). Missing required headers return `CamelError`.

## Header values and timeouts

The Producer reads command arguments from `CamelRedis.*` headers. The payload
of a writing command comes from the Exchange body (see
[value payload and reply rule](#value-payload-and-reply-rule)). A numeric
header accepts a JSON number or a trimmed numeric string. The numeric-string
form makes the natural `${env:...}` path work, because environment
interpolation always yields a string.

A header that is present but invalid fails the step with a named error. The error names the header and the received value. An absent header keeps its documented default. The component never silently defaults a misconfigured numeric header to zero. A non-negative header rejects a negative value. A value above `i64::MAX` is never wrapped into a negative number.

### Expiration

Redis reads a zero expiry argument as an immediate key deletion. `SETEX` rejects a zero TTL outright. The component refuses a missing or zero expiry before it sends any command, so no key deletion can result from a missing or zero value.

| Command | Header | Unit | Rule |
| --- | --- | --- | --- |
| `SETEX` | `CamelRedis.Timeout` | seconds | positive required |
| `EXPIRE` | `CamelRedis.Timeout` | seconds | positive required |
| `PEXPIRE` | `CamelRedis.Timeout` | milliseconds | positive required |
| `EXPIREAT` | `CamelRedis.Timestamp` | seconds | positive required |
| `PEXPIREAT` | `CamelRedis.Timestamp` | milliseconds | positive required |

`CamelRedis.Timestamp` must fit in a signed 64-bit integer. A larger value would wrap to a negative number, which Redis reads as an already-past timestamp, so the component rejects it. A positive past timestamp keeps the Redis deletion semantics. Use a future timestamp for a real expiry. Use `DEL` to delete a key on purpose.

### SET and GETEX options

`SET` takes a write condition, an expiry, and a return-old-value flag from headers. `GETEX` takes an expiry and a persist flag. The Producer translates each header into the Redis argument of the same meaning.

| Header | Values | Used by | Default | Rule |
| --- | --- | --- | --- | --- |
| `CamelRedis.SetCondition` | `NX` or `XX` (case-insensitive) | `SET` | none | one header holds one condition, so `NX` + `XX` is not expressible; any other value fails before send |
| `CamelRedis.KeepTtl` | boolean | `SET` | `false` | conflict with `CamelRedis.Timeout` or `CamelRedis.Timestamp` fails before send |
| `CamelRedis.ReturnOldValue` | boolean | `SET` | `false` | adds `GET`; the reply body is the old value |
| `CamelRedis.ExpireUnit` | `SECONDS` or `MILLISECONDS` (case-insensitive) | `SET`, `GETEX` | `SECONDS` | requires `CamelRedis.Timeout` or `CamelRedis.Timestamp`; a unit header alone fails before send |
| `CamelRedis.Persist` | boolean | `GETEX` | `false` | conflict with `CamelRedis.Timeout` or `CamelRedis.Timestamp` fails before send |

`CamelRedis.Timeout` and `CamelRedis.Timestamp` are mutually exclusive. `Timeout` selects a relative expiry (`EX` or `PX`). `Timestamp` selects an absolute expiry (`EXAT` or `PXAT`). `CamelRedis.ExpireUnit` selects the unit for both and defaults to `SECONDS`, which matches the `SETEX` and `EXPIREAT` meaning of a bare value.

`CamelRedis.KeepTtl` and any expiry header are mutually exclusive. `KEEPTTL` keeps the current expiry, and an expiry argument would replace it. `CamelRedis.Persist` is the `GETEX` opposite: it removes the expiry, so it cannot combine with an expiry header.

`CamelRedis.ReturnOldValue` adds the `GET` argument. The reply body then carries the old value. A missing key or an unmet `NX`/`XX` condition yields a null body.

A present but invalid boolean or enum value fails before send with a named error. The error names the header and the received value.

`SET` is not retried after a transport error. The retry classifier works per command and cannot see the option headers. A retry of `SET ... GET` reports the post-write value as the old value. A retry of `SET ... NX` reports a failed condition after an unconfirmed success. The worst-case option form governs, so a plain `SET` loses the retry too. There is no per-route classifier override, because the Producer always consults the classifier. A route that needs retry for a plain `SET` must use route-level redelivery. Redelivery re-executes the whole exchange, so it can repeat other side effects.

### Blocking pops

`BLPOP`, `BRPOP`, `BLMPOP`, `BZMPOP`, and `BLMOVE` are the blocking endpoints. Each one accepts an optional `CamelRedis.Timeout` in seconds. An absent value or `0` blocks indefinitely, which is the Redis command meaning. A non-numeric value fails with a named error. Their connections carry no driver response deadline, so a blocked call waits until Redis replies. See [Blocking pops and connection timeouts](#blocking-pops-and-connection-timeouts).

On the Consumer, the URI `timeout` parameter sets the block duration and defaults to `1` second. Queue mode accepts only `BLPOP` and `BRPOP`.

### Sorted-set score bounds

`CamelRedis.Min` and `CamelRedis.Max` are optional score bounds. An absent bound is unbounded. The component rejects an explicit `inf`, `-inf`, or `NaN`. Omit the header to mean unbounded.

### Environment values

`${env:VAR}` interpolation always produces a string. The component parses a trimmed numeric string, so the common case works directly:

```yaml
- set_header:
    key: CamelRedis.Timeout
    value: "${env:TTL_SECONDS:-45}"
```

For a computed value, use a Rhai expression. `parse_int` converts the string to an integer, and `parse_float` does the same for a decimal. Interpolation runs before evaluation, so the expression sees the concrete value:

```yaml
- set_header:
    key: CamelRedis.Timeout
    rhai: 'parse_int("${env:TTL_MINUTES:-5}") * 60'
```

## Commands

The component exposes 120+ commands across fourteen groups. The enum is exhaustive: an unknown command fails URI parsing with `CamelError::InvalidUri`.

| Group | Commands |
| --- | --- |
| String | `SET`, `GET`, `GETDEL`, `GETEX`, `GETSET`, `SETNX`, `SETEX`, `SETRANGE`, `GETRANGE`, `MGET`, `MSET`, `INCR`, `INCRBY`, `DECR`, `DECRBY`, `APPEND`, `STRLEN` |
| Bitmap | `SETBIT`, `GETBIT`, `BITCOUNT`, `BITPOS`, `BITOP`, `BITFIELD`, `BITFIELD_RO` |
| HyperLogLog | `PFADD`, `PFCOUNT`, `PFMERGE` |
| Key | `EXISTS`, `DEL`, `UNLINK`, `EXPIRE`, `EXPIREAT`, `PEXPIRE`, `PEXPIREAT`, `TTL`, `KEYS`, `RENAME`, `RENAMENX`, `COPY`, `TYPE`, `PERSIST`, `MOVE`, `TOUCH`, `SORT` |
| List | `LPUSH`, `RPUSH`, `LPUSHX`, `RPUSHX`, `LPOP`, `RPOP`, `BLPOP`, `BRPOP`, `LLEN`, `LRANGE`, `LINDEX`, `LINSERT`, `LSET`, `LREM`, `LTRIM`, `RPOPLPUSH`, `LMPOP`, `BLMPOP`, `LMOVE`, `BLMOVE`, `LPOS` |
| Hash | `HSET`, `HGET`, `HSETNX`, `HMSET`, `HMGET`, `HDEL`, `HEXISTS`, `HLEN`, `HKEYS`, `HVALS`, `HGETALL`, `HINCRBY`, `HRANDFIELD`, `HINCRBYFLOAT` |
| Set | `SADD`, `SREM`, `SMEMBERS`, `SCARD`, `SISMEMBER`, `SPOP`, `SMOVE`, `SINTER`, `SUNION`, `SDIFF`, `SINTERSTORE`, `SUNIONSTORE`, `SDIFFSTORE`, `SRANDMEMBER`, `SMISMEMBER`, `SINTERCARD` |
| Sorted set | `ZADD`, `ZREM`, `ZRANGE`, `ZREVRANGE`, `ZRANK`, `ZREVRANK`, `ZSCORE`, `ZCARD`, `ZINCRBY`, `ZCOUNT`, `ZRANGEBYSCORE`, `ZREVRANGEBYSCORE`, `ZREMRANGEBYRANK`, `ZREMRANGEBYSCORE`, `ZUNIONSTORE`, `ZINTERSTORE`, `ZMPOP`, `BZMPOP`, `ZRANDMEMBER`, `ZDIFF`, `ZINTER`, `ZUNION`, `ZRANGEBYLEX` |
| Pub/Sub | `PUBLISH`, `SUBSCRIBE`, `PSUBSCRIBE` |
| Stream | `XREADGROUP`, `XADD`, `XLEN` |
| Geospatial | `GEOADD`, `GEOPOS`, `GEODIST`, `GEOSEARCH`, `GEOHASH` |
| Script | `eval`, `fcall` |
| Batch | `multi` |
| Other | `PING`, `ECHO`, `WAIT` |

Stream commands require Redis >= 7.0 (Valkey >= 7.2 compatible). `XADD` and `XLEN` are Producer commands. `XREADGROUP` is Consumer-only. The component does not expose `ACKED` or `IDMP`. `eval` and `fcall` are Producer commands with their own operand contract; see [Script commands](#script-commands). `multi` is a Producer batch request rather than a Redis verb: it runs the entries of the `CamelRedis.Batch` header as one pipeline, see [Batch (multi)](#batch-multi).

### Phase 1b part 2 decisions

Three decisions govern the bitmap and WAIT commands. The per-family sections below restate each rule with its error cases.

- **BITFIELD overflow is always explicit.** `CamelRedis.BitFieldOverflow` (`WRAP`, `SAT`, or `FAIL`) sets the initial mode. An inline `OVERFLOW` entry switches the mode for the entries that follow it. A `SET` or `INCRBY` entry with no effective mode fails before send. There is no default, because the server default `WRAP` would silently wrap counters. See [Bitmap command inputs](#bitmap-command-inputs).
- **WAIT is a connection-local acknowledgement probe.** It acknowledges the writes that the invoking connection sent before the `WAIT` command. A different pooled connection or a reconnect resets that scope, so `WAIT` is not a durability guarantee for earlier route writes. The reply is the count of acknowledged replicas as a JSON number. `WAIT` is idempotent: it is a barrier, so a replay is safe. `CamelRedis.WaitTimeout` bounds the probe: the value is required, greater than `0`, and in milliseconds. A WAIT-only aggregate deadline of `WaitTimeout` plus the endpoint `timeout` also bounds the exchange, so the probe can never hang forever. The WAIT connection carries no driver response deadline, so the driver never aborts a valid slow probe inside its server-side timeout. See [WAIT durability probe](#wait-durability-probe).
- **`#<n>` BITFIELD offsets are independent.** The offset is `n × <entry width>` bits, evaluated for that entry alone. The product is never accumulated across entries. See [Bitmap command inputs](#bitmap-command-inputs).

### String command inputs

These four string commands read their addressing operands from headers. `SETRANGE` takes its payload from the Exchange body, under the [value payload and reply rule](#value-payload-and-reply-rule).

| Command | Required headers | Optional headers | Reply body |
| --- | --- | --- | --- |
| `GETDEL` | `CamelRedis.Key` | — | old value, binary-safe; null for a missing key |
| `GETEX` | `CamelRedis.Key` | `CamelRedis.Timeout`, `CamelRedis.Timestamp`, `CamelRedis.ExpireUnit`, `CamelRedis.Persist` | value, binary-safe; null for a missing key |
| `SETRANGE` | `CamelRedis.Key`, `CamelRedis.Offset` | — | new string length |
| `GETRANGE` | `CamelRedis.Key` | `CamelRedis.Start`, `CamelRedis.End` | requested slice, binary-safe; empty when the range selects no bytes |

`GETDEL` returns the value and then removes the key. `GETEX` applies its expiry options and leaves the key otherwise unchanged. Its options follow [SET and GETEX options](#set-and-getex-options).

`CamelRedis.Offset` is a required non-negative byte offset. Offset `0` overwrites the string from the start, so the header has no default. A write past the end of the string pads the gap with zero bytes.

`CamelRedis.Start` defaults to `0` and `CamelRedis.End` defaults to `-1`. A negative bound counts from the end of the string. A range with no overlap with the string returns an empty body.

`GETDEL` and `GETEX` are not retried after a transport error. `GETDEL` consumes the key, so a retry reports a null body. A retry of `GETEX` reports a null body when the value lapses between attempts. The first reply is not reproducible, so the component does not repeat the command. `SETRANGE` and `GETRANGE` are retried.

### Bitmap command inputs

| Command | Required headers | Optional headers | Reply body |
| --- | --- | --- | --- |
| `SETBIT` | `CamelRedis.Key`, `CamelRedis.BitOffset`, `CamelRedis.BitValue` | — | previous bit as a JSON number (`0` or `1`) |
| `GETBIT` | `CamelRedis.Key`, `CamelRedis.BitOffset` | — | current bit as a JSON number (`0` or `1`) |
| `BITCOUNT` | `CamelRedis.Key` | `CamelRedis.Start`, `CamelRedis.End`, `CamelRedis.BitRangeUnit` | number of set bits as a JSON number |
| `BITPOS` | `CamelRedis.Key`, `CamelRedis.BitValue` | `CamelRedis.Start`, `CamelRedis.End`, `CamelRedis.BitRangeUnit` | position of the first matching bit as a JSON number; `-1` when the bit is never set in the range |
| `BITOP` | `CamelRedis.BitOperation`, `CamelRedis.Destination`, `CamelRedis.Keys` | — | destination length in bytes as a JSON number |
| `BITFIELD` | `CamelRedis.Key`, `CamelRedis.BitFieldOps` | `CamelRedis.BitFieldOverflow` | JSON array of integers and nulls |
| `BITFIELD_RO` | `CamelRedis.Key`, `CamelRedis.BitFieldOps` | — | JSON array of integers and nulls |

`CamelRedis.BitOffset` is a required bit position in `0..=4294967295`, the 512 MB bitstring cap. An offset outside that range fails locally, before any command reaches Redis. `CamelRedis.BitValue` is required for `SETBIT` and `BITPOS` and accepts only `0` or `1`; the bit value is an argument header, not a body payload.

`CamelRedis.Start` and `CamelRedis.End` are signed (`i64`): a negative index counts from the end of the bitstring, matching Redis bounds. The range grammar follows Redis 7 and fails locally, before send, when violated:

- `BITCOUNT` requires `Start` and `End` together.
- `BITPOS` accepts `Start` alone. `End` always requires `Start`.
- `CamelRedis.BitRangeUnit` (`BYTE` or `BIT`) applies only to a complete range, so a unit without both endpoints is an error.

`CamelRedis.BitRangeUnit` defaults to `BYTE` when absent. That is the Redis default for the range unit, not a default bit value; it is documented so the unit in force is never an accident. For a complete range (both endpoints) the component always emits the unit token explicitly (`BITCOUNT k 0 2 BYTE`), so the wire frame states the unit it counted in. A start-only `BITPOS` range emits no unit token — `BITPOS key bit start` — because Redis 7 accepts a unit only after both endpoints. A present but malformed unit (`42`, `NIBBLE`) is a named `CamelRedis.BitRangeUnit` error rather than a silent fallback to `BYTE`.

`CamelRedis.BitOperation` is required for `BITOP` and accepts `AND`, `OR`, `XOR`, or `NOT`. `CamelRedis.Destination` names the key that receives the result. `CamelRedis.Keys` lists the source keys, as in `SINTERSTORE`. `NOT` requires exactly one source key. `AND`, `OR`, and `XOR` require one or more source keys. A missing or malformed operation, destination, or source list fails locally, before any command reaches Redis. The reply is the destination's new length in bytes.

`CamelRedis.BitFieldOps` lists `BITFIELD` sub-commands. Each list element is one entry: `GET <type> <offset>`, `SET <type> <offset> <value>`, `INCRBY <type> <offset> <increment>`, or `OVERFLOW <WRAP|SAT|FAIL>`. `<type>` is `[iu]<width>`. Signed widths are `1..=64` and unsigned widths are `1..=63`. `u64` is rejected because `i64` represents the signed 64-bit case. An absolute offset is a non-negative integer that fits `i64`. A `#<n>` offset addresses `n × <this entry's width>` bits from the start. The multiplication is independent for each entry, so two `GET u8 #1` entries read the same bits. An offset above `i64::MAX` or an `n × width` product that overflows fails locally, before any command reaches Redis.

Overflow behaviour is always explicit. Every `SET` or `INCRBY` entry needs an effective mode before it runs. The mode comes from `CamelRedis.BitFieldOverflow` (`WRAP`, `SAT`, or `FAIL`) or from an earlier `OVERFLOW` entry. A mutation with no mode in force fails locally with a `CamelRedis.BitFieldOverflow` error. There is no default. The server default `WRAP` would silently wrap counters. A trailing `OVERFLOW` entry does not cover a preceding mutation. `GET` entries need no mode. A present but malformed overflow header (`42`, `SOMETIMES`) is a named error even when an inline `OVERFLOW` entry exists.

`BITFIELD_RO` accepts only `GET` entries. It rejects `SET`, `INCRBY`, and `OVERFLOW` entries. It also rejects `CamelRedis.BitFieldOverflow`. The reply is a JSON array of numbers and nulls.

`GETBIT`, `BITCOUNT`, `BITPOS`, and `BITFIELD_RO` are retried after a transport error: they only read. `SETBIT`, `BITOP`, and `BITFIELD` are not retried: they write bits or the destination key, so the retry classification follows their worst-case form.

### HyperLogLog command inputs

| Command | Required headers | Optional headers | Reply body |
| --- | --- | --- | --- |
| `PFADD` | `CamelRedis.Key` | — | `1` when the structure changed and `0` otherwise, as a JSON number |
| `PFCOUNT` | `CamelRedis.Keys` | — | estimated cardinality as a JSON number |
| `PFMERGE` | `CamelRedis.Destination`, `CamelRedis.Keys` | — | `OK` as text |

`PFADD` takes its element from the Exchange body under the [value payload and reply rule](#value-payload-and-reply-rule). The element follows the shared body codec. A route that still sets `CamelRedis.Value` fails before send with the tombstone error.

`PFCOUNT` and `PFMERGE` take one or more source keys in `CamelRedis.Keys`. A single-key `PFCOUNT` is a one-element list. `PFMERGE` reads the sources and writes the merged HyperLogLog to `CamelRedis.Destination`.

A missing, empty, or malformed `CamelRedis.Keys` list fails before send with a named `CamelRedis.Keys` error. The list parser rejects any non-string entry, so the command never runs against fewer keys than the route configured ([ADR-0032](../adr/0032-exchange-data-trust-boundary.md)).

`PFCOUNT` is retried after a transport error: it only reads its sources. Its single-key form can refresh the cached cardinality inside the HyperLogLog, which is a converging internal write. `PFADD` and `PFMERGE` are not retried: they write the structure or the destination key.

### WAIT durability probe

| Command | Required headers | Optional headers | Reply body |
| --- | --- | --- | --- |
| `WAIT` | `CamelRedis.NumReplicas`, `CamelRedis.WaitTimeout` | — | number of acknowledged replicas as a JSON number |

`CamelRedis.NumReplicas` is the number of replicas to wait for. `CamelRedis.WaitTimeout` is the timeout in milliseconds. It is distinct from the TTL header `CamelRedis.Timeout`. Both headers accept a `u64`. Redis parses the WAIT arguments as signed 64-bit integers. A value above `i64::MAX` therefore fails before send with a named error. A missing header also fails before send. A zero `CamelRedis.WaitTimeout` fails too, because Redis reads `WAIT ... 0` as a wait with no limit.

The wire frame is `WAIT <numreplicas> <timeout>`. The integer reply becomes the Exchange body as a JSON number.

WAIT is a connection-local acknowledgement probe. It reports the writes that the invoking connection sent before the WAIT command. It is not a durability guarantee for an earlier route write. A different pooled connection does not acknowledge that write. A reconnect loses the write offset too. The component shares one multiplexed connection per endpoint ([camel-redis CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-redis/CONTEXT.md)).

The exchange has a bounded deadline. A WAIT-only aggregate deadline covers connection checkout, retries, backoff and replies. The deadline is `CamelRedis.WaitTimeout` plus the endpoint `timeout`. An unrepresentable deadline fails before send with a named error. On expiry the exchange fails with a named timeout error that names `CamelRedis.WaitTimeout`. The WAIT connection carries no driver response deadline, so the driver never aborts a valid probe that is still inside its server-side timeout.

WAIT is retried after a transport error. It changes no server state. A replay probes the current connection's offset again. The count can differ between attempts, like the count reply of `DEL` ([ADR-0013](../adr/0013-network-retry-policy-and-migration.md)).

### Key command inputs

| Command | Required headers | Optional headers | Reply body |
| --- | --- | --- | --- |
| `UNLINK` | `CamelRedis.Key` | — | integer count of unlinked keys |
| `COPY` | `CamelRedis.Key`, `CamelRedis.Destination` | `CamelRedis.Db`, `CamelRedis.Replace` | JSON boolean; `true` when the key is copied, `false` when the destination exists and `Replace` is unset |
| `TOUCH` | `CamelRedis.Keys` | — | integer count of keys that exist |

`UNLINK` removes one key, like `DEL`. `DEL` also accepts `CamelRedis.Keys`, but `UNLINK` reads only `CamelRedis.Key`.

`COPY` copies the key named by `CamelRedis.Key` to `CamelRedis.Destination`, which is required. `CamelRedis.Db` is optional: an absent value copies within the current database, and the component sends no `DB` argument. A `DB 0` default would retarget the copy. `CamelRedis.Replace` is a boolean that defaults to `false`. A present but malformed `Replace` value fails before send with an error that names the header.

`CamelRedis.Keys` is a non-empty JSON array of strings. An empty array, a non-array value, or a malformed entry fails before send, with an error that names the header and the entry index.

`COPY` is not retried after a transport error. Without `REPLACE`, a retry reports `false` after an unconfirmed success. With `REPLACE`, the retry re-reads the source, and the source can change between attempts. `UNLINK` and `TOUCH` are retried. A retry of `UNLINK` can report a count of `0` when the first attempt already removed the key. The state converges, so the component accepts the smaller count.

### List command inputs

`LMPOP` and `BLMPOP` pop from the first non-empty key. `LMOVE` and `BLMOVE` move one element between two lists. `LPOS` finds the index of an element in a list.

| Command | Required headers | Optional headers | Reply body |
| --- | --- | --- | --- |
| `LMPOP` | `CamelRedis.Keys`, `CamelRedis.Where` | `CamelRedis.Count` | `[key, [elements]]`; null when every key is empty |
| `BLMPOP` | `CamelRedis.Keys`, `CamelRedis.Where` | `CamelRedis.Count`, `CamelRedis.Timeout` | `[key, [elements]]`; null on a timeout |
| `LMOVE` | `CamelRedis.Key`, `CamelRedis.Destination`, `CamelRedis.WhereFrom`, `CamelRedis.WhereTo` | — | moved element, binary-safe; null when the source key is empty |
| `BLMOVE` | `CamelRedis.Key`, `CamelRedis.Destination`, `CamelRedis.WhereFrom`, `CamelRedis.WhereTo` | `CamelRedis.Timeout` | moved element, binary-safe; null on a timeout |
| `LPOS` | `CamelRedis.Key`, and the element in the body | `CamelRedis.Rank`, `CamelRedis.Count` | index, null when the element is absent, or an array of indexes when `Count` is present |

`CamelRedis.Keys` is a non-empty JSON array of strings. A malformed entry fails before send, with an error that names the header and the entry index.

`CamelRedis.Where` is required for `LMPOP` and `BLMPOP`, and it accepts `LEFT` or `RIGHT` in any case. The component refuses a missing value, because a default would hide a route typo. `CamelRedis.Count` must be positive on both commands. Without a count, the command pops one element.

`CamelRedis.WhereFrom` and `CamelRedis.WhereTo` are required for `LMOVE` and `BLMOVE`. Each one accepts `LEFT` or `RIGHT` in any case. `CamelRedis.Destination` names the target list.

`LPOS` reads the element from the Exchange body, under the [value payload and reply rule](#value-payload-and-reply-rule). `CamelRedis.Rank` defaults to `1`, and the component rejects `0`. A negative rank searches from the tail. `CamelRedis.Count` must be non-negative, and `0` returns every match.

`LMPOP`, `BLMPOP`, `LMOVE`, and `BLMOVE` are not retried after a transport error. A retry reports an empty pop or a different element, and it silently loses the element the first attempt moved. `LPOS` is a read and is retried.

When a key or an element is not valid UTF-8, the body is `Body::Bytes` with the RESP encoding of the reply, per the [`MGET` rule](#value-payload-and-reply-rule).

### Hash command inputs

| Command | Required headers | Optional headers | Reply body |
| --- | --- | --- | --- |
| `HRANDFIELD` | `CamelRedis.Key` | `CamelRedis.Count` | field name as text; null for a missing key. With `Count`: an array of field names, empty for a missing key |
| `HINCRBYFLOAT` | `CamelRedis.Key`, `CamelRedis.Field`, `CamelRedis.Increment` | — | new field value, as a decimal string |

`HRANDFIELD` returns fields at random. A negative `CamelRedis.Count` is valid Redis semantics: the reply may repeat a field.

`HINCRBYFLOAT` requires `CamelRedis.Increment`. It has no default, unlike `HINCRBY`, because a silent `1.0` would change the field value. A non-numeric increment fails before send.

`HINCRBYFLOAT` is not retried after a transport error. The reply holds cumulative state, so a retry reports a different value. `HRANDFIELD` is retried, and a retry can return a different field.

### Set command inputs

| Command | Required headers | Optional headers | Reply body |
| --- | --- | --- | --- |
| `SMISMEMBER` | `CamelRedis.Key`, `CamelRedis.Members` | — | JSON array of booleans, parallel to the requested members |
| `SINTERCARD` | `CamelRedis.Keys` | `CamelRedis.Limit` | integer cardinality of the intersection |

`CamelRedis.Members` is a non-empty JSON array of member strings. `SISMEMBER` reads a single member from the body instead.

`CamelRedis.Keys` is a non-empty JSON array of strings. `CamelRedis.Limit` must be non-negative, and `0` means no limit.

### Sorted-set command inputs

| Command | Required headers | Optional headers | Reply body |
| --- | --- | --- | --- |
| `ZMPOP` | `CamelRedis.Keys`, `CamelRedis.OrderBy` | `CamelRedis.Count` | `[key, [[member, score], ...]]`; null when every key is empty |
| `BZMPOP` | `CamelRedis.Keys`, `CamelRedis.OrderBy` | `CamelRedis.Count` (default `1`), `CamelRedis.Timeout` | `[key, [[member, score], ...]]`; null on a timeout |
| `ZRANDMEMBER` | `CamelRedis.Key` | `CamelRedis.Count` | member name as text; null for a missing key. With `Count`: an array of members, empty for a missing key |
| `ZDIFF` | `CamelRedis.Keys` | — | array of member names |
| `ZINTER` | `CamelRedis.Keys` | — | array of member names |
| `ZUNION` | `CamelRedis.Keys` | — | array of member names |
| `ZRANGEBYLEX` | `CamelRedis.Key`, `CamelRedis.Min`, `CamelRedis.Max` | — | array of member names |

`CamelRedis.OrderBy` is required for `ZMPOP` and `BZMPOP`, and it accepts `MIN` or `MAX` in any case. A sorted-set pop orders by score, not by list end, so the component refuses a missing value instead of picking an order.

`CamelRedis.Count` must be positive on `ZMPOP` and `BZMPOP`. `BZMPOP` sends `COUNT 1` when the header is absent. `ZMPOP` omits the count, and Redis also defaults it to `1`. On `ZRANDMEMBER`, a negative count is valid and may repeat a member.

`ZRANGEBYLEX` requires both bounds. Here `CamelRedis.Min` and `CamelRedis.Max` are lexicographic strings, not scores. They use the Redis lexicographic syntax: `[a` includes `a`, `(a` excludes it, `-` is the minimum, and `+` is the maximum.

`ZDIFF`, `ZINTER`, and `ZUNION` ship without `WEIGHTS` and `AGGREGATE`. `ZUNIONSTORE` and `ZINTERSTORE` also lack both options, and parity keeps the surface consistent (deferred per design.md D4). The bd follow-up tracks the two options.

When a key or a member is not valid UTF-8, the body is `Body::Bytes` with the RESP encoding of the reply, per the [`MGET` rule](#value-payload-and-reply-rule).

### Geospatial

`GEOADD` stores one member with its coordinates. The member comes from the
Exchange body. `GEOPOS` reads the stored positions of one or more members.
`GEODIST` measures the distance between two members. `GEOSEARCH` queries
members near a center point by radius or by bounding box. `GEOHASH` returns the
standard base-32 geohash of one or more members. The other arguments come from
headers. `CamelRedis.Member`, `CamelRedis.Member2`, and `CamelRedis.Members`
stay addressing operands on the read commands. `CamelRedis.Member` is not a
payload source on `GEOADD`, and a route that sets it there fails with a
tombstone error. The Producer overwrites the Exchange body with the JSON
result.

| Operation | Required inputs | Optional headers |
| --- | --- | --- |
| `GEOADD` | `CamelRedis.Key`, `CamelRedis.Longitude`, `CamelRedis.Latitude`, and the member in the body | — |
| `GEOPOS` | `CamelRedis.Key`, `CamelRedis.Members` | — |
| `GEODIST` | `CamelRedis.Key`, `CamelRedis.Member`, `CamelRedis.Member2` | `CamelRedis.Unit` |
| `GEOSEARCH` | `CamelRedis.Key`, `CamelRedis.Longitude`, `CamelRedis.Latitude`, and one shape | `CamelRedis.Unit`, `CamelRedis.WithDist`, `CamelRedis.WithCoord`, `CamelRedis.Count` |
| `GEOHASH` | `CamelRedis.Key`, `CamelRedis.Members` | — |

`CamelRedis.Members` is a JSON array of member strings. `CamelRedis.Longitude` must be in `[-180.0, 180.0]`. `CamelRedis.Latitude` must be in `[-90.0, 90.0]`. `CamelRedis.Unit` accepts `m`, `km`, `mi`, or `ft` and defaults to `m`. `CamelRedis.WithDist` and `CamelRedis.WithCoord` are booleans that default to `false`. `CamelRedis.Count` is a positive integer. A non-positive `Count` is ignored.

`GEOSEARCH` needs exactly one shape. `CamelRedis.Radius` selects `BYRADIUS`. `CamelRedis.Width` with `CamelRedis.Height` selects `BYBOX`. The radius and the box are mutually exclusive. A missing shape or a non-positive value fails before any command reaches Redis.

Result shapes follow the splitbody rule: plain string elements stay strings, and only structured rows become JSON objects.

| Operation | Body |
| --- | --- |
| `GEOADD` | Integer count of newly added members |
| `GEOPOS` | Array parallel to the requested members; each entry is `[longitude, latitude]` or `null` |
| `GEODIST` | Number in the requested unit, or `null` when a member is absent |
| `GEOSEARCH` without `WithDist`/`WithCoord` | Array of plain member strings |
| `GEOSEARCH` with `WithDist`/`WithCoord` | Array of objects `{member, distance?, longitude?, latitude?}` |
| `GEOHASH` | Array of geohash strings parallel to the requested members; `null` for an absent member |

The `member` field is a plain string. `distance` is a number in the requested unit. `longitude` and `latitude` are numbers rounded to six decimals. A structured row is a proper JSON object, never a quoted JSON string.

## Script commands

`command=eval` runs a Lua script through `EVALSHA`. `command=fcall` runs a registered Redis function through `FCALL`. Both read their keys and arguments from the Exchange body as one JSON object:

```text
{"keys": ["lock:orders"], "args": ["owner-1", 30]}
```

| Field | Type | Description |
| --- | --- | --- |
| `keys` | array of strings | Key names, passed positionally before the arguments |
| `args` | array of strings and numbers | Arguments, passed positionally after the keys |

Both fields are required. A body that is not a JSON object with both arrays fails before send. The error names the expected shape and the offending element. Numbers encode as decimal strings. This body form is the documented exception to header-sourced key addressing ([ADR-0098](../adr/0098-redis-value-contract.md)).

Replies map per the reply rule: nil to JSON `null`, integer to a JSON number, status and UTF-8 bulk string to text, non-UTF-8 bulk string to bytes, and nested arrays recursively.

### Script source and trust boundary

The source comes from the URI only: the inline `script` parameter or the `scriptFile` path, and exactly one of them. Script text in an Exchange header (`CamelRedis.Script`) is rejected before send with an error that names the header. Exchange data is untrusted, and executable text is an interpretable sink ([ADR-0032](../adr/0032-exchange-data-trust-boundary.md)).

A relative `scriptFile` path resolves against the route-file directory, the same rule the DSL applies to `source_file`. Without a route-file context (programmatic endpoint construction), a relative path resolves against the process working directory. Every failure names the resolved path.

### eval

`eval` sends `SCRIPT EXISTS` and `EVALSHA` in one `MULTI`/`EXEC` transaction, so the observation and the invocation cannot interleave with another client. When the script is absent and Redis answers `NOSCRIPT`, the component sends `SCRIPT LOAD` and invokes once more. A second `NOSCRIPT` is a named error. An error reply from a cached script is a business error: the script ran, so the component never re-invokes it.

### fcall

`function` names the invoked function and is required. `fcall` without `function` fails URI parsing with a named error. The library loads on first use with `FUNCTION LOAD REPLACE`; repeat exchanges reuse it. `REPLACE` is required because `FCALL` binds by function name: a pre-existing function with the same name would otherwise run instead of the configured code.

`fcall` sends `FUNCTION LIST` and `FCALL` in one transaction. When the list does not contain the function and Redis answers "function not found", the component reloads the library and invokes once more. A repeated missing function is a named terminal error. An error reply from a registered function is a business error: the function ran, so the component never re-invokes it.

### Retry

`eval` and `fcall` are non-idempotent by default. After a transport failure with unknown flush status, the component surfaces the error and does not replay the request, because a script can perform arbitrary side effects. `idempotent=true` declares the script replay-safe and allows replay under the configured retry budget.

The NOSCRIPT and function-not-found recovery is a separate bound. It permits one reload and one re-invocation per attempt, independent of the retry budget.

The three replay buckets and the connection-invalidation rule are described in [Retry semantics](#retry-semantics).

## Batch (multi)

`command=multi` runs several commands per Exchange as one pipeline. The batch
comes from the `CamelRedis.Batch` header, so the command list is per-exchange
route data while the payloads stay in the Exchange body (ADR-0098). Nothing is
sent until every entry and the body have been validated.

```text
redis://host:port?command=multi[&atomic=<true|false>]
```

| Parameter | Default | Description |
| --- | --- | --- |
| `atomic` | `true` | Wrap the batch in `MULTI`/`EXEC`; `false` sends a plain pipeline |

### Entry shape

`CamelRedis.Batch` is a JSON array of entry objects. Each entry names its
command in `command` and carries that command's operands as sibling fields:

```json
[
  {"command": "set", "key": "orders:1"},
  {"command": "incr", "key": "orders:count"},
  {"command": "getrange", "key": "orders:log", "start": 0, "end": -1}
]
```

Field names mirror the `CamelRedis.*` headers of the single-command path, and a
numeric field accepts a JSON number or a numeric string, exactly like a header.

The batch is aligned with the body: the body must be a `Body::Json` array with
exactly one element per entry, in entry order.

```json
["paid", null, null]
```

An entry that takes no value still needs its `null` placeholder. An entry that
takes a value (see the contract below) requires a non-null element; a string
element contributes its raw UTF-8 characters and any other scalar its compact
JSON form, per the [value payload rule](#value-payload-and-reply-rule). A JSON
object or array element is rejected before send: the v1 allowlist has no
multi-value command. A body that is not a JSON array (including an empty body),
or a body array whose length does not match the entry count, fails before send
with an error naming `CamelRedis.Batch` and the expected count.

### Typed allowlist

An entry's `command` must be one of the eighteen batchable commands. The
allowlist is exactly the set of commands with a batch encoder:

`GET`, `SET`, `GETDEL`, `GETEX`, `SETRANGE`, `GETRANGE`, `INCR`, `INCRBY`,
`DECR`, `DECRBY`, `APPEND`, `MGET`, `DEL`, `UNLINK`, `EXPIRE`, `PERSIST`,
`COPY`, `TOUCH`.

Every other command — the blocking pops, the consumer commands, Pub/Sub,
streams, and `eval`/`fcall` — is rejected at plan time with an error naming the
entry index, because a command without a batch encoder has no way to encode its
entry. An unknown command name, an unknown field, a wrong-typed field, a missing
required field, and a batch above the cap of 256 entries
(`MAX_BATCH_ENTRIES`) are rejected the same way. The whole batch is preflighted
before any command is sent, so a rejected batch cannot leave a partially applied
command list behind.

### Per-command field contract

| Command | Entry fields | Body slot |
| --- | --- | --- |
| `GET`, `GETDEL`, `INCR`, `DECR`, `PERSIST` | `key` (string) | `null` |
| `SET` | `key` (string) | value (required) |
| `GETEX` | `key` (string), optional `timeout` (positive integer, seconds) | `null` |
| `SETRANGE` | `key` (string), `offset` (non-negative integer) | value (required) |
| `GETRANGE` | `key` (string), `start`, `end` (integers) | `null` |
| `INCRBY`, `DECRBY` | `key` (string), `increment` (integer) | `null` |
| `APPEND` | `key` (string) | value (required) |
| `MGET`, `DEL`, `UNLINK`, `TOUCH` | `keys` (non-empty array of strings) | `null` |
| `EXPIRE` | `key` (string), `timeout` (positive integer, seconds) | `null` |
| `COPY` | `key`, `destination` (strings) | `null` |

`COPY` inside a batch copies within the current database and never replaces an
existing destination: `db` and `replace` are not expressible per entry.

### Reply

The body becomes a JSON array with one element per entry, in entry order.
Elements map by the reply rule: status to text (`"OK"`), integer to a number,
nil to `null`, and a non-UTF-8 bulk value to `{"bytes": "<base64>"}`.

### atomic

With `atomic=true`, the default, the entries run inside one `MULTI`/`EXEC`
transaction: no other client observes a state between the entries, and the reply
array is the `EXEC` result. A queue-time rejection makes Redis discard the whole
transaction (`EXECABORT`), so no entry runs.

With `atomic=false`, the entries run as one pipelined batch without a
transaction: they still travel as one write and one read, but another client can
interleave, and earlier entries stay applied when a later one fails. A
queue-time rejection does not exist on this form.

### Error precedence

A per-entry error reply is a business outcome, and the entries before the
failing one already executed. The batch is therefore never replayed wholesale:

- the first failing entry (lowest index) is surfaced as an error that names the
  entry index and the server text;
- that error is never transient, including a per-entry `LOADING`/`BUSY` reply:
  after a partial success those codes describe the rejected entry, not a
  whole-request rejection;
- invalidation evidence is independent of the surfaced entry: a `READONLY`
  reply from any entry evicts the demoted-master connection even when a
  different entry's error is the one reported;
- a pipeline-level driver failure is classified as usual: `EXECABORT` is a
  settled server verdict and is never replayed, while a connection failure
  after the batch was flushed is outcome-unknown. An `EXECABORT` aggregate that
  retains a queue-time `READONLY` reply also evicts the demoted-master
  connection, so the next exchange redials despite the terminal replay
  verdict.

### Idempotence aggregation

A batch is replay-safe only when every entry is. After an outcome-unknown
failure — a connection that died after the batch was flushed, so the atomic
`EXEC` may have committed or the plain pipeline may have partially executed —
the component replays the batch only when all its entries are idempotent.

| Idempotent entries | Non-idempotent entries |
| --- | --- |
| `GET`, `MGET`, `GETRANGE`, `DEL`, `UNLINK`, `TOUCH`, `EXPIRE`, `PERSIST`, `SETRANGE` | `SET`, `GETDEL`, `GETEX`, `INCR`, `INCRBY`, `DECR`, `DECRBY`, `APPEND`, `COPY` |

One non-idempotent entry makes the whole batch replay-unsafe. A failure that
proves the batch never ran — a checkout or dial failure before any byte was
written — is replayed whatever the entries are.

## Retry semantics

The Producer retries transport failures under the `NetworkRetryPolicy`
(`[components.redis.reconnect]` in `Camel.toml`). Each failed attempt is
classified into one of three buckets:

| Bucket | Meaning | Replayed |
| --- | --- | --- |
| `NotExecuted` | The request provably never ran: a checkout or dial failure before any byte was written. | Always, within the retry budget, whatever the command |
| `OutcomeUnknown` | The request was handed to the connection and may have run. | Only when the request is replay-safe: an idempotent command, a script with `idempotent=true`, or a batch whose entries are all idempotent |
| `NeverTransient` | A setup or auth defect, or a settled server verdict such as `WRONGTYPE` or `EXECABORT`. | Never; the error surfaces immediately |

Invalidation is a separate track. A failure that proves the connection dead — a
reset socket, a refused or dropped connection, a `READONLY` reply from a demoted
master — evicts the cached connection even when the request is not replayed, so
the next attempt, or the next exchange, redials and re-resolves the master
through the topology. A `READONLY` rejected at transaction queue time still
counts: redis-rs reports it inside one `EXECABORT` aggregate and keeps the
individual replies, so the component inspects them. A redial that fails is
itself a pre-send attempt and consumes the budget, so it cannot loop unbounded.

The per-command notes in this page state the bucket a command falls into. They
all come from one classifier, so an option-blind command takes its worst-case
form. See [ADR-0013](../adr/0013-network-retry-policy-and-migration.md) for the
policy and its migration boundaries.

## Producer

The Producer sends the command named by `command=`. A writing command takes its
payload from the Exchange body. The Producer holds a single multiplexed
connection per Endpoint. The connection opens lazily on the first call and
stays open for the route lifetime.

The `command` parameter picks one of the 120+ commands listed above. The
Exchange body, headers, and URI parameters supply the command arguments.
Different commands read different headers. For example, `HSET` reads
`CamelRedis.Key` and `CamelRedis.Field`, and takes its value from the Exchange
body. `LRANGE` reads `CamelRedis.Start` and `CamelRedis.End`. Missing required
headers return `CamelError`. A send failure returns `Err` to the route
`ErrorHandler`.

The `multi`, `eval`, and `fcall` endpoints keep their documented bounded
exceptions: entry commands come from the validated `CamelRedis.Batch` header,
and script keys and arguments come from the Exchange body. See
[Batch (multi)](#batch-multi) and [Script commands](#script-commands).

The Producer is a Tower `Service<Exchange>`. It composes with any pipeline step and reports per-route metrics.

### Value payload and reply rule

A payload command takes its single payload argument from the Exchange body
only. No value header exists. For an ordinary single-command endpoint, an
addressing operand, such as the key, the destination, the score, or the
timeout, never comes from the body
([ADR-0098](../adr/0098-redis-value-contract.md)).

Two endpoint families are bounded exceptions to that split. `command=multi`
takes each entry command name from the `CamelRedis.Batch` header, validated
against the typed allowlist, and each entry value from its index-aligned body
slot. `eval` and `fcall` bind their script keys and arguments from the body.
See [Batch (multi)](#batch-multi) and [Script commands](#script-commands).

The body codec is shared by every payload command.

| Body variant | Payload bytes |
| --- | --- |
| `Body::Text`, `Body::Xml` | UTF-8 bytes |
| `Body::Json` holding a string | the raw characters, with no JSON quoting |
| `Body::Json` holding any other value | compact JSON |
| `Body::Bytes` | the bytes verbatim |
| `Body::Empty`, `Body::Stream`, `Body::Json(null)` | rejected before send, with an error that names the command and the rejected form |

`HMSET` and `MSET` take a JSON-object body, or a `Body::Text`/UTF-8
`Body::Bytes` body that parses as a JSON object. Each entry value follows the
scalar rule, and a null entry fails before send. Binary map values are a
documented limit, because the map body is JSON or UTF-8 text.

The scalar payload commands are `SET`, `SETEX`, `SETNX`, `SETRANGE`, `GETSET`, `APPEND`,
`LPUSH`, `RPUSH`, `LPUSHX`, `RPUSHX`, `LINSERT`, `LSET`, `LREM`, `SADD`,
`SREM`, `SISMEMBER`, `SMOVE`, the `ZADD` member, `ZREM`, `ZRANK`, `ZREVRANK`,
`ZSCORE`, `ZINCRBY`, `HSET`, `HSETNX`, the `GEOADD` member, the `LPOS`
element, the `PFADD` element, and `PUBLISH`.

Three payload headers are removed: `CamelRedis.Value`, `CamelRedis.Values`,
and `CamelRedis.Member` as the `GEOADD` payload. A route that still sets one
fails before send with `"<header> was removed in 0.69; put the value in the
body"`. The [component README](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-redis/README.md)
lists the full tombstone scope and the migration steps.
`CamelRedis.Member`, `CamelRedis.Member2`, and `CamelRedis.Members` remain
addressing operands on `GEOPOS`, `GEODIST`, `GEOSEARCH`, and `GEOHASH`.

`GET` replies with `Body::Text` when the value is valid UTF-8, `Body::Bytes` when it is not, and `Body::Json(null)` for a missing key. `MGET` keeps the JSON array of strings and nulls when every element is nil or valid UTF-8. When any element is binary, `MGET` replies with `Body::Bytes` that holds the RESP encoding of the reply array, which `redis::parse_redis_value` decodes. Every other getter keeps its JSON reply.

### Command selection

The URI `command=` parameter is the only command source. A producer exchange that carries a `CamelRedis.Command` header, with any value, valid or not, fails with a named `ProcessorError` before the component connects. The header cannot select or change the command.

This is a breaking change from earlier releases, which accepted the header as a per-message command switch. A route that switched commands per message must use one endpoint per command. Branch with `choice`/`when` and pin one command in each producer URI:

```yaml
routes:
  - id: redis-command-by-message
    from: "direct:redis-op"
    steps:
      - choice:
          when:
            - simple: "${header.RedisOp} == 'increment'"
              steps:
                - to: "redis://127.0.0.1:6379?command=INCR"
            - simple: "${header.RedisOp} == 'read'"
              steps:
                - to: "redis://127.0.0.1:6379?command=GET"
          otherwise:
            - to: "log:warn?showHeaders=true"
```

The break reaches Rust library consumers too. `RedisCommand` gained the `Xadd` and `Xlen` variants, so a downstream exhaustive `match` no longer compiles until it handles both. `RedisEndpointConfig` gained the public `create_group: bool` and `group_start: Option<GroupStart>` fields, so a struct literal that names every field must set them. An endpoint built through `from_uri` fills both fields from the URI parser and needs no source change.

### Streams producer

`redis://host:port?command=XADD&key=events` appends one entry to a stream. `redis://host:port?command=XLEN&key=events` reads the entry count. Both take the stream key from the URI `key=`, falling back to the `CamelRedis.Key` header when the URI declares no key; the effective key is published on `CamelRedis.Key`. Migration from pre-0.x routes that relied on header-over-URI: omit `key=` from the producer URI to keep per-message key routing.

#### XADD

`XADD` appends one entry. The entry carries exactly one field named `body`. Its value is the Exchange body through the shared codec: text and XML contribute UTF-8 bytes, a JSON string contributes its raw characters, any other JSON value contributes compact JSON, and bytes pass verbatim. A JSON-object body is one field value, not one field per key. `Body::Empty`, `Body::Stream`, and `Body::Json(null)` fail with a named processor error before any command reaches Redis. The entry ID is always `*`, so Redis assigns it.

The Producer keeps the Exchange body unchanged. It reads one header and sets two:

| Header | Direction | Value |
| --- | --- | --- |
| `CamelRedis.Key` | input | Stream key; the URI `key=` wins when present (omit `key=` for per-message keys) |
| `CamelRedis.StreamId` | output | Server-assigned entry ID |
| `CamelRedis.Stream` | output | Resolved stream key |

`CamelRedis.StreamId` and `CamelRedis.Stream` overwrite any upstream value.

XADD is non-idempotent and the component never retries it. At-least-once delivery applies: when the command attempt fails during transport, the outcome is unknown, because the entry may or may not exist. The exchange then fails with an error that names the unknown outcome. A route redelivery can append the same body a second time. Mitigate duplicates with the Idempotent Consumer and `RedisIdempotentRepository`. `CamelRedis.StreamId` is not a deduplication key, because it exists only after a successful append.

#### XLEN

`XLEN` replaces the Exchange body with the entry count of the stream. It is a read, so it is idempotent and the component may retry it.

## Consumer

`redis://host:port?command=SUBSCRIBE&channels=foo,bar` subscribes to one or more Pub/Sub channels. The Consumer submits one Exchange per published message. The `CamelRedis.Channel` header carries the channel name. `CamelRedis.Pattern` carries the matched pattern for `PSUBSCRIBE`.

`redis://host:port?command=BLPOP&key=jobs&timeout=5` blocks on a list key and submits one Exchange per popped item. The `CamelRedis.Key` header carries the list key. The `timeout` parameter is the block duration in seconds, and `0` blocks indefinitely. Use `BLPOP` for left pop and `BRPOP` for right pop. A Producer `BLPOP`/`BRPOP` reads the timeout from the `CamelRedis.Timeout` header instead. See [Blocking pops](#blocking-pops).

The Consumer's mode comes from the URI command. `SUBSCRIBE` and `PSUBSCRIBE` use Pub/Sub mode. `BLPOP` and `BRPOP` use queue mode. `XREADGROUP` uses Streams mode. A command that fits none of these returns an error at consumer creation. `BLMPOP`, `BZMPOP`, and `BLMOVE` are Producer commands, not consumer modes. The component does not silently fall back to BLPOP (REDIS-003).

### Blocking pops and connection timeouts

Queue-mode consumers pop on a connection built with `AsyncConnectionConfig::set_response_timeout(None)` and `set_connection_timeout(None)`. The driver sets no response deadline, so the pop `timeout`, the transient retry budget, and route cancellation are the only bounds on a blocking pop. `connection_timeout_secs` remains the TCP connect bound. Producer endpoints whose command is `BLPOP`, `BRPOP`, `BLMPOP`, `BZMPOP`, or `BLMOVE` build the same deadline-free connections and ignore the `with_response_timeout` seam. Blocking pops are never multiplexed over a short driver timeout, and the component has no polling fallback (REDIS-003).

### Streams consumer groups

`redis://host:port?command=XREADGROUP&key=events&group=workers&consumer=worker-1` reads new entries from a consumer group with `XREADGROUP GROUP <group> <consumer> ... STREAMS <key> >`. `key`, `group`, and `consumer` are required; a missing or empty value fails at consumer creation. `count` bounds the entries returned per read and must be positive. `block` is the `BLOCK` duration in milliseconds and defaults to `10000`; `0` blocks indefinitely. The read uses a deadline-free connection, like a blocking pop (see [Blocking pops and connection timeouts](#blocking-pops-and-connection-timeouts)).

| Parameter | Required | Default | Description |
| --- | --- | --- | --- |
| `group` | Streams | none | Consumer-group name |
| `consumer` | Streams | none | Consumer name within the group |
| `count` | no | server default | Maximum entries returned per read; must be positive |
| `block` | no | `10000` | `BLOCK` duration in milliseconds; `0` blocks indefinitely |
| `createGroup` | no | `false` | Create the consumer group before the first read |
| `groupStart` | with `createGroup` | none | Group start position: `latest` (`$`) or `earliest` (`0`) |
| `streamRecoverEnabled` | no | `false` | Recover pending entries (opt-in) |
| `streamRecoverMinIdleMs` | no | `60000` | Minimum pending idle time before recovery, in ms (`1..=3600000`) |
| `streamRecoverBatch` | no | `10` | Maximum pending entries claimed per recovery pass (`1..=100`) |

The `streamRecover*` options are meaningful only for `XREADGROUP`; other commands ignore them. For `XREADGROUP`, an out-of-range or non-numeric value fails URI parsing and consumer creation with an error that names the option.

Group creation is opt-in. With `createGroup=false`, the default, the group stays operator-owned and a missing group keeps failing with `NOGROUP`. With `createGroup=true`, the session sends `XGROUP CREATE <key> <group> <start> MKSTREAM` once per session, on the read connection, before the first read. `groupStart=latest` maps to `$` and `groupStart=earliest` maps to `0`. Readiness is signalled only after the command succeeds. A `BUSYGROUP` reply is success with an `info!` log: the existing group wins and the component never sends `XGROUP SETID`. `groupStart` is required exactly when `createGroup=true`. `createGroup=true` without `groupStart`, or `groupStart` without `createGroup=true`, fails at URI parse time.

The Consumer delivers one Exchange per entry and waits for the route outcome through `send_and_wait`. It issues `XACK` only after the pipeline accepted the entry. A failed delivery logs at `warn!`, leaves the entry pending, and the Consumer continues with the next entry. Each Exchange carries these headers:

| Header | Value |
| --- | --- |
| `CamelRedis.Stream` | Stream key |
| `CamelRedis.StreamId` | Entry ID |
| `CamelRedis.StreamGroup` | Consumer-group name |
| `CamelRedis.StreamConsumer` | Consumer name |
| `CamelRedis.StreamDeliveryCount` | Deliveries the group has made for the entry, including this one |

`CamelRedis.StreamDeliveryCount` is the number of deliveries the consumer group has made for the entry including the current one. A first `>` delivery carries `1`. A recovered entry carries the post-claim count read from Redis after the claim, so redelivery pressure is visible.

The body is the raw field value for a single-field entry. For a multi-field entry the body is a JSON object that maps each field name to its value.

#### Pending-entry recovery

Recovery is opt-in with `streamRecoverEnabled=true`. When enabled, a recovery pass runs after the first successful connect and then at most once per `streamRecoverMinIdleMs`, measured from the start of the previous pass.

A pass runs on a dedicated recovery connection, separate from the read connection. It inspects the Pending Entries List and claims entries idle for at least `streamRecoverMinIdleMs`, in batches of at most `streamRecoverBatch` entries per pass. Claimed entries go through the same pipeline and `send_and_wait` delivery as `>` reads.

The Consumer acknowledges a recovered entry only after its delivery completes successfully. A failed delivery stays pending for a later pass to reclaim. Shutdown stops an in-progress pass promptly; entries already claimed but not delivered or acknowledged remain pending for a later pass or a later session.

## Security

The component supports TLS through the `rediss://` URI scheme or the `ssl=true` parameter. The two are equivalent.

TLS auto-enables for non-loopback hosts. This is the secure-by-default posture. The auto-enable check uses the `host` from the `[components.redis]` block. An endpoint that names its own host in the URI follows the global decision when it sets no `ssl` parameter. The component logs a `tracing::warn!` when auto-enabling TLS. Two opt-outs keep the connection plaintext:

- `?ssl=false` on the endpoint URI. The endpoint-level value wins over the global config.
- `tls_mode = false` in the `[components.redis]` block. This forces TLS off for every endpoint that does not set `ssl` itself. `tls_mode = true` forces TLS on for those endpoints, even for loopback hosts. When `tls_mode` is unset, auto-enable applies.

Sentinel endpoints select TLS by scheme (`redis-sentinel://` or `rediss-sentinel://`). The global TLS settings do not apply to them.

The `tls` boolean stays as a legacy force-on flag. Set `tls = true` to enable TLS on loopback hosts. The boolean cannot force TLS off: a plain `tls = false` keeps the auto-enable for remote hosts. Use `tls_mode = false` or `?ssl=false` for that.

A build without a TLS feature (`tls-rustls-webpki-roots`, `tls-rustls-native-certs`, or `tls-native-tls`) rejects every TLS endpoint at startup with a `Config` error. The error names the missing feature. The error is never classified as transient, so no retry loop runs. The `camel` release binary compiles the `redis-tls` feature by default, so remote Redis works without extra build steps.

The component redacts passwords in `Debug` output. Passwords with special characters (`@`, `:`, `/`) are percent-encoded in the connection URL. The `safe_endpoint()` helper returns a credential-free identifier for tracing.

An unset `password` and a blank (empty or whitespace-only) `password` both mean no `AUTH`. A blank value in the `[components.redis]` block, or in a `[components.redis.sentinel]` block, normalizes to unset, so `password = "${env:REDIS_PASSWORD:-}"` connects without `AUTH` when the variable is unset or empty. An explicit URI `password=` is taken literally and is not normalized.

### TLS CA trust

`tls_ca_cert` names a PEM file that holds the CA certificate. The field lives in the global `[components.redis]` block; it is not a URI parameter. `apply_defaults()` copies the global value onto each endpoint, and an endpoint-level value wins over the global one.

The component reads the file at endpoint creation for any endpoint that resolves to TLS, including standalone and Sentinel endpoints. Plaintext endpoints skip the read without filesystem access. Sentinel endpoints install the configured CA for TLS trust on both Sentinel and resolved Redis links.

The component trusts the PEM as the root for the connection. It passes the bundle to the CA-trusting client constructor, so the server certificate is verified against this root. There is no insecure bypass.

Redis TLS provides server authentication only in v1. Client-certificate authentication (mTLS) is intentionally unsupported; the client certificate slot remains unset. Revisit this decision when a deployment requires Redis `--tls-auth-clients yes`.

An unreadable file fails closed. Endpoint creation returns a `Config` error that names the path, before any connect attempt. One caveat: the error message contains the path, so a path that itself contains transient-classifier words (for example `readonly`) can be misclassified by the retry heuristic (bd rc-ezi0f).

## Connection handling

The Producer holds a single multiplexed connection per Endpoint. It opens lazily on the first call, and a reconnect drops the cached connection and re-resolves the master through the topology — that re-resolution is where a sentinel failover is picked up. The producer does not re-resolve on every command.

Each Consumer mode — Pub/Sub, queue, and Streams — holds ONE persistent read connection per session. Every message or popped item is delivered over that same connection; the consumer does not reconnect between messages. A Streams consumer with recovery enabled holds a second, dedicated recovery connection (see [Pending-entry recovery](#pending-entry-recovery)). A blocking-pop timeout (`BLPOP`/`BRPOP` returning nil) keeps the connection. The consumer reconnects only when the Pub/Sub stream ends or a transient transport error strikes, and a Pub/Sub reconnect replays all subscriptions. Reconnects are bounded by the configured `NetworkRetryPolicy`: when the budget is exhausted the consumer returns an error and Route supervision restarts the Route (ADR-0007).

Connections have a 10-second connect timeout by default (`connection_timeout_secs` in the config block). The route `ErrorHandler` owns the operational signal for non-transient errors.

The component registers an async health check that sends a `PING` command. The probe is healthy when Redis responds with `PONG` and degraded when PING fails or times out.

## Sentinel / failover

Redis Sentinel gives Redis high availability. Sentinel nodes monitor a master and its replicas. When the master fails, the sentinels elect a replica and promote it to master. Clients must re-discover the new master to keep working.

The component connects to a Sentinel topology with the `redis-sentinel://` scheme:

```text
redis-sentinel://sentinel-a:26379,sentinel-b:26379/<master-name>/<db>?command=<cmd>[&key=<key>][&channels=<list>]
```

The authority holds the comma-separated sentinel node list. The first path segment is the master group name. The second path segment is the optional database number. It defaults to `0`. The `rediss-sentinel://` scheme is the TLS variant. It enables TLS on the sentinel and the resolved master connections.

The runtime resolves a route URI by scheme. Register `RedisSentinelComponent` for `redis-sentinel://` routes and `RedissSentinelComponent` for `rediss-sentinel://` routes. Register them next to `RedisComponent`:

```rust,ignore
ctx.register_component(RedisComponent::new());
ctx.register_component(RedisSentinelComponent::new());
// Only for rediss-sentinel:// (TLS) routes:
ctx.register_component(RedissSentinelComponent::new());
```

`RedisBundle::register_all` registers all three schemes from the `[components.redis]` block.

You can also select Sentinel with the `[components.redis.sentinel]` config block:

```toml
[components.redis.sentinel]
nodes = ["redis://sentinel-a:26379", "redis://sentinel-b:26379"]
master_name = "mymaster"
# Optional sentinel credentials.
# username = "sentinel-user"
# password = "sentinel-pass"
```

`nodes` holds the sentinel node URLs. `master_name` is the master group name. The optional `username` and `password` authenticate the sentinel connections. The node password stays in the top-level `[components.redis]` block. The two credential sets are separate.

```rust,ignore
// Producer: timer writes a key every 3s through Sentinel.
let producer = RouteBuilder::from("timer:tick?period=3000&repeatCount=3")
    .route_id("redis-sentinel-producer")
    .set_header("CamelRedis.Key", Value::String("greeting".into()))
    .set_body("hello via redis sentinel!")
    .to("redis-sentinel://127.0.0.1:26379/mymaster/0?command=SET")
    .to("log:info?showHeaders=true")
    .build()?;
```

### Failover behavior

On a transport error, the producer and consumer reconnect loops re-resolve the current master through the sentinel nodes. The resolved master is never cached. Every reconnect asks the sentinels again. This is bounded transport reconnect, not consumer self-supervision. The retry budget comes from `NetworkRetryPolicy`. When the budget runs out, the consumer returns `Err` and Route supervision takes over ([ADR-0007](../adr/0007-route-supervised-consumer-failure.md)).

The health check also re-resolves the master through the sentinel nodes on each check. After a failover, the check reports the new master, not a cached address.

### Best-effort Pub/Sub

Pub/Sub delivery is best-effort. When a sentinel-triggered stream ends, the consumer re-subscribes to the new master. Messages published during the failover gap are lost. Duplicates are possible on reconnect. Do not use Pub/Sub for workloads that need durability.

### Feature flag

The `sentinel` cargo feature on `camel-component-redis` enables Sentinel topology construction. Enable it in your manifest:

```toml
camel-component-redis = { workspace = true, features = ["sentinel"] }
```

Without the feature, the component recognizes `redis-sentinel://` and a non-empty `[components.redis.sentinel]` block and rejects them at startup with a clear error. It fails closed. It does not fall back to a standalone connection.

## Error handling

The Consumer logs at `error!` for channel-closed conditions on Pub/Sub and BLPOP send paths and for retry-exhaustion. Each site reports a typed metric before the log line. The Producer logs send failures at `warn!`. Per-message non-transient Redis errors log at `error!` with a typed metric. The route handler owns the operational signal for transient producer errors.

**Reference**: [Redis crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-redis/CONTEXT.md). Example sources: [`examples/redis-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/redis-example), [`examples/redis-sentinel`](https://github.com/kennycallado/rust-camel/tree/main/examples/redis-sentinel).
