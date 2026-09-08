# Tasks: scenario-tier-p3-sweep

## Phase 1: Sweep implementation

- [x] 1.1

### Task 1.1 — Split DocError and conversion helpers into document/error.rs (rc-0ahfl, pure move)

**Files**
- `crates/camel-integration-test/src/document/error.rs` (new)
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/lib.rs` (unchanged — verify, do not edit)

**Steps**
1. Create `crates/camel-integration-test/src/document/error.rs` containing,
   moved VERBATIM from `document.rs`: the `pub enum DocError` (currently at
   document.rs:519) with every impl block on it, and the conversion helper
   `fn endpoint_from_raw(raw: RawEndpointRef) -> Result<EndpointRef, DocError>`
   (currently document.rs:1146) plus any private helper whose sole job is
   constructing `DocError` values (walk the fns between :519 and :683 and
   :1146 to file end; anything that only builds/errors-with `DocError`
   moves, struct-building helpers stay).
2. In `document.rs` add `pub mod error;` and re-export so every existing
   path keeps resolving: `pub use error::DocError;` (and `use
   error::endpoint_from_raw;` keeping its current private-to-module
   visibility). `lib.rs`'s `pub use document::{...}` must resolve unchanged.
3. Verify zero API change: `cargo build -p camel-integration-test` and
   `cargo build -p camel-cli` compile with no edits outside the two files.

**Tests** (pure move — existing suite is the test)
- name: existing suite untouched
- setup: the split landed
- action: run the crate's full test surface
- assert: every pre-existing test passes with NO test-file edits
- command: `cargo test -p camel-integration-test --lib && cargo test -p camel-integration-test --tests`
- expected: pass before and after the move (if a test needed editing, the
  move was not pure — fix the move, not the test)

**Acceptance**
- `document.rs` drops to under 1200 lines (`wc -l` in the task report).
- `git diff` shows only relocations of identical content (no logic edits).
- `cargo clippy -p camel-integration-test --all-targets -- -D warnings` exits 0.

- [x] 1.2

### Task 1.2 — Log-content assertion vocabulary (rc-tdgh5)

**Files**
- `crates/camel-integration-test/src/log_capture.rs` (new)
- `crates/camel-integration-test/src/lib.rs` (modified: `pub mod log_capture;` + pub use of `LogEvent`, `WindowHandle`, `ensure_capture_subscriber`)
- `crates/camel-integration-test/Cargo.toml` (modified: add `tracing-subscriber` dependency, `registry` feature at minimum — the crate has no tracing-subscriber dep today)
- `crates/camel-integration-test/src/document.rs` (modified: `ScenarioDocument` gains `pub logs: Option<LogsAssertion>`; raw struct + parse)
- `crates/camel-integration-test/src/document/error.rs` (modified: new `DocError` logs-block variants)
- `crates/camel-integration-test/src/runner.rs` (modified: window open/evaluate/close in `run_scenario_document`; new `ScenarioFailure::LogCaptureUnavailable`; `DocumentOutcome` logs-failure channel)
- `crates/camel-cli/src/commands/test/scenario.rs` (modified: one `ensure_capture_subscriber()` call at the integration-tier document loop entry, BEFORE the first `boot_scenario` — the CLI driver seam)
- `crates/camel-integration-test/tests/common/mod.rs` (new: shared harness — install-first run helper + a process-global serialization `static Mutex` for document runs)
- `crates/camel-integration-test/tests/log_assertion_test.rs` (new)
- `crates/camel-integration-test/tests/log_foreign_subscriber_test.rs` (new — OWN binary: it installs a foreign subscriber that must not poison the capture tests' process)
- `crates/camel-integration-test/tests/fixtures/logs/marker-info.routes.yaml` (new: route with a `to: log:marker?level=INFO` step)
- `crates/camel-integration-test/tests/fixtures/logs/marker-warn.routes.yaml` (new: route with a `to: log:marker?level=WARN` step)
- `crates/camel-integration-test/tests/fixtures/logs/processor-marker.routes.yaml` (new: route with a `to: log:marker?level=INFO` step whose body marker is `processor X emitted order` — the regex test's body source)

**Steps**
1. `log_capture.rs`: `pub struct LogEvent { pub at: std::time::Instant, pub level: tracing::Level, pub target: String, pub message: String }`. A `tracing_subscriber::Layer` impl that appends every event to every open window whose `[opened_at, now)` range contains the event timestamp. `pub struct WindowHandle` (id + `Arc<Mutex<Vec<LogEvent>>>` buffer, cap `LOG_WINDOW_CAP: usize = 10_000`, drop-oldest with a recorded marker event). Window registry: process-global, `Mutex<Vec<(id, opened_at, buffer)>>`. `pub fn ensure_capture_subscriber()` composes `tracing_subscriber::registry().with(capture_layer).try_init()`; an `AtomicBool` marks own-install success; a later call is idempotent when the bool is set. NOTE: the camel-log endpoint renders composite messages (`format_exchange`, e.g. an exchange prefix + `Body: <body>`), and takes its level from the `level` URI parameter (UPPERCASE values; the URI path is the category) — the `contains` markers below are substrings that survive composite rendering.
2. `document.rs`: `pub struct LogsAssertion { pub contains: Vec<String>, pub regex: Vec<String>, pub no_level_above: Option<LogLevel> }` (serde rename to camelCase `noLevelAbove`; `#[serde(deny_unknown_fields)]`). Parse-time validation: `LogLevel` accepts exactly `trace|debug|info|warn|error`; each `regex` entry compiles via `regex::Regex::new`. Any violation — unknown key, bad level value, invalid pattern — is a LOAD error from `parse_scenario_document` via a new `DocError::LogsBlock { detail: String }` variant naming the offending clause (spec scenario "malformed logs block is a load error").
3. Driver seams install FIRST (before any boot): (a) `tests/common/mod.rs` exposes a run helper that calls `camel_integration_test::ensure_capture_subscriber()` before constructing partners/booting; (b) `crates/camel-cli/src/commands/test/scenario.rs` calls `ensure_capture_subscriber()` at the integration-tier document loop entry before the first `boot_scenario`. The composition root's own install keeps losing to the capture layer via its existing warn-and-skip path.
4. `runner.rs` `run_scenario_document`: open a window at document start — if `ensure_capture_subscriber`'s AtomicBool is false (a foreign subscriber holds the global seat), the document fails through a NEW apparatus-class variant `ScenarioFailure::LogCaptureUnavailable { detail: String }` (the enum is `#[non_exhaustive]`; message names the foreign-subscriber condition). After the action loop (only when no action failed), evaluate the `logs:` block: every `contains` entry appears in ≥1 window event message; every `regex` matches ≥1; when `no_level_above` is set, no event level is above it. Conjunction across clauses. A violation is a verdict-class failure carried by a NEW `DocumentOutcome` field `pub logs_failure: Option<String>` (diagnostic naming the violated clause; for `no_level_above`, listing each offending event's level, target, message) with `verdict: None`. Close the window after evaluation.
5. Integration tests per the spec scenarios (names below). `tests/common/mod.rs` holds `pub static RUN_LOCK: Mutex<()>` — every test in `log_assertion_test.rs` takes it for the whole document run, serializing windows inside the binary (parallel cross-talk is the documented conservative semantics; the tests pin the deterministic serialized behavior, and `concurrent_windows_attribute_conservatively` pins the conservative rule at unit level).

