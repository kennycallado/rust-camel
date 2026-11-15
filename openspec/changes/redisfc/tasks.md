# Tasks: redisfc

## New symbols introduced by this change

Registry for cross-task name/signature consistency:

- `transport_error.rs`: `pub enum SendPhase { PreSend, PostFlush }`;
  `pub enum RetryClass { NotExecuted, OutcomeUnknown, NeverTransient }`;
  `pub fn retry_class(err: &CamelError, phase: SendPhase) -> RetryClass`
  (plain requests);
  `pub enum RequestKind { Plain, Script }`;
  `pub fn retry_class_script(err: &CamelError, phase: SendPhase) -> RetryClass`
  (script requests — conservative, Task 2.1);
  `NOT_EXECUTED_SERVER_CODES`, `NEVER_TRANSIENT_SERVER_CODES` (consts).
- `executor.rs`: `pub struct AttemptFailure { pub err: CamelError, pub phase: SendPhase }`;
  `pub struct CommandContext { pub script: Option<Arc<ScriptRuntime>>, pub script_idempotent: bool, pub atomic: bool, pub batch: Option<BatchPlan> }`.
- `script.rs`: `pub struct ScriptSource { pub code: String, pub resolved_path: Option<PathBuf> }`;
  `pub fn load_script_source(cfg: &RedisEndpointConfig) -> Result<ScriptSource, CamelError>`;
  `pub enum ScriptKind { Eval, Fcall { function: String } }`;
  `pub struct ScriptRuntime { pub source: ScriptSource, pub kind: ScriptKind, init: tokio::sync::OnceCell<()> }`
  — `init` runs the first-use `FUNCTION LOAD REPLACE` for `Fcall` (name
  binding) exactly once per runtime, SHARED across producer clones
  (`Arc<ScriptRuntime>`, `get_or_try_init` semantics: concurrent first use
  performs exactly one load); `Eval` needs no first-use load (the SHA binds
  the code). Fcall library recovery handling is inside `ScriptRuntime` (no
  separate type); the atomic provenance pair observes presence per
  invocation.
- `commands/mod.rs`: `pub fn script_operands(exchange: &Exchange) -> Result<(Vec<String>, Vec<String>), CamelError>`;
  `pub fn redis_value_to_body(value: &redis::Value) -> Result<Body, CamelError>`
  (fallible full-value reply seam: `ServerError` replies and nested
  `ServerError` elements surface `CamelError`; exact mapping in Task 2.1
  step 6).
- `commands/batch.rs`: `pub struct BatchEntry { pub command: RedisCommand, pub fields: serde_json::Map<String, serde_json::Value> }`;
  `pub fn plan_batch(exchange: &Exchange) -> Result<BatchPlan, CamelError>`;
  `pub struct BatchPlan { pub entries: Vec<BatchEntry>, pub idempotent: bool }`;
  `pub fn batchable(cmd: &RedisCommand) -> bool`;
  `pub fn allowed_entry_fields(cmd: &RedisCommand) -> &'static [FieldSpec]`
  where `pub struct FieldSpec { pub name: &'static str, pub required: bool, pub kind: FieldKind }`
  and `pub enum FieldKind { Str, Int, PositiveInt, UInt, Strs }`;
  `pub fn encode_entry(entry: &BatchEntry, value: Option<&serde_json::Value>) -> Result<redis::Cmd, CamelError>`;
  `pub const MAX_BATCH_ENTRIES: usize = 256`.
- `config.rs`: `RedisCommand::Eval | Fcall | Multi` (each introduced in the
  same task as its dispatch arm — exhaustiveness-safe);
  `RedisEndpointConfig` fields `script: Option<String>`,
  `script_file: Option<PathBuf>`, `function: Option<String>`,
  `idempotent: bool`, `atomic: bool`.
- `commands/string.rs`, `commands/key.rs`: extracted
  `encode_<cmd>(...) -> Result<redis::Cmd, CamelError>` helpers for the
  batchable commands (field contract per Task 3.1; existing single-command
  options and validation preserved).

## Spec coverage map

| Spec requirement / scenario | Task | Test |
|---|---|---|
| redis-commands / Script: eval inline body keys+args | 2.1 | `eval_inline_script_runs_with_body_keys_and_args` |
| redis-commands / Script: NOSCRIPT reloads once | 2.1 | `eval_noscript_reloads_once_and_reinvokes`, `eval_second_noscript_is_named_error` |
| redis-commands / Script: fcall loads library | 2.3 | `fcall_loads_library_and_invokes`, `fcall_first_use_replaces_preexisting_function` |
| redis-commands / Script: fcall one bounded reload | 2.3 | `fcall_missing_function_reloads_once` |
| redis-commands / Script: header script rejected | 2.1, 2.3 | `eval_rejects_script_header`, `fcall_rejects_script_header` |
| redis-commands / Script: scriptFile route-dir resolution | 2.2 | `scriptfile_resolves_against_route_file_dir`, `scriptfile_missing_file_names_resolved_path` |
| redis-commands / Script: malformed body operands | 2.1 | `eval_malformed_body_operands_named_error` |
| redis-commands / Script: eval non-idempotent default | 2.3 | `eval_non_idempotent_outcome_unknown_not_retried` |
| redis-commands / Script: idempotent opt-in retry | 2.3 | `eval_idempotent_optin_outcome_unknown_retried`, `fcall_idempotent_optin_outcome_unknown_retried` |
| redis-commands / Split: script reply provenance (faked codes never replay) | 2.1, 2.3 | `eval_provenance_pair_is_atomic`, `retry_class_script_business_errors_are_never_transient`, `eval_faked_busy_reply_not_replayed_without_optin`, `eval_forged_noscript_not_reinvoked_without_optin`, `fcall_faked_loading_reply_not_replayed_without_optin`, `fcall_forged_function_not_found_not_reinvoked_without_optin` |
| redis-commands / Batch: allowlisted batch atomic | 3.2 | `batch_atomic_runs_as_multi_exec` |
| redis-commands / Batch: unknown command fails closed | 3.1 | `batch_unknown_command_fails_closed_with_index` |
| redis-commands / Batch: non-batchable rejected | 3.1 | `batch_non_batchable_command_rejected_with_index` |
| redis-commands / Batch: unknown field / bad type | 3.1 | `batch_unknown_field_or_bad_type_fails_with_index` |
| redis-commands / Batch: body length mismatch | 3.1 | `batch_body_length_mismatch_fails_named_error` |
| redis-commands / Batch: count cap | 3.1 | `batch_entry_count_cap_fails_named_error` |
| redis-commands / Batch: partial entry failure never replayed | 3.2 | `batch_partial_entry_failure_not_replayed`, `batch_partial_success_with_loading_entry_is_never_replayed` |
| redis-commands / Batch: atomic disconnect outcome-unknown | 3.3 | `batch_atomic_disconnect_after_flush_is_outcome_unknown` |
| redis-commands / Batch: plain disconnect outcome-unknown | 3.3 | `batch_plain_disconnect_after_flush_is_outcome_unknown` |
| redis-commands / Batch: non-idempotent batch not retried | 3.3 | `batch_post_flush_drop_non_idempotent_not_retried` |
| redis-commands / Batch: all-idempotent batch retried | 3.3 | `batch_post_flush_drop_all_idempotent_retried` |
| redis-commands / Batch: EXECABORT never transient | 3.2 | `batch_execabort_is_never_transient` |
| redis-commands / Split: not-executed retries non-idempotent | 1.2 | `not_executed_failure_retries_non_idempotent_command`, `batch_pre_send_failure_retries_non_idempotent_batch` (3.3) |
| redis-commands / Split: LOADING/BUSY retried, never invalidate | 1.1, 1.2 | `retry_class_loading_and_busy_are_not_executed`, `loading_reply_retries_without_invalidating` |
| redis-commands / Split: outcome-unknown respects idempotence | 1.2 | `post_flush_drop_replays_idempotent_command`, `post_flush_drop_does_not_replay_non_idempotent_command` |
| redis-commands / Split: invalidates-but-never-replays | 1.2 | `post_flush_drop_does_not_replay_non_idempotent_command` (asserts invalidation) |
| redis-commands / Split: subset property | 1.1 | `invalidating_errors_remain_transient_subset` |
| redis-commands / Split: never-transient semantics (BUSYKEY, WRONGTYPE, auth, config) | 1.1 | `retry_class_busykey_is_semantic`, `retry_class_wrongtype_is_never_transient`, `retry_class_pre_send_auth_failure_is_never_transient`, `retry_class_native_auth_failure_is_never_transient`, `retry_class_config_errors_are_never_transient` |
| redis-commands / Split: READONLY conservative exception | 1.1 | `retry_class_readonly_stays_transient_conservative`, `readonly_idempotent_replay_preserved` |
| redis-commands / Split: budget covers failed redials | 1.2 | `retry_budget_covers_failed_redials` |
| redis-commands / Retry classification (MODIFIED): pop family | 1.2 | `is_idempotent_command_corpus_unchanged` (asserts LMPOP false) |
| redis-commands / Retry classification (MODIFIED): SET option forms | 1.2 | `is_idempotent_command_corpus_unchanged` (asserts SET false) |
| redis-commands / Retry classification (MODIFIED): reads | 1.2 | `is_idempotent_command_corpus_unchanged` (asserts SMISMEMBER true) |
| redis-commands / Retry classification (MODIFIED): batch aggregates | 3.2 | `batch_idempotence_aggregates_entries` |
| redis-values / Script operand binding | 2.1 | `script_keys_and_args_bind_from_body`, `non_script_commands_keep_header_sourced_keys` |
| Live-Redis coverage (mission deliverable) | 2.3, 3.3 | `live_eval_script_roundtrip`, `live_fcall_function_roundtrip`, `live_fcall_missing_function_recovery`, `live_multi_atomic_batch_roundtrip`, `live_multi_partial_entry_error_is_named` |

