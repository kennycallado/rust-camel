## ADDED Requirements

### Requirement: Script execution (eval and fcall)

The system SHALL run server-side scripts through `command=eval` (Lua via
`EVALSHA`) and `command=fcall` (Redis functions via `FCALL`). The script source
SHALL come only from the endpoint URI: parameter `script` (inline source) or
`scriptFile` (path), mutually exclusive; script text arriving in any exchange
header SHALL be rejected with a named error (ADR-0032: exchange data is
untrusted; executable text is an interpretable sink). `scriptFile` SHALL
resolve relative paths against the route-file directory the same way the DSL
resolves `source_file`, with no multi-directory guessing; when no route file
context exists (programmatic endpoint construction), relative paths SHALL
resolve against the process working directory (the documented fallback) and
all failures SHALL name the resolved path. Keys and args SHALL come from the
exchange body as a JSON object `{"keys": [<string>...], "args": [<string|number>...]}`;
this is the documented scoped exception to header-sourced key addressing
(see "Script operand binding"). `fcall` SHALL take the invoked function name
from endpoint parameter `function` (required). `EVALSHA` invocations SHALL
recover from `NOSCRIPT` by reloading the script once and re-invoking;
`NOSCRIPT` outside that path stays a non-transient business error. For
`fcall`, the function library SHALL be loaded lazily on first use with
`FUNCTION LOAD REPLACE` (repeat exchanges reuse the loaded library); a
"function not found" reply SHALL trigger at most one reload-and-reinvoke per
attempt. A failed reload, a still-failing re-invocation, or malformed body
operands SHALL surface named errors; these are terminal, never transient.
Missing `script`/`scriptFile`, both set at once, or `fcall` without
`function` SHALL fail with named errors before send. Replies SHALL map per the
body reply contract: nil to JSON null, integers to JSON numbers, status and
UTF-8 bulk strings to text, non-UTF-8 bulk strings to bytes, nested arrays
recursively.

#### Scenario: eval runs an inline script with keys and args from the body

- **GIVEN** an endpoint `command=eval` with `script` set to inline Lua and a
  body `{"keys": ["k1"], "args": ["v1"]}`
- **WHEN** the producer executes
- **THEN** the client invokes `EVALSHA` with those keys and args and the reply
  is returned as the exchange body per the reply mapping

#### Scenario: NOSCRIPT reloads once and re-invokes

- **GIVEN** a `command=eval` endpoint whose `EVALSHA` receives a `NOSCRIPT`
  reply (server restarted or script evicted)
- **WHEN** the script call fails
- **THEN** the client reloads the script and re-invokes once; the second
  result is the exchange result; a second `NOSCRIPT` surfaces a named error

#### Scenario: fcall loads its library and invokes the function

- **GIVEN** a `command=fcall` endpoint with `function=lock` and
  `scriptFile=functions/lock.lua` (a library defining `lock`)
- **WHEN** the producer executes its first exchange
- **THEN** the library is loaded with `FUNCTION LOAD REPLACE` and
  `FCALL lock <n> <keys> <args>` runs; subsequent exchanges reuse the loaded
  library without reloading

#### Scenario: fcall recovers from a missing function with one reload

- **GIVEN** a `command=fcall` endpoint whose `FCALL` returns "function not
  found" (server restarted or library evicted)
- **WHEN** the call fails
- **THEN** the client reloads the library and re-invokes once; a repeated
  failure surfaces a named error and is not retried

#### Scenario: script text in a header is rejected

- **GIVEN** any exchange carrying script text in a header (for example
  `CamelRedis.Script`) on an `eval`/`fcall` endpoint
- **WHEN** the producer resolves the request
- **THEN** it fails before send with a named error; the header content is
  never interpreted

#### Scenario: scriptFile resolves against the route-file directory

- **GIVEN** a route file in directory `D` with `scriptFile=scripts/lock.lua`
- **WHEN** the endpoint is created and executes
- **THEN** the script is read from `D/scripts/lock.lua`; a missing file fails
  with a named error naming the resolved path

#### Scenario: malformed body operands are a named error