**Tests** (in `log_assertion_test.rs` unless noted; fixtures use `to: log:marker?level=<LEVEL>` so the composite message carries the body marker and the level is WARN/INFO as declared; command for every test below: `cargo test -p camel-integration-test --test log_assertion_test` filtered by the test name; expected: fails before the implementation steps land, passes after — the per-test `expected` is omitted when it repeats exactly this)
- name: `contains_passes_on_window_event` — setup: `marker-info.routes.yaml` route logs body marker `cache served HIT`; action: run a document whose `logs.contains` lists that exact string; assert: document passes; command: `cargo test -p camel-integration-test --test log_assertion_test -- contains_passes`; expected: fails before step 4, passes after.
- name: `contains_fails_without_match` — same fixture, `contains: ["cache served Miss"]`; assert: document fails naming the unsatisfied entry (`logs_failure` Some).
- name: `regex_matches_window_event` — setup: `processor-marker.routes.yaml`; `regex: ["processor .*emitted"]` (UNANCHORED — camel-log renders a composite message, the body never starts the line); assert: passes.
- name: `no_level_above_clean_window_passes` — info-only fixture, `noLevelAbove: info`; assert: passes.
- name: `no_level_above_fails_on_route_warn` — `marker-warn.routes.yaml` (`to: log:marker?level=WARN`), `noLevelAbove: info`; assert: document fails, `logs_failure` diagnostic carries the warn level, the camel-log target, and the marker message.
- name: `spawned_task_events_counted` — unit test, `log_capture.rs` inline `#[cfg(test)]`, `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`: open a window via the registry, `tokio::spawn` a task emitting `tracing::warn!("lane overflow marker")`, await it, close; assert: the event is in the window buffer (mechanism-level witness for harness-task counting + thread-agnostic capture).
- name: `multi_thread_document_fails_on_warn` — document-level witness for both scenarios: `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`; while the `marker-warn` document with `noLevelAbove: info` is running (window open, actions in flight), the test `tokio::spawn`s a task emitting `tracing::warn!("harness task warn marker")` and awaits it before the document completes; assert: `logs_failure` is Some naming BOTH the route's warn and the spawned task's warn — the runner's evaluation saw the worker-thread + harness-task emissions, and the scenario's THEN (document fails) is witnessed at document level.
- name: `no_level_above_empty_window_passes` — document whose run emits nothing (no route trigger), `noLevelAbove: warn`; assert: passes (vacuous).
- name: `outside_window_never_satisfies` — test emits `tracing::info!("outside marker")` BEFORE running a document whose `contains: ["outside marker"]`; assert: document fails.
- name: `sequential_documents_capture_persists` — run document A then document B in one test, each with a distinct marker + `contains`; assert: both pass, no panic (boot's warn-and-skip path exercised by B's boot).
- name: `concurrent_windows_attribute_conservatively` — unit test in `log_capture.rs` inline `#[cfg(test)]`: open two windows, record one event through the layer, assert: both buffers contain it.
- name: `foreign_subscriber_is_apparatus_error` — in `tests/log_foreign_subscriber_test.rs` (own binary): the test first installs a plain `tracing_subscriber::fmt().try_init()` (foreign seat win), then runs a document with a `logs:` block through the common helper; assert: the document's outcome carries the `LogCaptureUnavailable` apparatus failure, never a silent pass; command: `cargo test -p camel-integration-test --test log_foreign_subscriber_test`.
- name: `malformed_logs_block_load_errors` — three `parse_scenario_document` cases: unknown key (`logz:`), bad level (`noLevelAbove: verbose`), invalid regex (`"["`); assert: `DocError::LogsBlock` load error naming the clause; command: `cargo test -p camel-integration-test --lib -- log_capture && cargo test -p camel-integration-test --test log_assertion_test`.

