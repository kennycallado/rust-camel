# redis-values Specification

## Purpose

The Redis producer binds every value payload from the exchange body only. This
capability defines that contract: the shared body codec, the scalar, map,
stream-entry, and addressing-operand payload classes, and the tombstone error
for each removed payload header.
## Requirements
### Requirement: Value source precedence

<!-- openspec-scenario-ops
renamed: SET takes the exchange body when the value header is absent -> SET carries the body as the value
dropped: The value header still wins over the body | the value header is removed in 0.69 and fails with a tombstone error, so there is no precedence left to test
-->

The Redis producer SHALL bind the single scalar payload argument of every payload command from the exchange body only, through the shared body codec. The payload commands are `SET`, `SETEX`, `SETNX`, `GETSET`, `APPEND`, `LPUSH`, `RPUSH`, `LPUSHX`, `RPUSHX`, `LINSERT`, `LSET`, `LREM`, `SADD`, `SREM`, `SISMEMBER`, `SMOVE`, `ZADD` (member), `ZREM`, `ZRANK`, `ZREVRANK`, `ZSCORE`, `ZINCRBY`, `HSET`, `HSETNX`, the `GEOADD` member, and `PUBLISH` (message). No value header exists: `CamelRedis.Value` is removed, and `CamelRedis.Member` is removed as the GEOADD payload source (it remains an addressing operand on the other GEO commands). The body path is the only path, and it is binary-safe end to end: bytes stored through one command bind as the operand of its read counterpart. Addressing operands (key, destination, pivot, score, increment, timeout, index, count) never come from the body.

#### Scenario: SET carries the body as the value

- **GIVEN** a SET producer command with `CamelRedis.Key` set and a `Body::Text("hello")` exchange body
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** the SET wire frame carries `hello` as the value argument

#### Scenario: A bytes body round-trips through SET and GET

- **GIVEN** a SET command with a `Body::Bytes` payload that is not valid UTF-8
- **WHEN** the SET frame is sent and the scripted server then answers a GET for the same key with those raw bytes
- **THEN** the SET frame carries the raw bytes verbatim and the GET reply body is `Body::Bytes` holding the identical payload

#### Scenario: A bytes body round-trips through SADD and SISMEMBER

- **GIVEN** a SADD command with a `Body::Bytes` payload that is not valid UTF-8
- **WHEN** the SADD frame is sent and a following SISMEMBER for the same member carries the identical raw bytes in its body
- **THEN** both frames carry the bytes verbatim and SISMEMBER reports the member as present

#### Scenario: A bytes body round-trips through ZADD and ZSCORE

- **GIVEN** a ZADD command with score header `CamelRedis.Score` and a `Body::Bytes` member that is not valid UTF-8
- **WHEN** the ZADD frame is sent and a following ZSCORE for the same member carries the identical raw bytes in its body
- **THEN** both frames carry the member bytes verbatim and ZSCORE returns the stored score

#### Scenario: An addressing operand never comes from the body

- **GIVEN** an SMOVE command with `CamelRedis.Key`, `CamelRedis.Destination`, and a `Body::Text("member1")` body
- **WHEN** the SMOVE frame is built
- **THEN** the member argument comes from the body and the source key and destination come from their headers, with no body text appended as an operand

### Requirement: Body-to-value codec

<!-- openspec-scenario-ops
renamed: HMSET takes a JSON object body when the map header is absent -> HMSET takes a JSON object body
-->

The Redis producer SHALL convert the exchange body to payload bytes through one shared codec used by every payload command: `Body::Text` and `Body::Xml` to UTF-8 bytes; `Body::Json` holding a string to the string's raw characters (no JSON quoting); `Body::Json` holding any other value to compact JSON bytes; `Body::Bytes` to raw bytes verbatim. `Body::Empty`, `Body::Stream`, `Body::Json(null)`, and any future body variant SHALL fail with a named processor error that names the command and the rejected body form before any command frame is sent; a `Body::Json(null)` body is rejected so a GET miss is never stored as the literal bytes `null`. An empty `Body::Text("")` is a valid payload and SHALL be accepted. XADD SHALL keep its one-`body`-field rule and use the same codec: a JSON object body contributes one field whose value is the compact JSON of the object, never split into per-key fields. HMSET and MSET SHALL take a JSON object body, or a `Body::Text`/UTF-8 `Body::Bytes` body that parses as a JSON object; each entry value is encoded by the scalar rule; a body that does not parse as a JSON object fails with the same named pre-send error. Binary map values are a documented limit: the map body is JSON or UTF-8 text, so a binary value cannot travel in a map entry.

#### Scenario: Empty and Stream bodies fail before sending

- **GIVEN** a SET command with a `Body::Empty` or `Body::Stream` exchange body
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** processing fails with a `CamelError::ProcessorError` naming the command and the rejected body form, and the server records no frames

#### Scenario: A JSON null body is rejected before sending

- **GIVEN** a SET command with a `Body::Json(null)` exchange body, the shape a GET miss produces
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** processing fails with a `CamelError::ProcessorError` naming the command and the rejected body form, and the server records no frames

#### Scenario: A JSON string body contributes its raw characters

- **GIVEN** a SET command with a `Body::Json(Value::String("abc"))` exchange body
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** the SET wire frame carries the three bytes `abc` as the value argument, with no surrounding quote characters

