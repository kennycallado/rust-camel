# Design: stream-component

## Approach

One new component crate follows the sibling convention exactly (`camel-timer` consumer shape, `camel-log` producer shape). No runtime, core, DSL, or `BuilderStep` changes — the ruling mandates a pure URI endpoint (declaration lives in the component URI, never a new step; consistent with `RULING-camel-job-orientation.md` §1).

**Crate:** `crates/components/camel-stream`, package `camel-component-stream`, `version.workspace = true` (=0.49.0 pin), `[lints] workspace = true`. Deps: `camel-api`, `camel-component-api`, `async-trait`, `tokio`, `tower`, `tracing` (mirrors camel-log/camel-timer; `lint-component-deps` compliant — no `camel-core`).

**Config:** `#[derive(Debug, Clone, UriConfig)] #[uri_scheme = "stream"] #[uri_config(skip_impl, metadata(scheme = "stream", description = "stdio data-plane adapter", producer, consumer), crate = "camel_component_api")]` + manual `UriConfig` impl (TimerConfig pattern). Path segment selects the fd: `out` (producer, fd 1), `err` (producer, fd 2), `in` (consumer, fd 0); any other path → `CamelError::InvalidUri` at construction (fail-closed, testable).

**Producers (Phase 1):** `StreamProducer: Service<Exchange>` (LogProducer pattern) returned as `BoxProcessor`. Body materialization via `Body::into_bytes(max_size)` (`crates/camel-api/src/body.rs:215` — the same API `camel-file` uses at `camel-file/src/lib.rs:2368`): `String`/`&str`/`Vec<u8>` write verbatim, structured bodies serialize through the body's byte conversion. Materialization bound: 100 MiB, mirroring `camel-file`; exceeding the bound or a conversion failure propagates as the producer's `CamelError` (exchange fails, nothing partial is written). `appendNewline=true` (default): body + one `\n`. `appendNewline=false`: verbatim bytes. Writes go through `tokio::io::stdout()`/`stderr()` behind a per-fd static `tokio::sync::Mutex` (serialize interleaving, R10) with flush per exchange. No level, no category, no redaction — a DATA sink; redaction is the route's job (ADR-0076 redaction plane boundary).

**Tracer collision (R3):** `camel-bundles` gains `pub fn warn_stream_stdout_collision(routes, camel_config) -> bool` — single-source helper (has `camel-core` `RouteDefinition` + `camel-config` tracer cfg). The CLI boot paths that own both facts (`camel run`; `camel job` in Phase 2) call it after route load, before start; it emits `warn!` when any route uses a `stream:out` endpoint AND `[observability.tracer.outputs.stdout] enabled && tracer.enabled` (`camel-config/src/context_ext.rs:791` condition). No auto-mux, no silent reconfiguration.

**Consumer (Phase 2):** `StreamConsumer: Consumer` (TimerConsumer pattern): `tokio::select!` loop over `cancel_token.cancelled()` and byte-oriented async reads (R1: never a blocking read). Line mode uses `read_until(b'\n')` (byte-oriented — never `read_line`/`String`-typed reads, so non-UTF-8 bytes are observable), then validates UTF-8; raw uses `read_to_end`; fixed uses `read_exact` with the final partial chunk emitted as-is. One frame → `Exchange::new(Message::new(...))` → `context.send(...).await`; send error → `increment_errors(route_id, "b-prime:stream:fire-send")` + break (ADR-0012 b′). Sequential backpressure is the loop shape: next read starts only after send completes; no queue. EOF → break → `Ok(())` — route completes gracefully; no-TTY/no-piped-input reads EOF immediately → zero exchanges → exit 0 (R2, the load-bearing test). Never `isatty` (R2/R7). Framing: `frame=line` (default; strips the terminator — trailing `\n` and a preceding `\r` — body is the line content), `frame=raw` (whole stdin → one exchange at EOF; empty input → zero exchanges), `frame=fixed&size=N` (N bytes → one exchange; final partial chunk → one exchange; missing or zero `size` → `CamelError::InvalidUri` at construction). `charset=utf-8` accepted on both producer and consumer, utf-8 the only v1 value (other values → `CamelError::Config` at construction); invalid UTF-8 in line mode → error metric + skip line + continue (ADR limitation); raw/fixed carry bytes verbatim.

**Registration:** `camel-bundles` Cargo.toml `camel-component-stream.workspace = true` (unconditional — slim, R9), `ctx.register_component(camel_component_stream::StreamComponent::new())` beside timer/log (`lib.rs:~300`), slim-test scheme list += `stream`. Root `Cargo.toml` `[workspace.dependencies]` entry with `=0.49.0` pin. `camel-cli` mirrors siblings with a direct dep only if compilation requires it. No feature gate → `lint-gate-forwarding` N/A by construction.

**Job allowlist (Phase 2) — normative lifecycle boundary:** `JOB_SAFE_CONSUMER_SCHEMES` += `"stream"` AND `validate_consumer_uri` restricts scheme `stream` to path exactly `in` — `from(stream:out)` fails closed at load (exit code 2) with an error naming `stream:in`. This change delivers the allowlist entry and load-gate tests ONLY. Job live-source wait mechanics are OUT OF SCOPE and stay with the job epic (rc-d5dgc coordination note): today `execute_job` performs the one configured send and then tears down (`commands/job/mod.rs`), so allowing `stream:in` does NOT by itself make a job's lifetime follow stdin to EOF. End-to-end pipe/EOF parity (`echo x | <runner> route`) is therefore verified and demonstrated on the runner that already hosts live consumers — `camel run` — where the composition works with zero runner changes. ADR-0080 records this boundary; the interactive example uses `camel run`.