**Acceptance**
- All 13 scenarios of the ADDED requirement are witnessed by the named tests above (map them in the task report; malformed-load-error is one of the 13).
- `cargo test -p camel-integration-test --lib --tests` exits 0 (both log test binaries included).
- `cargo test -p camel-cli` exits 0 (the scenario.rs seam compiles and existing CLI tests stay green).
- No `std::env::var` introduced in new code (ADR-0069; the ambient RUST_LOG read stays untouched inside camel-config).
- `cargo clippy -p camel-integration-test --all-targets -- -D warnings` and `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 1.3

### Task 1.3 — resolve_url bridge arm verbatim assembly (rc-vf7z7)

**Files**
- `crates/components/camel-http/src/lib.rs` (modified: bridge arm in `resolve_url` :2499 ff., its comment, and the resolve_url test module)

**Steps**
1. Replace the bridge arm's `url::Url::parse(&config.base_url) → set_query → to_string` with string assembly mirroring the `CamelHttpUri` arm: split the base at the first `?` if any, keep both parts verbatim, append `?` + resolved query using the same marker rules as the other arms (authored empty marker preserved; no dangling `?` when the resolved query is empty).
2. KEEP the `url::Url::parse` call for validation only: a malformed base still errors through the existing redacted-diagnostic path (rc-ph7z2 pin, scenario "Malformed base URL errors instead of panicking") — the parsed value is never re-emitted.
3. Replace the stale comment claiming "the Url base normalization the bridge pins expect" with verbatim-intent wording (authored bytes end-to-end, papal Direction A).
4. Flip existing bridge pins that assert normalized output (for example the expectation `http://x/?token=secret` with its inserted `/`) to verbatim expectations.
5. Add the new tests named below.

