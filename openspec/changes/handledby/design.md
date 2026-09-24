# Design: handledby

## Approach

The runtime API is already correct: `ExceptionPolicy.handled_by`
(camel-api) sits at policy level beside `retry`/`on_steps`/`disposition`,
and `resolve_producer` resolves the delegate per policy regardless of
retry. The work is (1) moving the DSL surface to match, (2) fixing the
delegate-failure contract, per the e_opus sealed ruling (bd rc-ntpof,
2026-09-24).

**1. DSL layout (camel-dsl).**
- `model.rs`: `DeclarativeOnException` gains `handled_by: Option<String>`;
  `DeclarativeRedeliveryPolicy` LOSES it. `retry` becomes optional and
  independent of delegation.
- The serde surface is `route_ast.rs` — `RouteDslOnException` /
  `RouteDslRedeliveryPolicy` (~L401-436), shared by the YAML and JSON
  formats through the single `route_dsl_to_declarative_route`
  converter. The clause struct gains `handled_by`; the retry struct
  loses it. Both structs ALREADY carry `#[serde(deny_unknown_fields)]`,
  so the old `retry: {handled_by: ...}` layout becomes an unknown-field
  HARD load error in both formats with no new attribute (ruling
  scenario g). The yaml.rs/json.rs converter mappings move the field
  accordingly.
- `compile.rs` clause loop: read `clause.handled_by` and call
  `builder.handled_by(uri)` OUTSIDE the `if let Some(retry)` block, so
  retry and delegation compose: retry first, delegate once retries
  exhaust (Apache Camel `onException` parity, ADR-0019 L34). The
  top-level `error_handler.retry` path DROPS its `handled_by` handling
  (field deleted; `dead_letter_channel` is the catch-all delegate).
- Typed terminal conflict: `steps` + `handled_by` on one clause is
  rejected in `validate_error_handler` via a NEW
  `ConfigValidationError` variant in camel-api, returned through
  `CamelError::ConfigValidation` (detretry/sedaretry precedent — typed
  variant, never message-text matching).
- `validate_redelivery_policy` is unchanged: `max_attempts == 0` stays
  rejected (ruling rejects dishonest retry blocks).

**2. Delegate-failure contract (camel-processor error_handler.rs).**
- `send_to_handler`: delegate `ready()` Err and `call()` Err now return
  `Err(delegate_error)`. Log-policy system-broken with BOTH errors
  structured (original + delegate) and record the delegate error on the
  span. The `None`-producer case (no handler configured) keeps returning
  the exchange (log-only semantic — no delegate, no delegate failure).
- ALL step-path callers map delegate `Err` to
  `StepDisposition::Propagate(original error)`: the `handle_step` match
  (~L329), the retry-exhausted return (~L621), the `execute_on_steps`
  fallback (~L636), the no-retry path (~L641), the no-match DLC path, and
  the helper at ~L47 (audit: align or remove per its contract).
  `handle_boundary` (~L417) returns `Result<Exchange, CamelError>` — its
  delegate-failure arm returns `Err(original error)` directly. The
  current "dead code by construction" Err arms (~L344, ~L431) go LIVE.
  The original error stays the main error; the pipeline outcome is
  `Failed(original kind)` — kind matching and HTTP status stay
  business-tied. `Handled`/`Continued` dispositions with a failed
  delegate NEVER yield `Completed`. No new `CamelError` variant.
- Pipeline mapping `Propagate(err) → Failed(err)` already exists in
  SequentialPipeline/TracedPipeline — tests pin it.

**3. Tap semantics.** `handled_by` without `handled`/`continued`
defaults to `Propagate` disposition: the delegate fires as a side effect
and the original error propagates (scenario h). Existing default
disposition already is Propagate — pinned by test.

**4. do_try is DIFFERENT (recorded, not fixed).** `do_try/catch`
propagates the CATCH error and loses the original — that difference vs
`handled_by` (original wins) is recorded in the ADR-0019 amendment and
the error-handler delta text. do_try stays untouched (tracked rc-zgbqq).

## Affected crates

- `camel-api`: one `ConfigValidationError` variant (+ its error text).
- `camel-dsl`: model.rs, yaml.rs, json.rs, compile.rs (+ parse/compile
  tests ~2360/2966, yaml ~2493, json ~202 fixtures move to clause
  layout).
- `camel-processor`: error_handler.rs (send_to_handler + all callers) +
  unit tests for the new contract.
- `camel-test`: on_exceptions_wildcard_test.rs, do_try_test.rs (layout
  usages only), integration_test.rs — fixtures + scenario tests (a)-(h).
- Schema assets: run schema GENERATION to refresh the emitted JSON
  schema (handled_by moves), then verify with
  `cargo xtask schema --check`.

## Architecture boundaries

DSL (declarative surface) changes shape; Runtime (camel-processor)
changes error contract; camel-api gains one typed variant. No component,
transport, or test-runner changes. Zone lease respected: error_handler +
DSL compile.rs + dsl spec only.

## Phases

Omitted — single coherent slice; task order encodes dependencies
(runtime contract first, then DSL layout, then fixtures/specs/schema).