## Phase 1: retry classifier split

### Task 1.1: Add RetryClass three-bucket classification to transport_error.rs

**Files:**
- `crates/components/camel-redis/src/transport_error.rs` (modified)

**Steps:**
1. In `transport_error.rs`, add `pub enum SendPhase { PreSend, PostFlush }` and
   `pub enum RetryClass { NotExecuted, OutcomeUnknown, NeverTransient }`.
2. Add `pub fn retry_class(err: &CamelError, phase: SendPhase) -> RetryClass`
   with STRICT PRECEDENCE ORDER:
   a. If the error is a config/validation error (`CamelError::Config` /
      `ConfigValidation`) or wraps a native redis auth failure
      (`redis::ErrorKind::AuthenticationFailed` — it carries NO error code
      string, so match on the error kind/source chain) → `NeverTransient`,
      REGARDLESS of phase.
   b. If the error carries a Redis error code in
      `NOT_EXECUTED_SERVER_CODES = ["LOADING", "BUSY"]` (first word of the
      error string, exact code match, never substring; `BUSYKEY` does NOT
      match) → `NotExecuted`, regardless of phase.
   c. Else if `is_transient_redis_error(err)` is true (the EXISTING
      classifier — transport errors plus the structural transient set that
      includes `READONLY` via `legacy_substring_matches`): keep its
      conservative behavior — `phase == PreSend` → `NotExecuted`,
      `phase == PostFlush` → `OutcomeUnknown`. `READONLY` MUST keep its
      current transient+invalidating semantics (structural: replica
      redirection) and must NOT fall into the semantic rejection below.
   d. Else (non-transient semantic/business errors) → `NeverTransient`
      regardless of phase: `EXECABORT`, per-command `EXEC` errors,
      `NOAUTH`/`WRONGPASS`/`NOPERM` codes, and other server business errors
      (`WRONGTYPE`, `BUSYKEY`, `NOSCRIPT`, `ERR ...`) — matched via
      `NEVER_TRANSIENT_SERVER_CODES = ["EXECABORT", "EXEC", "NOAUTH",
      "WRONGPASS", "NOPERM"]` plus the existing non-transient business-error
      handling (reuse the code-matching style of `SETTLED_FATAL_SERVER_CODES`
      at `transport_error.rs:189`).
3. Keep `is_transient_redis_error` and `invalidates_connection` signatures
   unchanged. Module doc must state the invariant
   `invalidates_connection(e) ⇒ is_transient_redis_error(e)` as an
   error-class property, independent of replay (replay additionally needs
   the bucket plus idempotence). `LOADING`/`BUSY` join the transient error
   class (retryable) but NOT `invalidates_connection`; `READONLY` keeps both
   (unchanged).

**Tests:**
- `retry_class_pre_send_failure_is_not_executed`: setup: `CamelError::Io`
  dial failure → action: `retry_class(err, SendPhase::PreSend)` → assert:
  `RetryClass::NotExecuted`.
- `retry_class_loading_and_busy_are_not_executed`: setup: server errors with
  codes `LOADING` and `BUSY` → action: `retry_class(err,
  SendPhase::PostFlush)` → assert: both `RetryClass::NotExecuted`.
- `retry_class_busykey_is_semantic`: setup: server error code `BUSYKEY` →
  action: `retry_class(err, SendPhase::PostFlush)` → assert:
  `RetryClass::NeverTransient`.
- `retry_class_wrongtype_is_never_transient`: setup: server error
  `WRONGTYPE Operation against a key` → action: `retry_class(err,
  SendPhase::PreSend)` AND `retry_class(err, SendPhase::PostFlush)` →
  assert: both `RetryClass::NeverTransient`.
- `retry_class_pre_send_auth_failure_is_never_transient`: setup: server error
  `NOAUTH Authentication required` at checkout → action: `retry_class(err,
  SendPhase::PreSend)` → assert: `RetryClass::NeverTransient`.
- `retry_class_native_auth_failure_is_never_transient`: setup: a
  `CamelError` wrapping `redis::ErrorKind::AuthenticationFailed` with NO
  error code (the source-preserving checkout error shape) → action:
  `retry_class(err, SendPhase::PreSend)` and `retry_class(err,
  SendPhase::PostFlush)` → assert: both `RetryClass::NeverTransient`.
- `retry_class_config_errors_are_never_transient`: setup: a
  `CamelError::Config` / `CamelError::ConfigValidation` error → action:
  `retry_class(err, SendPhase::PreSend)` → assert:
  `RetryClass::NeverTransient`.
- `retry_class_execabort_and_exec_errors_are_never_transient`: setup: server
  errors `EXECABORT Transaction discarded` and `EXEC ...` → action:
  `retry_class(err, SendPhase::PostFlush)` → assert: both
  `RetryClass::NeverTransient`.
- `retry_class_post_flush_io_is_outcome_unknown`: setup: `CamelError::Io`
  connection reset → action: `retry_class(err, SendPhase::PostFlush)` →
  assert: `RetryClass::OutcomeUnknown`.
- `retry_class_readonly_stays_transient_conservative`: setup: server error
  `READONLY You can't write against a read only replica` → action:
  `retry_class(err, SendPhase::PreSend)` and `retry_class(err,
  SendPhase::PostFlush)` and `invalidates_connection(err)` → assert:
  `NotExecuted`, `OutcomeUnknown`, and `true` respectively (unchanged
  conservative behavior).
- `readonly_idempotent_replay_preserved`: setup: the existing classifier
  corpus → action: `is_transient_redis_error` on the READONLY error →
  assert: `true` (regression pin).
- `invalidating_errors_remain_transient_subset`: setup: the corpus used by
  `subset_property_holds_over_corpus` extended with `LOADING`, `BUSY`,
  `EXECABORT`, `NOAUTH`, `READONLY` → action: evaluate
  `invalidates_connection(e) ⇒ is_transient_redis_error(e)` for every entry
  → assert: holds for all; `LOADING`/`BUSY` are transient with
  `invalidates_connection == false`.
- Command: `cargo test -p camel-component-redis retry_class`
  then `cargo test -p camel-component-redis invalidating_errors`
  then `cargo test -p camel-component-redis readonly`
- Expected: fail before implementation (symbols missing), pass after.

**Acceptance:**
- `cargo test -p camel-component-redis` passes.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- `cargo xtask lint-unwrap` exits 0 (no new unwrap).

- [x] 1.1

### Task 1.2: Wire the bucket split into the executor retry loop

**Files:**
- `crates/components/camel-redis/src/executor.rs` (modified)
- `crates/components/camel-redis/src/producer.rs` (modified)

**Steps:**
1. Introduce the phase-bearing attempt seam: add
   `pub struct AttemptFailure { pub err: CamelError, pub phase: SendPhase }`
   in `executor.rs` and change the executor method `execute_with_retry`
   consumes (the `execute_command` call returning `Result<(), CamelError>`)
   to return `Result<(), AttemptFailure>`. `MultiplexedExecutor::execute_command`
   wraps failures from connection checkout/dialing with `SendPhase::PreSend`
   and every failure after the request is handed to the connection with
   `SendPhase::PostFlush` (conservative default when flush status is
   unknown). Update the fake executor used in tests (`test_stub` harness and
   any `FakeExecutor`) to construct `AttemptFailure` with an explicit phase.