**Tests** (command for every test below: `cargo test -p camel-component-http --lib -- resolve_url_bridge`; expected: fails before step 1, passes after — omitted per-test when it repeats exactly this)
- name: `resolve_url_bridge_preserves_dot_segments` — setup: bridged config base `http://h/a/../b` with `query_params [("k","1")]`; action: `HttpProducer::resolve_url` with a bare exchange; assert: URL is `http://h/a/../b?k=1` — path bytes unchanged.
- name: `resolve_url_bridge_preserves_default_port` — base `http://h:80/p`; assert: emission keeps `:80`.
- name: `resolve_url_bridge_preserves_scheme_and_host_case` — base `HTTP://ExAMPLE.COM/p`; assert: emission keeps the case verbatim.
- name: `resolve_url_bridge_no_query_emits_base_verbatim` — bridged base `http://h/p`, no raw query, no query_params; assert: exactly `http://h/p`, no dangling `?`.
- name: `resolve_url_bridge_and_non_bridge_byte_identical` — same base `http://H:80/a/../b` and same effective query: one exchange under `bridgeEndpoint=true`, another through the `CamelHttpQuery` composition path with equivalent pairs; assert: both emitted strings are equal (cross-arm byte identity).
- regression guards unchanged and green: `resolve_url_raw_wrapper_not_re_encoded`, `resolve_url_authored_and_programmatic_merge`, `resolve_url_preserves_empty_query_marker`, `resolve_url_malformed_base_url_errors_no_panic`.

**Acceptance**
- All 5 new MODIFIED-requirement scenarios witnessed by the named tests.
- `cargo test -p camel-component-http --lib` exits 0.
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0.

- [x] 1.4

### Task 1.4 — Redact the ParsedTarget empty-path error echo (rc-o072s)

**Files**
- `crates/camel-integration-test/src/adapters/http.rs` (modified)

**Steps**
1. Change `ParsedTarget::parse(endpoint: &str)` to take a second argument
   carrying the secret-key set — use the exact existing secret-key type the
   wire-path redaction in `adapters.rs` (~:760) already consumes (inspect
   that call site and match its parameter type verbatim; do not invent a
   new type).
2. Apply the masking INSIDE the `invalid` closure so every declaration echo
   redacts — the empty-path error, and also the `invalid uri` and
   `unsupported scheme` echoes: sensitive keys keep their raw key span,
   values are masked with the existing helper's marker (the `***` masking
   the wire-path diagnostics already print).
3. Update every `ParsedTarget::parse` call site (find with
   `rg 'ParsedTarget::parse'`) to pass the caller's already-available
   secret-key set — never an empty default that silently disables
   redaction.
4. Delete the self-disclosing stale redaction comment above the empty-path
   error return.
5. Add the test below (inline test module of `adapters/http.rs`).

**Tests**
- name: `empty_path_error_redacts_secret_query_value` — setup: secret-key set containing `authPassword`; action: `ParsedTarget::parse("http://host?authPassword=x", &keys)`; assert: returns the apparatus error AND the rendered message contains `authPassword=` followed by the existing redaction marker, and does NOT contain the raw `authPassword=x`; command: `cargo test -p camel-integration-test --lib -- empty_path_error`; expected: fails before (raw echo), passes after.
- name: `empty_path_error_keeps_plain_query_diagnostic` — no secret keys configured, target `http://host?flag=a`; assert: the error names `?flag=a` unchanged (diagnostic shape preserved).

**Acceptance**
- Spec scenario "Empty-path error redacts authored secret query values"
  witnessed by `empty_path_error_redacts_secret_query_value`.