- **GIVEN** a `command=eval` endpoint and a body that is not a JSON object
  with `keys` and `args` arrays (for example `{"keys": "k1"}`)
- **WHEN** the producer resolves the request
- **THEN** it fails before send with a named error naming the body shape

#### Scenario: eval and fcall are non-idempotent without opt-in

- **GIVEN** a `command=eval` endpoint (no `idempotent` opt-in) and an I/O
  failure after the request was flushed
- **WHEN** the retry classifier evaluates the outcome
- **THEN** the request is classified outcome-unknown and is NOT retried

#### Scenario: idempotent opt-in allows outcome-unknown retry

- **GIVEN** a `command=fcall` endpoint with `idempotent=true` and an I/O
  failure after flush
- **WHEN** the retry classifier evaluates the outcome
- **THEN** the request is retried under the retry budget

### Requirement: Atomic batch execution (multi)

The system SHALL execute a batch of commands from header `CamelRedis.Batch`
through `command=multi`. The header SHALL be a JSON array of entries with a
required `command` name that parses into the `RedisCommand` enum and belongs
to the batchable subset (single-shot producer commands with batch encoders;
blocking pops, consumer and pubsub commands are excluded), plus per-entry
arguments using the same names and semantics as the existing
`CamelRedis.*` header arguments (`key`/`keys`, `field`, `member`, `score`,
count/limit/range scalars). This typed allowlist SHALL fail closed: unknown
commands, non-batchable commands, unknown entry fields, wrong-typed
arguments, missing required arguments, malformed entries, or a count above
the documented cap SHALL produce named errors before send, naming the
offending index where applicable. Unlike single-command dispatch (where
`CamelRedis.Command` is rejected and the command is fixed by the URI),
entry-level `command` selection is permitted here because `command=multi` is
the operator opt-in and every entry is validated against the typed allowlist.
Per-entry values SHALL come from the exchange body as a JSON array aligned by
index; body element `i` is entry `i`'s value payload (scalar, array of
scalars for multi-value commands, or null for entries that take no body
value), and a length mismatch SHALL fail before send. With `atomic=true`
(default) the batch SHALL be sent as `MULTI`/`EXEC`
(`redis::pipe().atomic()`); with `atomic=false` as a plain pipeline. Error
precedence SHALL distinguish proven non-execution from uncertain outcomes:
`EXECABORT` and whole-request server rejections (`LOADING`, `BUSY`) mean no
entry executed; a connection failure before send means nothing executed; a
connection failure after entries were flushed (or with unknown flush status)
is outcome-unknown and follows the execution-outcome split — the batch is
replayed only when every entry is idempotent. An `EXEC` or pipeline reply
carrying per-entry errors means earlier entries DID execute and the batch
SHALL never be replayed wholesale — entry errors surface as named errors and
are never transient. `EXECABORT` SHALL be a non-transient error and SHALL
never be retried. A batch SHALL be classified idempotent iff every entry is
idempotent per `is_idempotent_command`.

#### Scenario: allowlisted batch runs atomically

- **GIVEN** `CamelRedis.Batch` listing `SET` and `INCR` entries with values in
  the body and `atomic=true`
- **WHEN** the producer executes
- **THEN** the commands are sent as one `MULTI`/`EXEC` transaction and the
  per-command replies are returned

#### Scenario: unknown batch command fails closed before send

- **GIVEN** `CamelRedis.Batch` containing an entry whose `command` does not
  parse into `RedisCommand`
- **WHEN** the producer resolves the batch
- **THEN** it fails before send with a named error naming the header and the
  entry index; nothing is sent

#### Scenario: non-batchable command is rejected

- **GIVEN** `CamelRedis.Batch` containing a consumer-only or blocking entry
  (for example `XREADGROUP` or `BLPOP`)
- **WHEN** the producer resolves the batch
- **THEN** it fails before send with a named error naming the index

#### Scenario: unknown entry field or bad argument type fails closed

- **GIVEN** `CamelRedis.Batch` containing an entry with an unknown field or a
  non-numeric `score`
- **WHEN** the producer resolves the batch
- **THEN** it fails before send with a named error naming the index and the
  field

