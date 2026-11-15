# Design: redisfc

## Approach

**eval|fcall.** New endpoint params: `script` (inline Lua/function-library
source) XOR `scriptFile` (path), `function` (required for `fcall`: the function
name to invoke), `idempotent` (default `false`; endpoint opt-in declaring the
script replay-safe). Wire contract: the body is a JSON object
`{"keys": [<string>...], "args": [<string|number>...]}`; keys and args pass
positionally to `EVALSHA`/`FCALL` (the documented scoped exception to
header-sourced keys, recorded in the redis-values delta); scalars encode as
bulk strings (UTF-8 text, non-UTF-8 preserved as bytes). Replies map per
`reply_to_body`: nil→JSON null, integer→JSON number, status/UTF-8 bulk→text,
non-UTF-8 bulk→bytes, nested arrays recursively. Nothing executable ever
comes from a header (ADR-0032). Malformed body shapes fail before send with
named errors. `eval` invokes through `redis::Script` (enable the `script`
feature of the `redis` crate): the client sends `EVALSHA`, and on a `NOSCRIPT`
reply reloads once and re-invokes; `NOSCRIPT` remains a non-transient business
error everywhere else. `fcall` lifecycle: the library loads lazily on first
use with `FUNCTION LOAD REPLACE` (repeat exchanges reuse it), invokes
`FCALL <function> <n> <keys…> <args…>`, and a "function not found" reply
triggers at most one reload-and-reinvoke per attempt; failed reloads and
repeat failures are terminal named errors. `scriptFile` resolution mirrors the
DSL `source_file` seam (`ScriptLoadContext::resolve` in
`crates/camel-dsl/src/yaml.rs`): relative paths resolve against the route-file
directory (absolute paths as-is), no multi-directory guessing; the DSL
pre-resolves the param at route load and the component reads the resolved
path. Without a route-file context (programmatic construction), relative paths
resolve against the process working directory — the documented
`ScriptLoadContext` fallback — and every failure names the resolved path.
Both-set/neither-set and missing `function` fail with named errors.

**multi.** New endpoint param `atomic` (default `true`). The batch comes from
header `CamelRedis.Batch`: a JSON array of entries
`{"command": "<name>", "key"|"keys": …, <scalar args matching existing
CamelRedis.* argument names: field, fields, member, score, count, limit,
start, end, …>}` where `<name>` must parse into the `RedisCommand` enum and
pass the batchable predicate (single-shot producer commands with batch
encoders; blocking pops, consumer and pubsub commands excluded) — the typed
enum plus predicate IS the allowlist; unknown commands, unknown fields,
wrong-typed or missing args fail closed before send with named errors naming
the index. Entry count is capped (named constant `MAX_BATCH_ENTRIES = 256`).
Per-entry VALUES come from the body as a JSON array aligned by index; element
`i` is entry `i`'s value payload (scalar, array of scalars for multi-value
commands, or null when the command takes no body value); a length mismatch
fails before send. Unlike single-command dispatch (URI fixes the command,
`CamelRedis.Command` is rejected), entry selection is permitted here: the
operator opt-in is choosing `command=multi`, and every entry is validated.
Send: `redis::pipe().atomic()` (`atomic=false`: plain pipeline). Error
precedence: proven non-execution (pre-send failure, `EXECABORT`,
whole-request `LOADING`/`BUSY` rejection) means no entry executed; a
connection failure after flush (or unknown flush status) is outcome-unknown —
an atomic `EXEC` may have committed, a plain pipeline may have partially
executed — and the batch is replayed only when every entry is idempotent;
`EXEC`/pipeline replies carrying per-entry errors mean earlier entries did
execute — the batch is never replayed wholesale, entry errors surface as
named errors, never transient. Batch encoding requires an
encode/send split: today's `commands::*` dispatchers encode AND send in one
step, so Phase 3 extracts pure `encode_* -> redis::Cmd` helpers (arg
resolution reused from the existing header helpers) that both the single
dispatch path and the batch path call.