- All `ParsedTarget::parse` call sites pass a real secret-key set (list them
  in the task report).
- `cargo test -p camel-integration-test --lib --tests` exits 0.

- [x] 1.5

### Task 1.5 — Multi-path partner pattern documentation + runnable example (rc-2miu)

**Files**
- `docs/src/testing/index.md` (modified: "Scenario documents" section)
- `examples/integration-testing/partner-multi-path.test.yaml` (new)
- `examples/integration-testing/partner-multi-path.routes.yaml` (new)
- `examples/integration-testing/README.md` (modified: one entry line)
- `crates/camel-integration-test/CONTEXT.md` (modified: arrival-lane entry)
- `crates/camel-integration-test/README.md` (modified: grammar-area sentence)

**Steps**
1. `docs/src/testing/index.md` "Scenario documents" section: new passage —
   ONE declared endpoint + ONE `bindVar` (e.g. `MOCK`), route `to:` URIs
   sharing that authority with distinct paths, scripted responses
   discriminating per path, dynamic-reference receives on the DECLARED
   path with sibling paths asserted through exact-count partner validates
   with `path` filters (empirically verified: a dynamic receive naming a
   sibling path drains the declared lane — `lane_key_for` resolves the
   reference to the registered key wholesale; per-path receives are the
   rc-ps97b follow-up); state the two-key rule (declared provisioning key
   vs dynamic authority key; arrivals queue per path on the single
   listener); name the N-bindVar fan-out as the anti-pattern citing the
   2026-09-06 pilot incident (spurious port reassignment); add the
   keep-going migration note: one scenario document per independent
   assertion chain — documents run independently and the runner continues
   past a failing document (the honest replacement for run.sh counting
   every assert; ADR-0069 section 11 ordered-action-lists is a
   pin-invariant, no amendment). Link the example pair.
2. `partner-multi-path.routes.yaml`: a route sending to two paths on one
   `${env:MOCK}` authority (e.g. `/orders` and `/billing`).
3. `partner-multi-path.test.yaml`: one declared harness endpoint with
   `bindVar: MOCK` (self-declared on the first validate's object-form
   partner target), per-path `partners:` script entries, a
   dynamic-reference receive on the declared path (`from:
   http://${MOCK}/orders`), and exact-count partner validates with `path`
   filters covering the sibling path (`/billing`).
4. `examples/integration-testing/README.md`: list the new pair.
5. `crates/camel-integration-test/CONTEXT.md` arrival-lane entry: add the
   one-listener-per-authority sentence (a dynamic reference resolves the
   registered partner by authority with the path preserved; lanes exist per
   path on that listener).
6. `crates/camel-integration-test/README.md`: next to the existing
   "scenario = authority, route env = full URI" line (:239), add the
   multi-path pattern sentence with a pointer to the example pair.
7. `docs/src/testing/index.md` "Scenario documents" section: document the
   `logs:` block grammar — `contains` (substrings of captured event
   messages), `regex` (unanchored patterns over composite camel-log
   messages), `noLevelAbove` (level cap over the whole document window,
   evaluated after the action list) — including the concurrency caveat and
   its escape hatch: under concurrent documents in one process, events are
   attributed conservatively (a sibling's WARN can fail a document's cap);
   serialize log-asserting documents or keep them on the current-thread
   itest path for strict isolation.

**Tests**
- name: example pair runs green
- setup: the two new example files
- action: run the scenario document through the test driver in the worktree
  (the same invocation the examples README documents for the
  partner-retry pair)
- assert: the document passes (exit 0); one listener serves both dial
  paths, the declared-path receive drains its lane, and the sibling path
  is asserted through the per-path count validate
- command: the README-documented `camel test` invocation against
  `examples/integration-testing/partner-multi-path.test.yaml` (run in the
  worktree; record exact command + exit code in the task report)
- expected: fails before the files exist (no document), passes after

**Acceptance**
- The docs passage, README lines, and CONTEXT.md entry exist and
  `cargo xtask lint-context-citations` exits 0.
- `cargo xtask schema --check` exits 0.
- The example-pair run's exit code 0 is recorded in the task report.