#### Scenario: body length mismatch fails closed

- **GIVEN** `CamelRedis.Batch` with 3 entries and a body array of 2 elements
- **WHEN** the producer resolves the batch
- **THEN** it fails before send with a named error naming the expected and
  actual counts

#### Scenario: partial entry failure is never replayed

- **GIVEN** an `atomic=false` pipeline where entry `INCR` succeeded and a
  later entry returned a per-command error (for example `WRONGTYPE`)
- **WHEN** the reply is evaluated
- **THEN** the batch is NOT replayed (the `INCR` already took effect); the
  entry error surfaces as a named error and is classified never-transient

#### Scenario: atomic batch disconnect after flush is outcome-unknown

- **GIVEN** an `atomic=true` batch whose connection drops after the
  transaction was flushed but before the reply arrived
- **WHEN** the retry classifier evaluates the failure
- **THEN** it is outcome-unknown (the `EXEC` may have committed atomically):
  the batch is replayed only when every entry is idempotent

#### Scenario: plain pipeline disconnect after flush is outcome-unknown

- **GIVEN** an `atomic=false` pipeline whose connection drops after entries
  were flushed
- **WHEN** the retry classifier evaluates the failure
- **THEN** it is outcome-unknown (some entries may have executed): the
  pipeline is replayed only when every entry is idempotent

#### Scenario: non-idempotent batch is not retried after an unconfirmed success

- **GIVEN** a batch containing `INCR` (not idempotent) and an I/O failure
  after flush
- **WHEN** the retry classifier evaluates the batch
- **THEN** the batch is outcome-unknown and is NOT retried

#### Scenario: all-idempotent batch is retried after an unconfirmed success

- **GIVEN** a batch containing only idempotent entries (for example read-class
  commands) and an I/O failure after flush
- **WHEN** the retry classifier evaluates the batch
- **THEN** the batch is retried under the retry budget

#### Scenario: EXECABORT is never transient

- **GIVEN** a `multi` transaction whose `EXEC` returns `EXECABORT`
- **WHEN** the retry classifier evaluates the error
- **THEN** it is classified never-transient and is not retried

### Requirement: Execution-outcome retry split

The system SHALL classify every failed request attempt into exactly one of
three buckets and apply bucket-specific replay rules. Replay safety is the
conjunction of two different questions: is the error class transient (an
error classification), and was the request's effect confirmed absent or
idempotent (a replay decision). (1) Not-executed: the request provably never
ran — connection checkout/dial failure before send, or a whole-request server
reply of `LOADING`/`BUSY` (`BUSYKEY` excluded, it is semantic) — and SHALL
always be replayed under the retry budget, even for non-idempotent commands.
(2) Outcome-unknown: I/O failed after the request was flushed, or the flush
status is unknown (conservative) — and SHALL be replayed only when the
request is idempotent (single command per `is_idempotent_command`; batch iff
every entry is; `eval`/`fcall` only with the `idempotent` endpoint opt-in).
(3) Never-transient: `EXECABORT`, per-command `EXEC` errors, fatal
config/auth errors (for example `NOAUTH`, `WRONGPASS`, `NOPERM`), and all
other semantic/business errors — never replayed. The invariant
`invalidates_connection(e) ⇒ is_transient_redis_error(e)` SHALL be preserved
(error classification, independent of replay): `LOADING`/`BUSY` are
transient but never invalidate the connection; connection-lost errors are
transient and do invalidate; an outcome-unknown failure on a non-idempotent
request invalidates the connection but is never replayed. Not-executed
retries SHALL share the existing consecutive-failure budget. The split SHALL
be recorded in ADR-0013 together with the ADR-0032 trust boundary (scripts
from URI/file only).

#### Scenario: not-executed retries for a non-idempotent command

- **GIVEN** a `GETDEL` request (not idempotent) whose connection checkout
  fails before send
- **WHEN** the retry classifier evaluates the failure
- **THEN** it is classified not-executed and IS retried under the retry
  budget

#### Scenario: LOADING and BUSY are always retried and never invalidate

