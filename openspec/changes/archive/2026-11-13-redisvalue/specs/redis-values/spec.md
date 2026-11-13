## ADDED Requirements

### Requirement: Value source precedence

The Redis producer SHALL bind each value payload of a value-writing command (SET, SETEX, SETNX, APPEND, LPUSH, RPUSH, LPUSHX, RPUSHX, LINSERT, LSET, HSET, HSETNX, SADD, the ZADD member, the GEOADD member, the PUBLISH message) from the command's value header (`CamelRedis.Value`, or `CamelRedis.Member` for GEOADD) when that header is present, and from the exchange body otherwise. No other value sources exist. The header path is encoded byte-identically to the current `value_to_redis_arg` behavior (JSON string → raw characters; any other JSON → compact JSON), so existing routes keep working unchanged.

#### Scenario: SET takes the exchange body when the value header is absent

- **GIVEN** a SET producer command with `CamelRedis.Key` set, no `CamelRedis.Value` header, and a `Body::Text("hello")` exchange body
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** the SET wire frame carries `hello` as the value argument and no missing-header error is raised

#### Scenario: The value header still wins over the body

- **GIVEN** a SET command with `CamelRedis.Value` = `"header-value"` and a `Body::Text("body-value")` exchange body
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** the SET wire frame carries `header-value` as the value argument (header precedence keeps existing routes byte-identical)

#### Scenario: A bytes body round-trips through SET and GET

- **GIVEN** a SET command with no value header and a `Body::Bytes` payload that is not valid UTF-8
- **WHEN** the SET frame is sent and the scripted server then answers a GET for the same key with those raw bytes
- **THEN** the SET frame carries the raw bytes verbatim and the GET reply body is `Body::Bytes` holding the identical payload

### Requirement: Body-to-value codec

The Redis producer SHALL convert the exchange body to value bytes through one shared codec used by every value-writing command: `Body::Text` to UTF-8 bytes, `Body::Json` to compact JSON bytes, `Body::Xml` to the XML text as UTF-8 bytes, `Body::Bytes` to raw bytes verbatim; `Body::Empty` and `Body::Stream` SHALL fail with a named processor error that names the command and the rejected body variant before any command frame is sent. XADD SHALL keep its one-`body`-field rule and use the same codec. HMSET resolves its field-to-value map from `CamelRedis.Values` when present (byte-identical to the current header encoding); when that header is absent the body SHALL be a JSON object whose entries are the field-to-value map, with each value encoded like a header value (JSON string → raw characters, other JSON → compact JSON); a body that does not parse as a JSON object fails with the same named pre-send error. MSET keeps the `CamelRedis.Values` map header only.

#### Scenario: Empty and Stream bodies fail before sending

- **GIVEN** a SET command with no value header and a `Body::Empty` or `Body::Stream` exchange body
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** processing fails with a `CamelError::ProcessorError` naming the command and the rejected body variant, and the server records no frames

#### Scenario: HMSET takes a JSON object body when the map header is absent

- **GIVEN** an HMSET command with no `CamelRedis.Values` header and a `Body::Json` object `{"field": "value"}`
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** the HMSET wire frame carries `field` and `value` (the string value unquoted) as the field-value arguments

#### Scenario: XADD accepts a bytes body

- **GIVEN** an XADD command with a `Body::Bytes` payload that is not valid UTF-8
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** the XADD wire frame carries the raw bytes as the `body` field value and `CamelRedis.StreamId` is set from the server reply

### Requirement: Binary-safe value replies

The Redis producer SHALL map a reply that replaces the body with a value payload through one shared rule: `Body::Text` when the reply bytes are valid UTF-8, `Body::Bytes` otherwise; a nil reply keeps `Body::Json(null)`. GET applies the rule directly; its reply shape changes from the current `Body::Json(string|null)` to `Body::Text`, `Body::Bytes`, or `Body::Json(null)`. MGET queries byte vectors: when every element is nil or valid UTF-8 the reply stays today's `Body::Json` array of strings and nulls; when any element is binary the reply is `Body::Bytes` holding a RESP encoding of the reply array (length-prefixed bulk strings, preserving element order and byte boundaries). Only GET and MGET are covered by this requirement; every other value-returning getter keeps its current `Body::Json` reply. Replies that are not value payloads (counts, booleans, ack nulls) stay `Body::Json`.

#### Scenario: GET replies are Text when UTF-8 and Bytes otherwise

- **GIVEN** a scripted fake RESP server that answers a first GET with a UTF-8 bulk string and a second GET with a non-UTF-8 bulk string
- **WHEN** two GET exchanges are processed
- **THEN** the first reply body is `Body::Text` with the decoded string and the second reply body is `Body::Bytes` with the raw bytes

#### Scenario: MGET keeps the JSON array for text replies

- **GIVEN** a scripted fake RESP server answering MGET with UTF-8 bulk strings and one nil
- **WHEN** the exchange is processed
- **THEN** the reply body is the same `Body::Json` array of strings and null that the command produced before this change

#### Scenario: MGET encodes binary elements losslessly

- **GIVEN** a scripted fake RESP server answering MGET with, in order, a UTF-8 bulk string, a nil, and a non-UTF-8 bulk string
- **WHEN** the exchange is processed
- **THEN** the reply body is `Body::Bytes` whose RESP encoding decodes to exactly three elements in server order with byte-exact contents and the nil in position two

GEOADD's member fallback MODIFIES the existing `redis-geo` requirement `GEOADD stores a member with coordinates`; that delta lives in `specs/redis-geo/spec.md` (targeting the capability it modifies).