**ADR-0080** (`docs/adr/`, accepted-limitations): the four REJECTED items + ADR-0060:108 distinction (own fd 0/1/2, not child supervision); un-redacted egress is operator-owned trusted egress (future optional `?mask=true` reusing the `log` masker — not v1); tracer-collision warn posture; charset/UTF-8 line-mode skip semantics; job-wait boundary.

**Known mission-order conflict — deptree gate (decision point):** slim inclusion (ruling Q5, BINDING) puts `camel-component-stream` into camel-cli's default dependency closure, so the golden fixture gate (`crates/camel-cli/tests/feature_profiles.rs` vs `tests/fixtures/default-deptree.txt`, which asserts exact equality of the normalized default closure) WILL fail with the new crate's lines — regenerating the fixture is the only way to make it green, and mission 124 explicitly forbids regeneration ("green WITHOUT regen ... if they do, STOP and report"). The two binding instructions are irreconcilable at the mechanical level; this change does not resolve the conflict and does not claim the gate green. Handling per the mission's own instruction: build per the ruling (slim, no feature gate); at gate time run the deptree test; verify the drift consists ONLY of `camel-component-stream` lines (no third-party creep — the property the fixture protects, per `cli-feature-profiles` spec the fixture is a closure change-detector); do NOT regenerate; execute STOP-and-report: the change parks with the conflict flagged for the human, whose decision is either (a) regenerate the fixture (documented one-command regen at `feature_profiles.rs:16-19`) or (b) feature-gate `stream` (contradicts ruling Q5).

## Affected crates

- `crates/components/camel-stream` (NEW): component, config, producers, consumer, conformance tests, CONTEXT.md, README.md.
- `crates/camel-bundles`: unconditional dep + cascade registration + slim test + `warn_stream_stdout_collision` helper.
- `crates/camel-cli`: job allowlist (`commands/job/document.rs`), collision-warn call at boot seams; (run seam as located by task).
- Root `Cargo.toml`: workspace dependency entry.
- `docs/adr/0080-stream-component-stdio-data-plane.md`, `examples/` interactive example, `openspec/specs/cli-jobs` delta.

## Architecture boundaries

Components → Runtime only: the crate implements `Component`/`Endpoint`/`Producer`/`Consumer` from `camel-component-api` and touches fds; the Runtime, DSL, and `BuilderStep` are untouched. Data plane (bodies to fds) vs control plane (leveled/redacted tracing via `log:`) stay separate — that separation IS the feature. Job safety stays at the CLI load gate (fail-closed allowlist), not in the component. Security posture: operator config trusted; `to(stream:out)` is a trusted egress decision (CONTEXT-MAP Exchange-data trust boundary).

## Phases

### Phase 1: Producers `stream:out`/`stream:err` (slim)
- **Goal:** fill the log-only output gap; data to fd 1/2.
- **Dependencies:** none (ruling §4 build-first).
- **Externally-visible types/interfaces:** `StreamComponent`, `StreamConfig` (pub), `stream:out`/`stream:err` endpoints; `camel_bundles::warn_stream_stdout_collision`.
- **Deliverable:** crate + registration + ADR-0080 + collision warn + tests + CONTEXT/README.
- **Exit-criteria:** line/raw framing, body coercion, flush-per-exchange, no-redaction, collision-warn, slim-registration tests green; touched-crate clippy `-D warnings`; deptree drift check recorded (expected-lines-only) without regen.

### Phase 2: Consumer `stream:in` + job allowlist + example
- **Goal:** stdin as an exchange source; EOF-graceful; job gate entry.
- **Dependencies:** Phase 1; rc-d5dgc settled (job-args landed).
- **Externally-visible types/interfaces:** `stream:in` endpoint (`frame=line|raw|fixed`, `size`, `charset`); allowlist += `stream` (path-checked `in`).
- **Deliverable:** consumer + gate change + cli-jobs spec MODIFIED + interactive example (`camel run`) + integration tests.
- **Exit-criteria:** one-line-one-exchange, EOF-graceful (load-bearing no-TTY test), backpressure, `from(stream:out)` load rejection (exit 2), `echo x | camel run` end-to-end parity verified by test, example compiles and is verified by test.

## Alternatives considered

- **DSL `read` step / `set-prompt` step / REPL state:** REJECTED by ruling (human-coupled blocking, shell-script-in-route-costume; Q2).
- **`file:/dev/stdout` workaround:** obscure, platform-fragile; the gap is real (Q1 evidence).
- **Producer-side redaction / auto-mux of tracer output:** REJECTED — a data sink that auto-redacts silently corrupts output; silent reconfiguration violates fail-closed posture (Q4/Q7).
- **Feature-gated (non-slim) shipping:** contradicts ruling Q5 (same weight class as timer/log; zero bridges interaction); retained only as the human's fallback option on the deptree conflict.
- **Absorb into job epic rc-d5dgc:** REJECTED by ruling Q6 — orthogonal risk surfaces, independently reviewable.