**Classifier split.** `transport_error.rs` gains
`pub enum RetryClass { NotExecuted, OutcomeUnknown, NeverTransient }` and
`pub fn retry_class(err: &CamelError, phase: SendPhase) -> RetryClass` with
`SendPhase::{PreSend, PostFlush}`. Phase propagation: `execute_with_retry`
tags each attempt — checkout/dial failures are `PreSend` (today's executor
merges checkout and dispatch errors into one `CamelError`, so the retry loop
marks the phase itself at the call sites); anything returned after the
request is handed to the connection is `PostFlush`, and an unknown flush
status is conservatively `PostFlush` (outcome-unknown). Mapping: `PreSend`
failures → `NotExecuted`; whole-request server replies `LOADING`/`BUSY` →
`NotExecuted` (explicit non-execution; `BUSYKEY` stays semantic);
`EXECABORT`, per-command `EXEC` errors, and fatal config/auth errors
(`NOAUTH`, `WRONGPASS`, `NOPERM`) → `NeverTransient` regardless of phase;
any other post-flush I/O failure → `OutcomeUnknown`. Replay rule: retry
`NotExecuted` always (sharing the existing consecutive-failure budget),
retry `OutcomeUnknown` only when idempotent — per-command via
`is_idempotent_command`, batch via "every entry idempotent", `eval`/`fcall`
only with the `idempotent` opt-in. Classification and replay are independent
questions: the invariant `invalidates_connection(e) ⇒ is_transient_redis_error(e)`
is preserved over error classes (the corpus property test is extended, not
loosened) — `LOADING`/`BUSY` are transient but never invalidate;
connection-lost errors are transient and do invalidate; an outcome-unknown
failure on a non-idempotent request invalidates the connection yet is never
replayed.

## Affected crates

- `camel-component-redis`: config/params, producer, executor, commands
  (script + batch encode), `retry.rs`, `transport_error.rs`, `metadata.rs`
  (schema registration).
- `camel-test` (integration): live-Redis tests for eval/fcall, batch
  atomicity, classifier buckets.
- `docs`: `docs/src/components/redis.md` (commands, params, retry semantics).
- `docs/adr`: 0013 amendment.

## Architecture boundaries

Endpoint params and dispatch stay inside the component (Components plane);
`redis::Script`/pipelines are transport details behind the existing
`MultiplexedExecutor`. The only DSL touch is the `scriptFile` resolution,
reusing the established `source_file` seam at route-load time (control plane),
so runtime never reads exchange data as code. Data plane remains untrusted
(ADR-0032): batch commands are allowlisted types with a count bound; script
text is operator config only.

## Phases

### Phase 1: retry classifier split
- **Goal:** three-bucket classification wired into the retry loop; ADR-0013
  amendment.
- **Dependencies:** none (foundation for phases 2–3).
- **Externally-visible types/interfaces:** `RetryClass`, `SendPhase`,
  `retry_class()` in `transport_error.rs`.
- **Deliverable:** code + unit tests + ADR amendment.
- **Exit-criteria:** classifier bucket unit tests pass; subset property test
  passes; non-idempotent command retries on `NotExecuted` but not
  `OutcomeUnknown`.

### Phase 2: eval|fcall
- **Goal:** script execution with file/URI loading and NOSCRIPT reload.
- **Dependencies:** Phase 1 (`idempotent` opt-in feeds the classifier).
- **Externally-visible types/interfaces:** endpoint params
  `script|scriptFile|function|idempotent`; `RedisCommand::Eval|Fcall`.
- **Deliverable:** impl + tests + docs section + schema regen.
- **Exit-criteria:** fake-RESP tests cover NOSCRIPT reload and header
  rejection; live-Redis eval/fcall tests pass.

### Phase 3: multi batch
- **Goal:** allowlisted atomic batches with per-entry idempotence aggregation.
- **Dependencies:** Phase 1 (batch idempotence feeds the classifier).
- **Externally-visible types/interfaces:** endpoint param `atomic`; header
  `CamelRedis.Batch`.
- **Deliverable:** impl + tests + docs + schema regen.
- **Exit-criteria:** atomicity and allowlist tests pass; mixed-idempotence
  batch is not retried on `OutcomeUnknown`; all-idempotent batch is.

## Alternatives considered

- `WATCH`/`EXEC` optimistic transactions: rejected — every endpoint shares one
  `MultiplexedConnection`; WATCH state would leak across exchanges (stated in
  the ADR).
- Script text from a header (like `CamelRedis.Key`): rejected — ADR-0032
  marks headers untrusted; executable text is an interpretable sink.
- Raw `EVAL`/`EVALSHA` commands instead of `redis::Script`: rejected —
  hand-rolled SHA bookkeeping duplicates the crate's reload path.
- Keeping one `is_transient_redis_error(err)`-only classifier and folding the
  phase into error text: rejected — text sniffing is what the 7-rule
  precedence replaced; an explicit phase enum is testable.