- **GIVEN** any request whose whole-request reply is `LOADING` (or `BUSY`)
- **WHEN** the retry classifier evaluates the reply
- **THEN** it is classified not-executed and retried; the connection is NOT
  invalidated

#### Scenario: outcome-unknown respects idempotence

- **GIVEN** a request whose I/O fails after flush (or with unknown flush
  status)
- **WHEN** the retry classifier evaluates the failure
- **THEN** it is classified outcome-unknown and replayed only if the
  request's idempotence (per command, per batch, or per `eval`/`fcall`
  opt-in) holds

#### Scenario: non-idempotent outcome-unknown invalidates but never replays

- **GIVEN** a `GETDEL` request whose connection drops after flush
- **WHEN** the retry classifier evaluates the failure
- **THEN** the connection is invalidated (it cannot be trusted) but the
  request is NOT replayed; the two decisions are independent

#### Scenario: invalidation remains a strict subset of transient errors

- **GIVEN** the error corpus of transport and server errors
- **WHEN** the subset property test evaluates `invalidates_connection(e)`
  against `is_transient_redis_error(e)`
- **THEN** every invalidating error is transient and at least one transient
  class (for example `LOADING`) is non-invalidating

## MODIFIED Requirements

### Requirement: Retry classification

The system SHALL classify retry safety per command in `is_idempotent_command`:
retry-safe iff a retry after an unconfirmed success converges server state and
does not lose data the first attempt reported. Count-style replies may differ
across attempts (`DEL`/`UNLINK` 1 then 0) and random-sample reads may return a
fresh draw (`SRANDMEMBER` precedent). The classifier is option-blind, so the
worst-case option form governs: `SET` classifies not idempotent because
`ReturnOldValue` and `NX` lose replay data, and the classifier cannot be
overridden per route (the producer always consults it); routes needing
plain-`SET` retry semantics use route-level redelivery, which re-executes the
whole exchange and may duplicate other side effects. Reads (`GETRANGE`,
`SMISMEMBER`, `SINTERCARD`, `ZDIFF`, `ZINTER`, `ZUNION`, `ZRANGEBYLEX`,
`ZRANDMEMBER`, `HRANDFIELD`, `LPOS`, `TOUCH`) and converging writes
(`SETRANGE`, `UNLINK`) are retry-safe. The pop/move family (`LMPOP`, `BLMPOP`,
`ZMPOP`, `BZMPOP`, `LMOVE`, `BLMOVE`), `SET`, `GETDEL`, `GETEX`, `COPY`, and
`HINCRBYFLOAT` are not. Batch requests SHALL be idempotent iff every entry is
idempotent. `EVAL`/`FCALL` SHALL classify not idempotent unless the endpoint
sets the `idempotent` opt-in. For single-command dispatch the command is fixed
by the endpoint URI and SHALL never be overridable per exchange (ADR-0032);
batch entry selection is permitted only through the validated typed allowlist
under `command=multi`.

#### Scenario: pop family is not retried after an unconfirmed success

- **GIVEN** a transport error after `LMPOP` (or `ZMPOP`/`LMOVE`/`BLMOVE`) was
  flushed
- **WHEN** the retry classifier evaluates the command
- **THEN** the command is classified not idempotent, so the driver does not
  re-issue it (a re-issued pop reports empty and silently loses the popped
  item)

#### Scenario: SET with option forms is not retried after an unconfirmed success

- **GIVEN** a transport error after `SET` (with `NX` or `ReturnOldValue`) was
  flushed
- **WHEN** the retry classifier evaluates the command
- **THEN** the command is classified not idempotent: the classifier cannot
  see options, and a retry would report the wrong old value or a false NX
  failure

#### Scenario: reads are retried safely

- **GIVEN** a transport error after `SMISMEMBER` (or any listed read) was
  flushed
- **WHEN** the retry classifier evaluates the command
- **THEN** the command is classified idempotent and may be retried

#### Scenario: batch idempotence aggregates entries

- **GIVEN** a `multi` batch mixing `GET` (idempotent) and `GETDEL` (not)
- **WHEN** the retry classifier evaluates the batch
- **THEN** the batch is classified not idempotent and is not retried after an
  unconfirmed success