#### Scenario: An empty string body is accepted and an empty body is not

- **GIVEN** three SET exchanges: `Body::Text("")`, `Body::Json(Value::String(""))`, and `Body::Empty`
- **WHEN** all three are processed against the fake RESP server
- **THEN** the first two succeed and their frames carry a zero-length value (no bytes, no quote characters), while the third fails with a named pre-send processor error

#### Scenario: HMSET takes a JSON object body

- **GIVEN** an HMSET command with a `Body::Json` object `{"field": "value"}`
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** the HMSET wire frame carries `field` and `value` (the string value unquoted) as the field-value arguments

#### Scenario: HMSET map values follow the scalar rule

- **GIVEN** an HMSET command with a `Body::Json` object whose entries are `{"n": 42, "s": "txt", "o": {"k": 1}}`
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** the HMSET frame carries `42` as the numeric text, `txt` unquoted, and the compact JSON `{"k":1}` for the object entry

#### Scenario: A map entry holding JSON null is rejected before sending

- **GIVEN** an HMSET command with a `Body::Json` object `{"bad": null}`, and an MSET command with the same body
- **WHEN** each exchange is processed against the fake RESP server
- **THEN** both fail with a `CamelError::ProcessorError` naming the command and the rejected null entry, and the server records no frames

#### Scenario: MSET takes a JSON object body

- **GIVEN** an MSET command with a `Body::Json` object `{"k1": "v1", "k2": "v2"}`
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** the MSET wire frame carries `k1`, `v1`, `k2`, `v2` in object order, each value unquoted

#### Scenario: XADD accepts a bytes body

- **GIVEN** an XADD command with a `Body::Bytes` payload that is not valid UTF-8
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** the XADD wire frame carries the raw bytes as the `body` field value and `CamelRedis.StreamId` is set from the server reply

#### Scenario: XADD does not split a JSON object into fields

- **GIVEN** an XADD command with a `Body::Json` object `{"sensor": "s1", "temp": 21}`
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** the XADD frame carries exactly one field named `body` whose value is the compact JSON `{"sensor":"s1","temp":21}`, and no field is named `sensor` or `temp`

#### Scenario: An XADD JSON string body contributes its raw characters

- **GIVEN** an XADD command with a `Body::Json(Value::String("abc"))` exchange body
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** the XADD frame carries exactly one field named `body` whose value is the three bytes `abc`, with no surrounding quote characters

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

### Requirement: Removed payload headers fail before sending

The Redis producer SHALL reject, with a named processor error before any command frame is sent, any exchange that carries a removed payload header on the exact command set that header used to feed. The tombstone scope is fixed by this matrix:

| Removed header | Tombstoned on |
| --- | --- |
| `CamelRedis.Value` | `SET`, `SETEX`, `SETNX`, `GETSET`, `APPEND`, `LPUSH`, `RPUSH`, `LPUSHX`, `RPUSHX`, `LINSERT`, `LSET`, `LREM`, `SADD`, `SREM`, `SISMEMBER`, `SMOVE`, `ZADD`, `ZREM`, `ZRANK`, `ZREVRANK`, `ZSCORE`, `ZINCRBY`, `HSET`, `HSETNX`, `PUBLISH` |
| `CamelRedis.Values` | `HMSET`, `MSET` |
| `CamelRedis.Member` | `GEOADD` |

The error SHALL name the removed header and state that the payload moved to the body (for example, `CamelRedis.Value was removed in 0.69; put the value in the body`). The error SHALL NOT fire anywhere else: `CamelRedis.Member`, `CamelRedis.Member2`, and `CamelRedis.Members` keep their addressing-operand roles on `GEOPOS`, `GEODIST`, `GEOSEARCH`, and `GEOHASH`, and `XADD` never carried these headers, so no tombstone applies to it.

#### Scenario: A CamelRedis.Value header fails before sending

- **GIVEN** a SET command with `CamelRedis.Value` set and a `Body::Text("hello")` body
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** processing fails with a `CamelError::ProcessorError` naming `CamelRedis.Value` and the move to the body, and the server records no frames

#### Scenario: A CamelRedis.Values header fails before sending on HMSET

- **GIVEN** an HMSET command with `CamelRedis.Values` set to a JSON object and a `Body::Empty` body
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** processing fails with a `CamelError::ProcessorError` naming `CamelRedis.Values` and the move to the body, and the server records no frames

#### Scenario: A CamelRedis.Values header fails before sending on MSET

- **GIVEN** an MSET command with `CamelRedis.Values` set to a JSON object and a `Body::Empty` body
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** processing fails with a `CamelError::ProcessorError` naming `CamelRedis.Values` and the move to the body, and the server records no frames

#### Scenario: A CamelRedis.Member header on GEOADD fails before sending

- **GIVEN** a GEOADD command with `CamelRedis.Member` set and the member also present in the body
- **WHEN** the exchange is processed against the fake RESP server
- **THEN** processing fails with a `CamelError::ProcessorError` naming `CamelRedis.Member` and the move to the body, and the server records no frames

#### Scenario: Member headers stay addressing operands outside GEOADD

- **GIVEN** a GEODIST command with `CamelRedis.Member` and `CamelRedis.Member2` set
- **WHEN** the exchange is processed
- **THEN** no tombstone error fires and the command resolves its two members from the headers

