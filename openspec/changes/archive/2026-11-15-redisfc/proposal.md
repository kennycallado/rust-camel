# Proposal: redisfc

## Why

Producers can issue exactly one Redis command per exchange. There is no way to
run a Lua script (`EVAL`) or a Redis function (`FCALL`), and no way to send a
batch atomically (`MULTI`/`EXEC`). Separately, the retry classifier cannot tell
"never sent" from "may have executed": a dial failure and a dropped connection
after flush are both just "transient", so non-idempotent commands never retry
even when the command provably never reached the server. Callers who want
Redis as a lock/counter store must hand-roll compensation. This is Redis phase
2 (e_opus ranking 2026-11-13, epic `rc-pahez`).

## What Changes

In `crates/components/camel-redis`:

- `command=eval|fcall`: the script comes from URI params `script` or
  `scriptFile`, never from a header (ADR-0032). Keys/args come from the body
  per the body-only value contract (`rc-01ka4`). `EVALSHA` with a `NOSCRIPT`
  reload path via `redis::Script`. `scriptFile` resolves against the route-file
  directory the same way the DSL resolves `source_file` (`rc-bqyc6`).
- `command=multi`: a header list of typed `RedisCommand` entries (the typed
  enum is the allowlist), sent as `redis::pipe().atomic()`; `atomic=false`
  sends a plain pipeline. Per-entry values from the body.
- Retry classifier split into three buckets: not-executed (always retryable),
  outcome-unknown (retryable only when every command is idempotent), and
  server/semantic errors like `EXECABORT` (never transient).
  `EVAL`/`FCALL` are non-idempotent unless the endpoint opts in. The
  `invalidates_connection` ⊂ `is_transient_redis_error` invariant is preserved.
- ADR-0013 amendment (retry semantics: the three classes + the
  invalidation-vs-retryability distinction + the ADR-0032 script trust
  boundary).

Explicitly excluded: `SCAN` family (`rc-r3m6x`), bitmaps/HLL/WAIT
(`rc-nukx2`), cluster, `WATCH` (it would bind state to one connection while
all endpoints share one `MultiplexedConnection`).

## Acceptance criteria

- `eval`/`fcall` run scripts loaded from URI/file only; a script arriving in a
  header is rejected with a named error.
- `multi` runs an allowlisted batch atomically (or as a plain pipeline when
  `atomic=false`); unknown or non-batchable commands fail closed before send.
- Dial failures and `LOADING`/`BUSY` retry even for non-idempotent commands;
  post-flush I/O failures retry only when the command (or every batch entry)
  is idempotent; `EXECABORT` never retries.
- `invalidates_connection` stays a strict subset of the transient error
  class, independent of replay eligibility.
- Live-Redis integration suites pass; `cargo xtask schema --check` passes.

## Risk budget

Retry-semantics changes are behavior changes for existing routes: acceptable
only where the new behavior retries strictly more safely (not-executed cases).
No change to existing command dispatch, header surface, or the body-only value
contract. Untrusted input is bounded: batch entries count-capped, commands
allowlisted, script text never read from exchange data.

Bd: rc-grcgu