2. In `execute_with_retry`, classify each attempt failure with
   `retry_class(&failure.err, failure.phase)` and apply the bucket rules:
   `NotExecuted` → always schedule a retry (respecting the
   `NetworkRetryPolicy`/consecutive-failure budget in `config.rs::reconnect`
   and ADR-0013's per-loop reset points); `OutcomeUnknown` → retry only when
   the idempotence argument is true; `NeverTransient` → never retry, surface
   the error.
3. Remove the reconnect escape hatch: the current loop propagates
   `reconnect().await?` immediately (`executor.rs:667–670`), bypassing the
   retry budget. Fold redial into the classified attempt path: a failed
   reconnect is a `PreSend` `AttemptFailure` classified and budgeted like any
   other attempt (redial only when `invalidates_connection(&failure.err)`
   held; non-invalidating server rejections like `LOADING`/`BUSY` reuse the
   current connection — its generation tag stays valid, no re-dial).
4. Keep invalidation on its own track: whenever `invalidates_connection(err)`
   holds the connection is invalidated INDEPENDENT of replay (a non-idempotent
   post-flush drop invalidates but never replays).
5. `producer.rs` computes the idempotence argument BEFORE calling
   `execute_with_retry` (the retry loop wraps dispatch, so eligibility is a
   producer-side preflight): single commands pass `is_idempotent_command(&cmd)`
   (unchanged); this task adds the plumbing point that later tasks extend
   (Task 2.3 for scripts, Task 3.2 for batches).

**Tests:**
- `not_executed_failure_retries_non_idempotent_command`: setup: fake RESP
  harness (`executor.rs` `test_stub`) whose first connection attempt fails at
  checkout (`AttemptFailure` with `PreSend`), second succeeds; command
  `GETDEL` (not idempotent) → action: `execute_with_retry(...)` → assert:
  two attempts observed, final result Ok.
- `post_flush_drop_does_not_replay_non_idempotent_command`: setup:
  kill-after-flush harness (`PostFlush` failure); command `GETDEL` → action:
  run → assert: exactly one request received (no replay), error surfaced,
  connection invalidated (generation bumped).
- `post_flush_drop_replays_idempotent_command`: setup: kill-after-flush
  harness; command `GET` → action: run → assert: second request received
  after the kill, run completes Ok.
- `loading_reply_retries_without_invalidating`: setup: harness replying
  `-LOADING` once then `+OK`; command `GET` → action: run → assert: two
  attempts observed, result Ok, connection generation unchanged (no
  invalidation, no reconnect).
- `retry_budget_covers_failed_redials`: setup: harness whose first 3
  connection attempts fail at dial (including the reconnect path), fourth
  succeeds; a `NetworkRetryPolicy` budget of ≥ 4 → action: run `GET` →
  assert: exactly 4 attempts (the redial failures consumed the budget like
  any attempt), final result Ok. Companion assertion: with a budget of 2,
  the run surfaces the dial error after 2 attempts (no unbounded redial
  loop).
- `is_idempotent_command_corpus_unchanged`: setup: the
  `is_idempotent_command` function → action: evaluate for `LMPOP`, `SET`,
  `GETDEL`, `GET`, `SMISMEMBER`, `SETRANGE` → assert: false, false, false,
  true, true, true (pins the pop-family / optioned-SET / read scenarios of
  the MODIFIED "Retry classification" requirement).
- Command: `cargo test -p camel-component-redis not_executed`
  then `cargo test -p camel-component-redis post_flush`
  then `cargo test -p camel-component-redis loading_reply`
  then `cargo test -p camel-component-redis retry_budget`
  then `cargo test -p camel-component-redis is_idempotent_command_corpus`
- Expected: fail before implementation, pass after.

**Acceptance:**
- `cargo test -p camel-component-redis` passes (existing retry/failover tests
  unchanged or updated only where the new split intentionally changes
  behavior, with the change explained in the task result).
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.

- [x] 1.2

### Task 1.3: Amend ADR-0013 with the three-bucket retry semantics

**Files:**
- `docs/adr/0013-network-retry-policy-and-migration.md` (modified)

**Steps:**
1. Add a section "Execution-outcome classification (redisfc)" documenting the
   three buckets exactly as implemented, with the precedence order from Task
   1.1 (config/native-auth first, then `LOADING`/`BUSY`, then the existing
   transient class by phase, then semantic rejection): not-executed
   (pre-send failures, `LOADING`, `BUSY`) always replayed; outcome-unknown
   (post-flush transient I/O or unknown flush status) replayed only when
   idempotent (per command, batch iff every entry is, `EVAL`/`FCALL` only
   with the endpoint `idempotent` opt-in); never-transient (`EXECABORT`,
   per-command `EXEC` errors, `NOAUTH`/`WRONGPASS`/`NOPERM`, native auth and
   config errors, all other semantic/business errors) never replayed. Note
   the `READONLY` exception: it stays structurally transient and invalidating
   (replica redirection) with conservative replay. Note the script provenance
   rule: script-generated error replies can carry arbitrary codes after side
   effects, so a post-send script error reply is NOT proof of
   non-execution. The atomic provenance pair resolves each script reply
   before classification: a proven-forged reply or a runtime terminal
   failure is a BUSINESS error (never replayed — the script ran exactly
   once and its outcome stands); only transport-class failures after send
   are outcome-unknown (replayed under the `idempotent` opt-in) —
   `retry_class_script` implements this distinction. The reload paths are
   provenance-checked ATOMICALLY: every script invocation travels as one
   `MULTI; SCRIPT EXISTS <sha>; EVALSHA; EXEC` pair (the fcall mirror uses
   `FUNCTION LIST`, since `FUNCTION EXISTS` does not exist), so the
   presence observation cannot race a concurrent `SCRIPT FLUSH`/function
   deletion; a present target with a "NOSCRIPT"/"function not found" reply
   proves the reply is forged and the script ran — no re-invoke, business
   error.
2. State explicitly that invalidation and replayability are different
   questions: the invariant `invalidates_connection(e) ⇒
   is_transient_redis_error(e)` holds over error classes and is independent
   of the replay decision; a non-idempotent outcome-unknown failure
   invalidates the connection but is never replayed; non-invalidating server
   rejections (`LOADING`/`BUSY`) retry on the same connection.
3. Add a paragraph "Script trust boundary" restating ADR-0032 for scripts:
   script text is operator config sourced from the endpoint URI
   (`script`/`scriptFile`) only, never from exchange headers or body;
   `scriptFile` resolves against the route-file directory.
4. Add a paragraph "Why no WATCH": all endpoints share one
   `MultiplexedConnection` (multiplexed, generation-tagged cache), so
   WATCH/EXEC state would leak across exchanges; optimistic transactions are
   deliberately not offered.

**Tests:** (documentation task — verification is content-presence)
- `adr-0013-buckets-present`: open
  `docs/adr/0013-network-retry-policy-and-migration.md` → assert: contains
  `not-executed`, `outcome-unknown`, `never-transient` with the replay rules
  of step 1.
- `adr-0013-trust-and-watch-present`: same file → assert: contains a script
  trust-boundary paragraph citing ADR-0032 and a no-WATCH rationale naming
  the shared `MultiplexedConnection`.
- Command: `grep -c "outcome-unknown" docs/adr/0013-network-retry-policy-and-migration.md`
  then `grep -c "WATCH" docs/adr/0013-network-retry-policy-and-migration.md`
- Expected: both counts ≥ 1 only after this task.

**Acceptance:**
- The four step-1..4 contents are present in the ADR file.
- `cargo xtask lint-context-citations` exits 0.

- [x] 1.3

## Phase 2: eval|fcall script execution

### Task 2.1: Eval vertical slice — params, script runtime, dispatch, body binding

**Files:**
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/script.rs` (new)
- `crates/components/camel-redis/src/executor.rs` (modified)
- `crates/components/camel-redis/src/commands/mod.rs` (modified)
- `crates/components/camel-redis/src/lib.rs` (modified: `mod script;`)
- `crates/components/camel-redis/src/metadata.rs` (modified)
- `crates/components/camel-redis/Cargo.toml` (modified)

NOTE: this task introduces `RedisCommand::Eval` TOGETHER with its real
dispatch arm (the `executor.rs` match on `RedisCommand` is exhaustive — a
variant must never land without its implementation; no stub arms).

**Steps:**
1. `Cargo.toml`: enable the `script` feature of the `redis` crate
   (`redis = { workspace = true, features = ["tokio-comp", "aio", "script"] }`).
2. `config.rs`: add `RedisCommand::Eval` (`as_str()` → `"eval"`, `FromStr`
   accepts `"eval"`). Add `RedisEndpointConfig` fields `script:
   Option<String>`, `script_file: Option<PathBuf>`, `idempotent: bool`
   (default `false`). Parse URI params `script` (percent-decoded),
   `scriptFile`, `idempotent` (`true`/`false` only, else
   `CamelError::InvalidUri` naming the param). Validation for `Eval`:
   exactly one of `script`/`scriptFile` (both or neither → named error
   naming both params).
3. `script.rs`: `ScriptSource`, `ScriptKind::Eval`, `ScriptRuntime`,
   `load_script_source` per the symbol registry. Inline `script` used
   verbatim; `scriptFile` read from disk (path already resolved — Task 2.2
   owns relative resolution), read failure → named error naming the resolved
   path. `RedisProducer::new` builds one `Arc<ScriptRuntime>` and carries it
   in `CommandContext` (`script_idempotent = cfg.idempotent`).
4. `commands/mod.rs`: `script_operands(exchange)` reading the body as JSON
   `{"keys": [<string>...], "args": [<string|number>...]}`: non-object
   bodies, non-array `keys`/`args`, or non-string/number elements fail
   before send with a named error naming the body shape (strict-resolver
   precedent of `CamelRedis.Keys`); numeric args encode as their decimal
   string form.
5. `executor.rs`: `Eval` dispatch arm (receiving `ctx: &CommandContext`).
   The script runs via EXPLICIT commands, NOT `redis::Script::invoke_async`
   (its internal NOSCRIPT auto-reload would re-invoke invisibly and
   duplicate effects — `redis-1.8.1/src/script.rs:209–227`): use
   `redis::Script` for the script's SHA (`Script::new(code).get_hash()`,
   `redis-1.8.1/src/script.rs:44–46`) and send every invocation as ONE
   ATOMIC PROVENANCE PAIR (`redis::pipe().atomic()`):
   `MULTI; SCRIPT EXISTS <sha>; EVALSHA <sha> <n> <keys…> <args…>; EXEC`.
   The pair MUST be queried with `.atomic().ignore_errors()` and decoded
   from raw values (default pipeline querying aggregates entry errors before
   results — `redis-1.8.1/src/pipeline.rs:403–425` — which would destroy
   the provenance tuple); both the presence observation and a possible
   invocation error must survive into the decoded `(exists, result)` tuple.
   The pair's atomicity is the provenance guarantee: no other client
   command (e.g. `SCRIPT FLUSH`) can interleave between the existence
   observation and the invocation, so the reply tuple `(exists, result)`
   is trustworthy:
   a. `exists == 1` and `result` is a normal reply → map the result (the
      ordinary path; the pair is used on EVERY invocation, not just
      recovery).
   b. `exists == 1` and `result` is a `NOSCRIPT`-shaped error → the reply is
      FORGED (a cached script's invocation never returns NOSCRIPT — the
      script ran and returned the error itself): do NOT re-invoke; surface a
      named business error (`NeverTransient`); the side effects of that one
      invocation stand.
   c. `exists == 0` and `result` is `NOSCRIPT` → genuine non-execution:
      `SCRIPT LOAD` the source, then re-invoke ONCE via a second atomic
      pair; the second pair's result is the exchange result.
   d. A second `NOSCRIPT` after the reload (whatever the pair reports), a
      failed `SCRIPT LOAD`, or a malformed pair reply surfaces a named
      terminal error (`NeverTransient`). `NOSCRIPT` stays non-transient
      business everywhere else (`config.rs:1637`).
6. `commands/mod.rs`: `redis_value_to_body(value: &redis::Value) -> Result<Body, CamelError>`
   (FALLIBLE — `ServerError` replies cannot be represented as a body) with
   this exact mapping over the redis 1.8.1 `Value` variants: `Nil` →
   `Body::Json(null)`; `Int(i64)` → `Body::Json(number)`; `Okay` →
   `Body::Text("OK")`; `SimpleString(s)` → `Body::Text(s)`; `BulkString(b)` →
   UTF-8 valid → `Body::Text`, else `Body::Bytes` (top-level binary replies
   keep the byte-body contract); `Array` → `Body::Json(JSON array)` with the
   element mapping: nested UTF-8 strings → JSON strings, nested integers →
   numbers, `Nil` → null, `Okay`/`SimpleString` → their text, nested arrays
   recursively, nested non-UTF-8 `BulkString` bytes → JSON object
   `{"bytes": "<base64 of the raw bytes>"}` (JSON arrays cannot carry raw
   bytes; this is the explicit nested-binary representation); `Double` →
   JSON number; `Boolean` → JSON bool; `VerbatimString` → its text content;
   `BigNumber(bytes)` → JSON string of the raw decimal digits as UTF-8
   lossy text (the driver carries `Vec<u8>`, not a big-num type);
   `Map` → JSON object whose keys are the element-mapped key rendered to a
   string (UTF-8 lossy bulk/simple text; `Int` as its decimal string; other
   key kinds via their element-mapped JSON serialized to a string) and whose
   values use the element mapping; `Set` → JSON array (element mapping);
   `Attribute` → RESP3 attributes are metadata attached to a reply: drop the
   attribute and map the underlying data value; `ServerError` → NOT
   body-representable: the function returns `Err(CamelError)` carrying the
   error text — at top level and for any `ServerError` nested inside an
   `Array` (batch context: `plan`/dispatch extracts per-entry errors with
   the entry index BEFORE conversion, Task 3.2 step 4; script context: the
   named terminal/`OutcomeUnknown` paths); `Push` → `Body::Json(null)` (not
   expected on these paths; defensive, must not panic). The existing
   `reply_to_body(Option<&[u8]>)` stays unchanged; script and batch paths
   use `redis_value_to_body`.
6b. `retry_class_script(err, phase)` and `pub enum RequestKind { Plain,
   Script }` in `transport_error.rs`: script-generated error replies can
   carry ARBITRARY codes (a Lua script can run side effects and then return
   `redis.error_reply('BUSY ...')`), so a post-send script error reply is
   NOT proof of non-execution. The dispatch's atomic provenance pair (step
   5) resolves each script reply before classification: proven-forged
   replies and runtime terminal failures arrive here as NAMED BUSINESS
   ERRORS from the script runtime. Precedence:
   a. Config errors and native `redis::ErrorKind::AuthenticationFailed` →
      `NeverTransient`.
   b. Script business/terminal errors (named errors raised by the script
      runtime: proven-forged replies, reload-exhausted `NOSCRIPT`, failed
      `SCRIPT LOAD` / `FUNCTION LOAD`, malformed provenance pair) →
      `NeverTransient`.
   c. `phase == PreSend` with a transient error → `NotExecuted` (proven:
      nothing was sent).
   d. Everything else post-flush — TRANSPORT-class failures (I/O, timeout;
      the script may or may not have run) → `OutcomeUnknown` (replayed only
      under the `idempotent` opt-in). `execute_with_retry` takes
      `kind: RequestKind` and selects the classifier accordingly; plain
      requests keep `retry_class` (v1 batch entries are plain commands and
      cannot fabricate replies, so their whole-request `LOADING`/`BUSY`
      provenance holds).
7. Reject script text from exchange data (shared producer preflight in
   `producer.rs`): if header `CamelRedis.Script` is present on a script-kind
   request, fail before send with a named error citing ADR-0032 (same
   pattern as the `CamelRedis.Command` rejection at `producer.rs:52`). This
   task wires the preflight for `Eval`; Task 2.3 adds `Fcall` to the same
   check.
8. `metadata.rs`: declare `script`, `scriptFile`, `idempotent` with
   `#[uri_param(...)]`; regenerate schemas (`cargo xtask schema`) and commit.

**Tests:**
- `eval_parses_inline_script_param`: setup: URI
  `redis://localhost/?command=eval&script=return%201` → action:
  `RedisEndpointConfig::from_uri` → assert: Ok, `script == Some("return 1")`.
- `eval_rejects_script_and_scriptfile_together`: setup: URI with both
  `script=...&scriptFile=...` → action: `from_uri` → assert: Err naming
  `script` and `scriptFile`.
- `eval_rejects_neither_script_source`: setup: URI `?command=eval` → action:
  `from_uri` → assert: Err naming `script`/`scriptFile`.
- `idempotent_param_parses_bool`: setup: URIs with `idempotent=true`,
  `idempotent=false`, `idempotent=maybe` → action: `from_uri` → assert: Ok
  true, Ok false, Err naming `idempotent`.
- `eval_inline_script_runs_with_body_keys_and_args`: setup: fake RESP
  harness replying an integer; config `command=eval`, `script=return 1`;
  body `{"keys": ["k1"], "args": ["a1"]}` → action: run the producer →
  assert: wire request is `EVALSHA`-shaped with 1 key `k1` and arg `a1`;
  reply becomes the exchange body.
- `eval_provenance_pair_is_atomic`: setup: fake RESP harness recording raw
  pipeline bytes; `command=eval`, `script=return 1` → action: run once →
  assert: the invocation arrives as ONE atomic pipeline containing `MULTI`,
  `SCRIPT EXISTS <sha>`, `EVALSHA`, `EXEC` (no interleaving window between
  the existence observation and the invocation — the deterministic race
  proof).
- `eval_noscript_reloads_once_and_reinvokes`: setup: fake RESP harness whose
  first pair replies `0` to `SCRIPT EXISTS` and `-NOSCRIPT` to `EVALSHA`;
  `OK` to `SCRIPT LOAD`; second pair replies `1` and integer `1` → action:
  run → assert: harness observes pair1 (`SCRIPT EXISTS`, `EVALSHA`),
  `SCRIPT LOAD`, pair2 (`SCRIPT EXISTS`, `EVALSHA`) in that order; result
  Ok body `1`.
- `eval_second_noscript_is_named_error`: setup: harness whose first pair
  replies `0`/`-NOSCRIPT`, `OK` to `SCRIPT LOAD`, second pair replies
  `1`/`-NOSCRIPT` → action: run → assert: exactly two `EVALSHA`
  invocations, no third, named terminal error containing `NOSCRIPT`.
- `eval_forged_noscript_not_reinvoked_without_optin`: setup: harness whose
  single pair replies `1` to `SCRIPT EXISTS <sha>` and `-NOSCRIPT forged`
  to `EVALSHA` (the SHA IS cached — the script ran and forged the reply) →
  action: run → assert: exactly ONE `EVALSHA` invocation (one script side
  effect), NO `SCRIPT LOAD`, NO re-invoke; error surfaced as a named
  business error classified `NeverTransient`.
- `eval_rejects_script_header`: setup: `command=eval`, `script=...`,
  exchange carrying header `CamelRedis.Script` with Lua text → action: run →
  assert: fails before send (harness sees zero requests), error names the
  header.
- `eval_malformed_body_operands_named_error`: setup: body `{"keys": "k1"}`
  → action: run → assert: fails before send, error names the expected body
  shape `{"keys": [...], "args": [...]}`.
- `script_keys_and_args_bind_from_body`: setup: body
  `{"keys": ["a", "b"], "args": ["x", 7]}` → action: `script_operands(...)` →
  assert: Ok with keys `["a", "b"]` and args `["x", "7"]`.
- `non_script_commands_keep_header_sourced_keys`: setup: `command=get`
  exchange with `CamelRedis.Key=k1` and plain text body → action: run the
  producer → assert: wire request carries key `k1` from the header.
- `redis_value_to_body_maps_nested_replies`: setup:
  `redis::Value::Array(vec![Int(1), BulkString("ütf8".as_bytes().to_vec()),
  BulkString(vec![0xFF]), Nil, Okay])` plus standalone
  `BulkString(vec![0xFF])` → action: `redis_value_to_body` on each → assert:
  the array maps to `Ok(Body::Json([1, "ütf8", {"bytes": "/w=="}, null,
  "OK"]))` (base64 of `[0xFF]` is `/w==`); the standalone binary maps to
  `Ok(Body::Bytes(vec![0xFF]))` (top-level byte-body contract preserved).
- `redis_value_to_body_maps_map_bignumber_and_attribute`: setup:
  `Map` with entries `Int(1) → BulkString("one")` and
  `BulkString("k") → Int(2)`; `BigNumber(b"12345678901234567890")`; an
  `Attribute` wrapping `Int(5)` → action: `redis_value_to_body` on each →
  assert: `Ok(Body::Json({"1": "one", "k": 2}))`,
  `Ok(Body::Json("12345678901234567890"))`, and `Ok(Body::Json(5))`
  (attribute dropped, underlying value mapped).
- `redis_value_to_body_server_error_is_err`: setup: a top-level
  `ServerError("WRONGTYPE ...")` and an `Array` containing a nested
  `ServerError("ERR ...")` → action: `redis_value_to_body` on each → assert:
  both return `Err(CamelError)` naming the server error text (fallible
  channel; no body is produced).
- `retry_class_script_business_errors_are_never_transient`: setup: the
  named business errors the script runtime raises — a proven-forged reply
  error, the reload-exhausted `NOSCRIPT` error, a failed `SCRIPT LOAD`
  error → action: `retry_class_script(err, SendPhase::PostFlush)` → assert:
  all `RetryClass::NeverTransient`.
- `retry_class_script_pre_send_is_not_executed`: setup: `CamelError::Io`
  dial failure → action: `retry_class_script(err, SendPhase::PreSend)` →
  assert: `RetryClass::NotExecuted`.
- `retry_class_script_post_flush_io_is_outcome_unknown`: setup:
  `CamelError::Io` connection reset after flush (the script may or may not
  have run) → action: `retry_class_script(err, SendPhase::PostFlush)` →
  assert: `RetryClass::OutcomeUnknown`.
- `eval_faked_busy_reply_not_replayed_without_optin`: setup: fake RESP
  harness whose pair replies `1` to `SCRIPT EXISTS` and `-BUSY fake script
  reply` to `EVALSHA`; `command=eval`, no `idempotent` opt-in → action: run
  → assert: exactly ONE invocation observed (one script side effect), error
  surfaced as a named business error, no replay.
- `redis_metadata_uri_options_parity` (existing): assert: still passes with
  the new params registered.
- Command: `cargo test -p camel-component-redis eval_`
  then `cargo test -p camel-component-redis idempotent_param`
  then `cargo test -p camel-component-redis script_keys`
  then `cargo test -p camel-component-redis non_script_commands`
  then `cargo test -p camel-component-redis redis_value_to_body`
  then `cargo test -p camel-component-redis retry_class_script`
  then `cargo test -p camel-component-redis metadata`
- Expected: fail before implementation, pass after.

**Acceptance:**
- `cargo test -p camel-component-redis` passes.
- `cargo xtask schema --check` exits 0.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- `cargo xtask lint-unwrap` and `cargo xtask lint-secrets` exit 0.

- [x] 2.1

### Task 2.2: scriptFile path resolution (DSL route-file anchor)

**Files:**
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/components/camel-redis/src/script.rs` (modified)

**Steps:**
1. In `camel-dsl/src/yaml.rs`, extend the route-load endpoint handling so a
   redis endpoint URI carrying `scriptFile` has its value resolved before the
   endpoint is built, reusing `ScriptLoadContext::resolve` (`yaml.rs:2354`
   semantics): relative paths join the route file's parent directory
   (`base_dir`), absolute paths as-is, no multi-directory guessing. Reuse
   the route's existing `ScriptLoadContext` instance so `record` feeds script
   watching/registration identically.
2. Without a route-file context (programmatic endpoint construction),
   `load_script_source` resolves a relative `scriptFile` against the process
   working directory (the documented `ScriptLoadContext` fallback) —
   implement in `script.rs::load_script_source`.
3. Every read failure names the resolved path in the error message.

**Tests:**
- `scriptfile_resolves_against_route_file_dir`: setup: temp dir `D` with
  route file `route.yaml` whose redis endpoint URI has
  `scriptFile=scripts/lock.lua` and `D/scripts/lock.lua` exists with known
  Lua source → action: load the route, run one exchange on the endpoint
  (fake RESP harness) → assert: the executed script source is
  `D/scripts/lock.lua`'s (harness sees its SHA); no CWD dependence (test
  runs with CWD ≠ `D`).
- `scriptfile_missing_file_names_resolved_path`: setup: route file in `D`
  with `scriptFile=missing.lua`, no such file → action: load the route →
  assert: error message contains the string `D/missing.lua`.
- `programmatic_relative_scriptfile_uses_cwd`: setup: no route context,
  `scriptFile=rel.lua`; guarded `std::env::set_current_dir` to a temp dir
  containing `rel.lua` (follow the repo's env-mutation lint pattern) →
  action: `load_script_source(...)` → assert: source loaded from the
  CWD-joined path.
- Command: `cargo test -p camel-dsl scriptfile`
  then `cargo test -p camel-component-redis programmatic_relative`
- Expected: fail before implementation, pass after.

**Acceptance:**
- `cargo test -p camel-dsl` and `cargo test -p camel-component-redis` pass.
- `cargo xtask lint-no-env-mutation-unguarded` exits 0.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.

- [x] 2.2

### Task 2.3: Fcall vertical slice, idempotent opt-in, live tests, eval/fcall docs

**Files:**
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/script.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified)
- `crates/components/camel-redis/src/metadata.rs` (modified)
- `crates/camel-test/tests/redis_test.rs` (modified)
- `docs/src/components/redis.md` (modified)

NOTE: `RedisCommand::Fcall` is introduced HERE together with its real
dispatch arm (exhaustiveness-safe; no stub arms).

**Steps:**
1. `config.rs`: add `RedisCommand::Fcall` (`as_str()` → `"fcall"`,
   `FromStr` accepts `"fcall"`), field `function: Option<String>`, and its
   URI parsing: `Fcall` requires `function` (missing → named error) and
   exactly one of `script`/`scriptFile`; `Eval` with `function` set → named
   error. `metadata.rs`: register `function`; regenerate schemas.
2. `script.rs`: implement `ScriptKind::Fcall` handling inside
   `ScriptRuntime` with explicit commands and the same ATOMIC PROVENANCE
   PAIR design as Task 2.1 step 5 (no invisible auto-recovery). NOTE: there
   is NO `FUNCTION EXISTS` command — the presence check is `FUNCTION LIST`
   and the lookup inspects registered FUNCTION names (not library names)
   within each library's `functions` list. Because `FCALL` binds by NAME, a
   pre-existing function with the same name but different code would
   otherwise run: before the FIRST invocation on an endpoint, run
   `FUNCTION LOAD REPLACE` with the configured library source (first-use
   lazy load binding the name to the configured code, guarded by
   `ScriptRuntime::init` — `tokio::sync::OnceCell` shared across producer
   clones, exactly one load under concurrent first use; repeat exchanges
   skip it). Every invocation is one
   `redis::pipe().atomic()`: `MULTI; FUNCTION LIST; FCALL <function> <n>
   <keys…> <args…>; EXEC`, queried with `.ignore_errors()` and raw-value
   decoding (same requirement as Task 2.1 step 5 — default pipeline
   querying would destroy the provenance tuple); the pair reply is
   `(function_present, result)`:
   a. present and normal result → map the result (ordinary path).
   b. present and a "function not found"-shaped error → FORGED (a registered
      function's invocation cannot miss — it ran and returned the error):
      do NOT re-invoke; surface a named business error (`NeverTransient`).
   c. absent and "function not found" → genuine non-execution:
      `FUNCTION LOAD REPLACE` the library source, then re-invoke ONCE via a
      second atomic pair.
   d. A repeated "function not found" after the reload, a failed
      `FUNCTION LOAD`, or a malformed pair reply surfaces a named terminal
      error (never transient).
3. `executor.rs`: `Fcall` dispatch arm through `ScriptRuntime` with the same
   `script_operands` body binding and `redis_value_to_body` reply mapping as
   `Eval`. Add `Fcall` to the shared `CamelRedis.Script` header-rejection
   preflight from Task 2.1 step 7 (script text from headers is rejected for
   BOTH `Eval` and `Fcall`).
4. Idempotent opt-in wiring (producer-side preflight, per Task 1.2 step 5):
   `Eval`/`Fcall` pass `ctx.script_idempotent` (from `cfg.idempotent`,
   default `false`) as the idempotence argument to `execute_with_retry`.
5. Live-Redis tests in `crates/camel-test/tests/redis_test.rs` (behind the
   `integration-tests` feature, `shared_redis()` helper):
   `live_eval_script_roundtrip`, `live_fcall_function_roundtrip`.
6. Docs in `docs/src/components/redis.md`: update the Commands table
   (replace the "does not expose `EVAL`, `EVALSHA`, or script-loading
   commands" note with `eval`/`fcall` rows), add params
   `script`/`scriptFile`/`function`/`idempotent` to the param table, add a
   "Script commands" section documenting the body shape
   `{"keys": [...], "args": [...]}`, the script trust boundary (URI/file
   only — ADR-0032), `scriptFile` resolution, NOSCRIPT reload semantics, and
   the `idempotent` opt-in's retry effect.

**Tests:**
- `fcall_requires_function_param`: setup: URI `?command=fcall&script=...`
  (no `function`) → action: `from_uri` → assert: Err naming `function`.
- `fcall_loads_library_and_invokes`: setup: fake RESP harness: `OK` to
  `FUNCTION LOAD`; pairs reply present and a bulk reply to `FCALL`; config
  `command=fcall&function=lock&script=...` → action: run two exchanges →
  assert: exactly one `FUNCTION LOAD REPLACE` (first-use lazy load before
  the first pair), two `FCALL` invocations (one pair per exchange), replies
  mapped to bodies.
- `fcall_first_use_replaces_preexisting_function`: setup: harness whose
  `FUNCTION LIST` already reports a `lock` function (a DIFFERENT
  pre-existing implementation) → action: run one exchange with the
  configured `scriptFile` source → assert: the wire traffic contains
  `FUNCTION LOAD REPLACE` with the configured source BEFORE the first
  `FCALL` pair (the configured implementation runs, not the pre-existing
  one); a second exchange sends no `FUNCTION LOAD`.
- `fcall_missing_function_reloads_once`: setup: harness after first-use
  init (the first-use `FUNCTION LOAD REPLACE` already ran in a prior
  exchange), then simulating an eviction/restart: pair1 replies absent
  (`FUNCTION LIST` without `lock`) and `-ERR Function not found`; `OK` to
  the RECOVERY `FUNCTION LOAD`; pair2 replies present and a bulk reply →
  action: run → assert: one recovery `FUNCTION LOAD`, one re-invoke, Ok
  (initial and recovery loads are distinguishable in the wire log); with a
  harness always replying absent + "function not found" after init: exactly
  two `FCALL` invocations and a named terminal error.
- `fcall_concurrent_first_use_loads_once`: setup: producer cloned N times
  sharing one `Arc<ScriptRuntime>`; harness counting `FUNCTION LOAD`
  commands; N concurrent first exchanges → action: run all N → assert:
  exactly ONE `FUNCTION LOAD REPLACE` observed (the `OnceCell` init runs
  once), N `FCALL` invocations, all exchanges Ok.
- `fcall_forged_function_not_found_not_reinvoked_without_optin`: setup:
  harness after first-use init, whose pair replies present
  (`FUNCTION LIST` includes `lock`) and `-ERR Function not found forged` to
  `FCALL` (the function exists — the reply is forged) → action: run the
  measured exchange → assert: exactly ONE `FCALL` invocation (one side
  effect), ZERO recovery loads (the only load is the first-use init),
  NO re-invoke; error surfaced as a named business error classified
  `NeverTransient`.
- `eval_non_idempotent_outcome_unknown_not_retried`: setup:
  kill-after-flush harness; `command=eval` without `idempotent` → action:
  run → assert: exactly one invocation, error surfaced.
- `eval_idempotent_optin_outcome_unknown_retried`: setup:
  kill-after-flush harness; `command=eval&idempotent=true` → action: run →
  assert: second invocation after the kill, run completes Ok.
- `fcall_idempotent_optin_outcome_unknown_retried`: setup:
  kill-after-flush harness; `command=fcall&function=lock&idempotent=true`
  → action: run → assert: second invocation after the kill, run completes Ok.
- `fcall_rejects_script_header`: setup: `command=fcall`, `function=lock`,
  `script=...`, exchange carrying header `CamelRedis.Script` with Lua text →
  action: run → assert: fails before send with a named error naming the
  header (harness observes zero requests).
- `fcall_faked_loading_reply_not_replayed_without_optin`: setup: fake RESP
  harness whose pair replies present and `-LOADING fake script reply` to
  every `FCALL`; no `idempotent` opt-in → action: run → assert: exactly ONE
  `FCALL` invocation observed (one side effect), error surfaced as a named
  business error, no replay.
- `live_eval_script_roundtrip`: setup: `shared_redis()` container; endpoint
  `command=eval` with inline script
  `redis.call('set',KEYS[1],ARGV[1]) return 1` (URI-encoded); body
  `{"keys": ["live:k"], "args": ["v"]}` → action: send one exchange →
  assert: body is `1`; a follow-up `GET live:k` exchange returns `v`.
- `live_fcall_function_roundtrip`: setup: `shared_redis()` container; write
  library source `#!lua name=echo` + `redis.register_function('echo',
  function(keys, args) return args[1] end)` to a temp file and point
  `scriptFile` at it; endpoint `command=fcall&function=echo` → action: send
  one exchange with body `{"keys": [], "args": ["hi"]}` → assert: body is
  `hi`.
- `live_fcall_missing_function_recovery`: setup: same `echo` endpoint after
  a successful exchange; run `FUNCTION FLUSH` against the container
  (evicting the library) → action: send a second exchange with body
  `{"keys": [], "args": ["again"]}` → assert: the exchange recovers via the
  provenance pair (`FUNCTION LIST` absent → `FUNCTION LOAD` → re-invoke) and
  the body is `again`.
- Command: `cargo test -p camel-component-redis fcall_`
  then `cargo test -p camel-component-redis eval_non_idempotent`
  then `cargo test -p camel-component-redis eval_idempotent`
  then `cargo test -p camel-test --features integration-tests --test redis_test live_eval`
  then `cargo test -p camel-test --features integration-tests --test redis_test live_fcall`
- Expected: unit tests fail before implementation; live tests require Docker
  (host has it; ~1 min warm).

**Acceptance:**
- `cargo test -p camel-component-redis` passes.
- `cargo test -p camel-test --features integration-tests --test redis_test` passes.
- `cd docs && mdbook test` exits 0.
- `cargo xtask schema --check` exits 0.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.

- [x] 2.3

## Phase 3: multi batch

### Task 3.1: CamelRedis.Batch parsing, allowlist and the atomic param

**Files:**
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/commands/batch.rs` (new)
- `crates/components/camel-redis/src/commands/mod.rs` (modified: module wiring)
- `crates/components/camel-redis/src/metadata.rs` (modified)

NOTE: this task does NOT touch the `RedisCommand` enum — `BatchEntry`
commands are existing variants parsed via `FromStr`, so no exhaustive-match
breakage. `RedisCommand::Multi` lands in Task 3.2 with its dispatch arm.

**Steps:**
1. `config.rs`: add field `atomic: bool` (default `true`) parsed from URI
   param `atomic` (`true`/`false` only, else `CamelError::InvalidUri`
   naming `atomic`).
2. `commands/batch.rs`: `BatchEntry`, `FieldSpec`, `FieldKind`, `BatchPlan`,
   `plan_batch`, `batchable`, `allowed_entry_fields`, `MAX_BATCH_ENTRIES`
   per the symbol registry. `plan_batch` reads header `CamelRedis.Batch`:
   MUST be a JSON array of objects; each `command` value MUST be a string
   parsing into `RedisCommand`; malformed JSON, non-array, non-object
   entries, or bad `command` fail before send with a named error naming
   `CamelRedis.Batch` and the entry index. Entry count > `MAX_BATCH_ENTRIES`
   fails before send naming the cap. `plan_batch` also validates the body:
   a JSON array with exactly one element per entry (length mismatch → named
   error naming expected and actual counts), and computes
   `BatchPlan.idempotent = entries.iter().all(|e| is_idempotent_command(&e.command))`.
3. `batchable` returns true ONLY for commands with an `encode_entry`
   encoder — the v1 allowlist: `GET`, `SET`, `GETDEL`, `GETEX`, `SETRANGE`,
   `GETRANGE`, `INCR`, `INCRBY`, `DECR`, `DECRBY`, `APPEND`, `MGET`, `DEL`,
   `UNLINK`, `EXPIRE`, `PERSIST`, `COPY`, `TOUCH`. Everything else (blocking
   pops, consumer, pubsub, stream, script commands) returns `false`.
4. `allowed_entry_fields` — the complete per-command field contract. Field
   names match the EXISTING `CamelRedis.*` argument semantics (the resolvers
   in `commands/*` are the norm): value payloads always come from the body,
   never from fields.
   - `GET`, `GETDEL`, `INCR`, `DECR`, `PERSIST`: `key` (required, Str).
   - `SET`: `key` (required, Str). Value from body.
   - `GETEX`: `key` (required, Str), `timeout` (optional, PositiveInt) —
     matching the existing `CamelRedis.Timeout` semantics
     (`get_i64_header` + positive validation, `commands/key.rs:59–72`).
   - `SETRANGE`: `key` (required, Str), `offset` (required, UInt). Value
     from body.
   - `GETRANGE`: `key` (required, Str), `start` (required, Int), `end`
     (required, Int).
   - `INCRBY`, `DECRBY`: `key` (required, Str), `increment` (required, Int).
   - `APPEND`: `key` (required, Str). Value from body.
   - `MGET`, `DEL`, `UNLINK`, `TOUCH`: `keys` (required, Strs, non-empty).
   - `EXPIRE`: `key` (required, Str), `timeout` (required, PositiveInt).
   - `COPY`: `key` (required, Str), `destination` (required, Str) —
     mirroring `resolve_copy_args`.
   `PositiveInt` = i64 range, strictly > 0 (zero, negative, or overflow →
   named error naming the entry index and field). Unknown fields,
   wrong-typed values (FieldKind mismatch), or missing required fields fail
   before send with named errors naming the entry index and the field.
   Int/UInt accept JSON numbers and numeric strings.
5. `metadata.rs`: register URI param `atomic`. Regenerate schemas
   (`cargo xtask schema`), commit.

**Tests:**
- `batch_parses_allowlisted_entries`: setup: header
  `CamelRedis.Batch=[{"command":"set","key":"a"},{"command":"incr","key":"b"}]`
  → action: `plan_batch` + `batchable` gate → assert: Ok, 2 entries,
  commands `RedisCommand::Set`/`RedisCommand::Incr`, `batchable` true,
  `idempotent == false` (SET/INCR not idempotent).
- `batch_unknown_command_fails_closed_with_index`: setup:
  `[{"command":"set","key":"a"},{"command":"nosuchcmd"}]` → action: parse →
  assert: Err naming `CamelRedis.Batch` and index 1; nothing sent.
- `batch_non_batchable_command_rejected_with_index`: setup: entry
  `{"command":"blpop","key":"k"}` → action: `batchable` gate → assert: Err
  naming index 0 and the command.
- `batch_unknown_field_or_bad_type_fails_with_index`: setup: entry
  `{"command":"expire","key":"k","timeout":"soon"}` and entry
  `{"command":"set","key":"a","bogus":1}` → action: field validation →
  assert: two named errors, each naming the entry index and field.
- `batch_missing_required_field_fails_with_index`: setup: entry
  `{"command":"copy","key":"a"}` (no `destination`) → action: field
  validation → assert: Err naming index 0 and `destination`.
- `batch_timeout_must_be_positive`: setup: entries
  `{"command":"expire","key":"k","timeout":0}` and
  `{"command":"expire","key":"k","timeout":-5}` and
  `{"command":"expire","key":"k","timeout":99999999999999999999}`
  (i64 overflow) → action: field validation → assert: three named errors,
  each naming the entry index and `timeout`.
- `batch_body_length_mismatch_fails_named_error`: setup: 3 entries, body
  array of 2 → action: `plan_batch` → assert: Err naming expected 3 and
  actual 2.
- `batch_entry_count_cap_fails_named_error`: setup: 257 valid entries →
  action: `plan_batch` → assert: Err naming `MAX_BATCH_ENTRIES`.
- `redis_metadata_uri_options_parity` (existing): assert: still passes with
  `atomic` registered.
- Command: `cargo test -p camel-component-redis batch_`
  then `cargo test -p camel-component-redis metadata`
- Expected: fail before implementation, pass after.

**Acceptance:**
- `cargo test -p camel-component-redis` passes.
- `cargo xtask schema --check` exits 0.
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.

- [x] 3.1

### Task 3.2: Batch encoders, Multi dispatch, pipeline execution and error precedence

**Files:**
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/commands/batch.rs` (modified)
- `crates/components/camel-redis/src/commands/string.rs` (modified)
- `crates/components/camel-redis/src/commands/key.rs` (modified)
- `crates/components/camel-redis/src/executor.rs` (modified)
- `crates/components/camel-redis/src/producer.rs` (modified)

NOTE: `RedisCommand::Multi` is introduced HERE together with its real
dispatch arm (exhaustiveness-safe; no stub arms).

**Steps:**
1. Extract pure `encode_<cmd> -> Result<redis::Cmd, CamelError>` helpers
   from the `commands/string.rs` and `commands/key.rs` dispatchers for the
   18 batchable commands (args per the Task 3.1 field contract); each helper
   builds the `redis::Cmd` from resolved arguments. The existing dispatch
   path calls the helper then sends unchanged — behavior-preserving:
   existing single-command options and validations (e.g. `CamelRedis.Timeout`
   positive checks, `SET` option forms) stay exactly as they are.
2. `commands/batch.rs`: `encode_entry(entry, value)` resolving args from
   validated entry fields plus the body value slot (values never from
   fields), delegating to the `encode_*` helpers.
3. `config.rs`: add `RedisCommand::Multi` (`as_str()` → `"multi"`).
   `executor.rs`: `Multi` dispatch arm (receiving `ctx: &CommandContext`):
   encode all entries from `ctx.batch` (the `BatchPlan` built by the
   producer preflight), send via `redis::pipe().atomic()` with all commands
   queued when `ctx.atomic`, else plain `redis::pipe()`. Query the pipeline
   with `ignore_errors()` (redis 1.8.1 default pipeline querying collapses
   the first entry error into an indexed overall `RedisError`,
   `pipeline.rs:403–425`) and collect per-entry raw values; each entry
   reply becomes one JSON array element via `redis_value_to_body`'s element
   mapping (non-UTF-8 entry reply → `{"bytes": "<base64>"}`).
4. Per-entry error provenance: entries whose raw reply is an error become a
   named `CamelError` carrying the entry index and error text. A per-entry
   error means earlier entries DID execute: the batch is never replayed
   wholesale and the error classifies `NeverTransient` — INCLUDING per-entry
   `LOADING`/`BUSY` replies (those are NOT the whole-request `NotExecuted`
   path; partial success must never replay). `EXECABORT` (whole-request
   rejection) maps through the same `NeverTransient` classification.
   Connection-level post-flush failures classify `OutcomeUnknown` (Task 1.2
   handles replay).
5. Producer-side batch preflight (per Task 1.2 step 5): `producer.rs` calls
   `plan_batch(exchange)` BEFORE `execute_with_retry` and passes
   `CommandContext { batch: Some(plan), atomic: cfg.atomic, .. }` through
   retry → executor → dispatch; `execute_with_retry`'s idempotence argument
   for `Multi` is `plan.idempotent` (= every entry idempotent).

**Tests:**
- `batch_atomic_runs_as_multi_exec`: setup: fake RESP harness recording raw
  pipeline bytes; entries `SET` + `INCR` with body `["v", null]`;
  `atomic=true` → action: run via the producer (preflight + dispatch) →
  assert: wire bytes contain `MULTI`, both commands, `EXEC` in one pipeline;
  reply array mapped to body.
- `batch_plain_pipeline_skips_multi_exec`: setup: same entries,
  `atomic=false` → action: run → assert: wire bytes contain the two commands
  with no `MULTI`/`EXEC`.
- `batch_partial_entry_failure_not_replayed`: setup: `atomic=false`;
  harness replies `+OK` for entry 0 and `-WRONGTYPE ...` for entry 1 →
  action: run → assert: error names entry index 1 and `WRONGTYPE`; harness
  received exactly one pipeline (no replay); `retry_class(err, PostFlush)`
  is `NeverTransient`.
- `batch_partial_success_with_loading_entry_is_never_replayed`: setup:
  `atomic=false`; harness replies `+OK` for entry 0 (`INCR`, executed) and
  `-LOADING` for entry 1 (and likewise a `-BUSY` variant) → action: run →
  assert: exactly one pipeline received (no replay despite the
  `LOADING`/`BUSY` code — partial success is not whole-request
  non-execution); error names the entry index.
- `batch_execabort_is_never_transient`: setup: harness replying
  `-EXECABORT Transaction discarded` → action: run `atomic=true` batch →
  assert: error surfaced, `retry_class(err, PostFlush)` is
  `NeverTransient`, exactly one attempt.
- `batch_idempotence_aggregates_entries`: setup: batches `[GET, GETDEL]` and
  `[GET, GET]` → action: `plan_batch` idempotence → assert: false and true.
- `encode_entry_matches_dispatch_encoding`: setup: entry
  `{"command":"set","key":"a"}` with body value `"v"` vs a single-command
  `SET` exchange with `CamelRedis.Key=a` and body `v` → action:
  `encode_entry` vs the extracted `encode_set` helper → assert: identical
  `redis::Cmd` debug output.
- Command: `cargo test -p camel-component-redis batch_`
  then `cargo test -p camel-component-redis encode_entry`
- Expected: fail before implementation, pass after.

**Acceptance:**
- `cargo test -p camel-component-redis` passes (existing command tests prove
  the refactor is behavior-preserving).
- `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 3.2

### Task 3.3: Batch retry behavior, live tests and docs

**Files:**
- `crates/components/camel-redis/src/executor.rs` (modified)
- `crates/camel-test/tests/redis_test.rs` (modified)
- `docs/src/components/redis.md` (modified)

**Steps:**
1. Verify the batch retry wiring (Tasks 1.2 + 3.2) against the tests below
   and close any gap they expose: `NotExecuted` always replays batches,
   `OutcomeUnknown` replays only all-idempotent batches (`BatchPlan.idempotent`),
   `NeverTransient` never replays.
2. Live-Redis tests in `crates/camel-test/tests/redis_test.rs`
   (`integration-tests` feature, `shared_redis()` helper):
   `live_multi_atomic_batch_roundtrip` (SET+INCR batch on two keys, assert
   both applied and reply array) and `live_multi_partial_entry_error_is_named`
   (`INCR` on a list key → `WRONGTYPE` at exec; assert the error names entry
   0 and `WRONGTYPE`). EXECABORT and kill-based replay semantics are covered
   by the unit tests only (no live trigger).
3. Docs in `docs/src/components/redis.md`: add `multi` to the Commands table;
   add a "Batch (multi)" section documenting `CamelRedis.Batch` entry shape,
   the per-command field contract (the Task 3.1 table), the typed allowlist
   (commands without batch encoders are rejected), `atomic` semantics, body
   value array alignment and the length rule, the entry-count cap, error
   precedence (partial entry failures are never replayed — including
   per-entry `LOADING`/`BUSY` after partial success), and batch idempotence
   aggregation; extend the retry-semantics doc text with the three buckets
   and a pointer to ADR-0013.

**Tests:**
- `batch_post_flush_drop_non_idempotent_not_retried`: setup:
  kill-after-flush harness; batch `[INCR]` → action: run → assert: exactly
  one pipeline received, error surfaced.
- `batch_post_flush_drop_all_idempotent_retried`: setup:
  kill-after-flush harness; batch `[GET, GET]` → action: run → assert:
  second pipeline after the kill, run completes Ok.
- `batch_pre_send_failure_retries_non_idempotent_batch`: setup: harness
  whose first checkout fails at dial (`PreSend`); batch `[INCR]` → action:
  run → assert: two attempts, final Ok.
- `batch_atomic_disconnect_after_flush_is_outcome_unknown`: setup:
  kill-after-flush harness; `atomic=true` batch `[INCR]` → action: run →
  assert: exactly one pipeline received (the atomic `EXEC` may have
  committed; not replayed because `INCR` is not idempotent).
- `batch_plain_disconnect_after_flush_is_outcome_unknown`: setup:
  kill-after-flush harness; `atomic=false` batch `[GET]` (idempotent) →
  action: run → assert: a second pipeline is sent after the kill and the run
  completes Ok.
- `live_multi_atomic_batch_roundtrip`: setup: `shared_redis()`; batch
  `[{"command":"set","key":"live:b1"},{"command":"incr","key":"live:b2"}]`,
  body `["v1", null]` → action: one exchange → assert: reply array has 2
  elements; follow-up `GET live:b1` is `v1` and `GET live:b2` is `1`.
- `live_multi_partial_entry_error_is_named`: setup: `live:b3` as a list via
  `LPUSH`; batch `[{"command":"incr","key":"live:b3"}]` (`INCR` on a list →
  `WRONGTYPE` at exec) → action: one exchange → assert: error names entry 0
  and `WRONGTYPE`.
- Command: `cargo test -p camel-component-redis batch_post_flush`
  then `cargo test -p camel-component-redis batch_pre_send`
  then `cargo test -p camel-component-redis batch_atomic_disconnect`
  then `cargo test -p camel-component-redis batch_plain_disconnect`
  then `cargo test -p camel-test --features integration-tests --test redis_test live_multi`
- Expected: unit tests fail before implementation; live tests need Docker.

**Acceptance:**
- `cargo test -p camel-component-redis` passes.
- `cargo test -p camel-test --features integration-tests --test redis_test` passes.
- `cargo test -p camel-test --features integration-tests --test redis_repositories_test` passes.
- `cd docs && mdbook test` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 3.3
