# Tasks: exec-cli-startup-guard

<!-- Single-phase change (design.md has no ## Phases section). Flat task list. -->

## camel-core

### Task 1.1: Add reusable scheme-presence route scanner

**Files:**
- `crates/camel-core/src/startup_validation.rs` (modified)

**Steps:**
1. Refactor the private `fn walk_step_uris(step: &BuilderStep, out: &mut Vec<Box<dyn ConfigCheck>>)` into a generic `fn for_each_step_uri<F: FnMut(&str)>(step: &BuilderStep, f: &mut F)` that calls `f(uri)` for every statically declared URI in `step`. Keep the variant coverage and recursion identical to the current walker: `To`, `WireTap { uri }`, `Enrich { uri, .. }`, `PollEnrich { uri, .. }`, and recurse into the `steps` of `Filter`/`DeclarativeFilter`, `Split`/`DeclarativeSplit`/`DeclarativeStreamSplit`, `Multicast`, `Throttle`, `LoadBalance`, `Loop`/`DeclarativeLoop`, `IdempotentConsumer`; recurse into `Choice`/`DeclarativeChoice` `whens` (+`otherwise`) and `DeclarativeDoTry` `try_steps`/`catch`/`finally`. Dynamic-URI steps (routing slip / recipient list / dynamic router) remain in the `_ => {}` skip arm.
2. Rewrite `scan_route_definitions_for_sql_checks` to use the generic walker: for each route call `collect_sql_checks_for_uri(route.from_uri(), &mut out)` directly, then `for_each_step_uri(step, &mut |uri| collect_sql_checks_for_uri(uri, &mut out))` for each step. Delete the old `walk_step_uris`.
3. Add `pub fn route_definitions_reference_scheme(routes: &[RouteDefinition], scheme: &str) -> bool`. For each route: if the from-uri matches `scheme`, return `true`; otherwise walk each step with `for_each_step_uri` and return `true` if any visited URI matches. Match a URI by `camel_endpoint::parse_uri(uri).map(|p| p.scheme == scheme).unwrap_or(false)` (the same parse the SQL scanner uses). Return `false` when no route statically declares the scheme.
4. Ensure the function is reachable as `camel_core::startup_validation::route_definitions_reference_scheme` (the `startup_validation` module is already declared; just keep it `pub`).

**Tests:** (append to the existing `#[cfg(test)] mod tests`; build routes with `RouteDefinition::new(from, steps).with_route_id("r".to_string())` and `BuilderStep` variants — read the `BuilderStep` enum for exact field shapes)
- `scheme_scanner_detects_exec_from_uri`: from `exec:echo`, no steps → `route_definitions_reference_scheme(&[route], "exec") == true`.
- `scheme_scanner_detects_exec_in_to_step`: from `timer:tick?period=500`, step `BuilderStep::To("exec:echo".into())` → true.
- `scheme_scanner_detects_exec_in_wiretap`: a `BuilderStep::WireTap { uri: "exec:audit".into(), .. }` step → true.
- `scheme_scanner_detects_exec_in_enrich`: a `BuilderStep::Enrich { uri: "exec:enricher".into(), .. }` step → true.
- `scheme_scanner_detects_exec_in_pollenrich`: a `BuilderStep::PollEnrich { uri: "exec:poller".into(), .. }` step → true.
- `scheme_scanner_detects_exec_in_choice_branch`: from timer, a `BuilderStep::Choice { whens, .. }` whose first `when` contains a `To("exec:echo")` → true.
- `scheme_scanner_detects_exec_in_dotry`: a `BuilderStep::DeclarativeDoTry { try_steps, .. }` whose `try_steps` contain `To("exec:echo")` → true.
- `scheme_scanner_detects_exec_across_multiple_routes`: two routes, only the second has `To("exec:echo")` → true.
- `scheme_scanner_false_for_non_exec_route`: from timer, step `To("log:info")` → false.
- `scheme_scanner_false_for_dynamic_uri_only`: a route whose only exec reference is a dynamic-URI step (use whichever of routing-slip / recipient-list / dynamic-router exists in `BuilderStep`) → false.
- `scheme_scanner_detects_exec_in_filter`: a `BuilderStep::Filter { steps, .. }` whose `steps` contain `To("exec:echo")` → true.
- `scheme_scanner_detects_exec_in_split`: a `BuilderStep::Split { steps, .. }` whose `steps` contain `To("exec:echo")` → true.
- `scheme_scanner_detects_exec_in_multicast`: a `BuilderStep::Multicast { steps, .. }` containing `To("exec:echo")` → true.
- `scheme_scanner_detects_exec_in_loop`: a `BuilderStep::Loop { steps, .. }` containing `To("exec:echo")` → true.
- `scheme_scanner_detects_exec_in_idempotent_consumer`: a `BuilderStep::IdempotentConsumer { steps, .. }` containing `To("exec:echo")` → true.
- `scheme_scanner_detects_exec_in_throttle`: a `BuilderStep::Throttle { steps, .. }` containing `To("exec:echo")` → true.
- `scheme_scanner_detects_exec_in_loadbalance`: a `BuilderStep::LoadBalance { steps, .. }` containing `To("exec:echo")` → true.
- `scheme_scanner_detects_exec_in_declarative_filter`: a `BuilderStep::DeclarativeFilter { steps, .. }` containing `To("exec:echo")` → true.
- `scheme_scanner_detects_exec_in_declarative_split`: a `BuilderStep::DeclarativeSplit { steps, .. }` containing `To("exec:echo")` → true.
- `scheme_scanner_detects_exec_in_declarative_stream_split`: a `BuilderStep::DeclarativeStreamSplit { steps, .. }` containing `To("exec:echo")` → true.
- `scheme_scanner_detects_exec_in_declarative_loop`: a `BuilderStep::DeclarativeLoop { steps, .. }` containing `To("exec:echo")` → true.
- `scheme_scanner_detects_exec_in_choice_otherwise`: a `BuilderStep::Choice { whens, otherwise }` where the only `To("exec:echo")` is in `otherwise` → true.
- `scheme_scanner_detects_exec_in_declarative_choice_when`: a `BuilderStep::DeclarativeChoice { whens, otherwise }` with `To("exec:echo")` in a `when` branch → true.
- `scheme_scanner_detects_exec_in_declarative_choice_otherwise`: a `BuilderStep::DeclarativeChoice { whens, otherwise }` where the only `To("exec:echo")` is in `otherwise` → true.
- `scheme_scanner_detects_exec_in_dotry_catch`: a `BuilderStep::DeclarativeDoTry { try_steps, catch, finally }` where the only `To("exec:echo")` is in a `catch` clause's `steps` → true.
- `scheme_scanner_detects_exec_in_dotry_finally`: a `DeclarativeDoTry` where the only `To("exec:echo")` is in `finally`'s `steps` → true.
- Regression: existing sql scanner tests (`scanner_walks_top_level_to_sql_step`, `scanner_flags_from_sql_endpoint_with_body_but_no_allow`, `scanner_emits_nothing_for_non_sql_route`, `scanner_skips_sql_endpoint_without_dynamic_intent`) pass unchanged.

**Acceptance:**
- `cargo test -p camel-core --lib startup_validation` passes (new + existing scanner tests).
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo fmt --check --all` clean.

- [x] 1.1

## camel-cli

### Task 2.1: Gate ExecBundle registration on exec usage or declaration

**Files:**
- `crates/camel-cli/src/commands/run.rs` (modified)

**Steps:**
1. Remove the unconditional exec registration from the bundle-registration phase: delete the `#[cfg(feature = "exec")] register_bundle!(ctx, camel_config, camel_component_exec::ExecBundle);` statement that sits among the other feature-gated bundles (next to the `#[cfg(feature = "llm")]` / `#[cfg(feature = "wasm")]` block).
2. In the route-discovery `match` arm — the `Ok(defs)` branch (step 5) — at the TOP of the arm, before `scan_route_definitions_for_sql_checks(&defs)` and before `defs` is moved into `maybe_instrument_routes(defs)`, add:
   ```rust
   #[cfg(feature = "exec")]
   {
       let exec_used = camel_core::startup_validation::route_definitions_reference_scheme(&defs, "exec");
       let exec_configured = camel_config.components.raw.contains_key("exec");
       if exec_used || exec_configured {
           register_bundle!(ctx, camel_config, camel_component_exec::ExecBundle);
       }
   }
   ```
   Confirm the type of `camel_config.components.raw`: it is `HashMap<String, toml::Value>` (`crates/camel-config/src/config.rs`), so `camel_config.components.raw.contains_key("exec")` is the correct and only form. The whole block is `#[cfg(feature = "exec")]` so there are no unused-variable warnings when exec is disabled.
3. Leave `ExecBundle`, `ExecGlobalConfig::validate`, and the `register_bundle!` macro unchanged. Do not touch any other bundle registration.

**Tests:** (cfg compile-gate — proves the feature gating compiles both ways and the unused-variable hazard from step 2 cannot recur)
- `build_with_exec_feature`: `cargo build -p camel-cli --no-default-features --features exec` → exit 0.
- `build_without_exec_feature`: `cargo build -p camel-cli --no-default-features` → exit 0.
- Existing `cargo test -p camel-cli --lib` still compiles and passes.

**Acceptance:**
- Both build commands above exit 0.
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo fmt --check --all` clean.
- Behavioral correctness is verified by Task 2.2.

- [x] 2.1

### Task 2.2: End-to-end startup integration tests

**Files:**
- `crates/camel-cli/tests/run_exec_guard_test.rs` (new)

**Steps:**
1. Add two helpers. Both spawn `std::process::Command::new(env!("CARGO_BIN_EXE_camel"))` with working dir `dir`, args `["run", "--no-watch", "--config", "<dir/Camel.toml>"]`, and BOTH `stdout`/`stderr` set to `Stdio::piped()`. To avoid pipe-fill deadlock, spawn TWO reader threads that concurrently drain `child.stdout` and `child.stderr` into one shared `Arc<Mutex<String>>` buffer (append-only); the main thread polls that buffer. Always `join` both reader threads before returning or asserting.
   - `fn run_expect_exit(dir: &Path, timeout: Duration) -> (i32, String)`: poll `child.try_wait()` in a loop (with short sleeps) up to `timeout`; when the child exits on its own, join readers and return `(exit_code, buffer)`. If it is still alive at `timeout`, `child.kill()` + `child.wait()` (reap), join readers, and return `(-1, buffer)` (the test then fails its assertion). Used for the abort cases (CLI exits non-zero on its own).
   - `fn run_observe_then_signal(dir: &Path, observe: &str, timeout: Duration) -> (i32, String)`: poll the shared buffer until it contains `observe` (cap `timeout`); then send exactly ONE SIGTERM via `std::process::Command::new("kill").arg("-TERM").arg(child.id().to_string()).status()` (unix; the CLI's `tokio::select!` handles SIGTERM → graceful `Ok(())` → exit 0). Then `child.wait()` with a BOUNDED deadline (e.g. 10s); on the deadline, `child.kill()` + reap. Join readers and return `(exit_code, buffer)`. Used for the success cases — `observe` reaching proves startup (+ exchange processing when it is a route log line); exit code 0 proves graceful signal-driven shutdown.
2. Write each fixture (Camel.toml + `routes/*.yaml`) into a fresh `tempfile::TempDir` per test (camel-cli already has `tempfile` as a dev-dependency). Camel.toml uses the `[default]` profile with `routes = ["routes/*.yaml"]`.

**Tests:**
- `non_exec_route_starts_without_exec_config`: fixture Camel.toml with NO `[components.exec]`; `routes/hello.yaml` = route `from: timer:tick?period=300`, step `log: "non-exec-tick-ok"`. Action: `run_observe_then_signal(dir, "non-exec-tick-ok", 4s)`. Assert: exit code == 0 AND the captured output contains `"non-exec-tick-ok"` (an exchange was processed) AND output does NOT contain `"no profiles configured"`.
- `exec_route_without_profiles_aborts`: `routes/exec.yaml` = route `from: timer:tick?period=300`, step `to: exec:echo`; Camel.toml with no exec profiles. Action: `run_expect_exit(dir, 4s)`. Assert: exit code != 0 AND output contains `"no profiles configured"`.
- `explicit_empty_exec_config_aborts`: `routes/hello.yaml` = timer→log (no exec usage); Camel.toml with `[default.components.exec]` + `workspace_root = "."` and ZERO profiles. Action: `run_expect_exit(dir, 4s)`. Assert: exit code != 0 AND output contains `"no profiles configured"`.
- `exec_route_with_profile_starts`: `routes/exec.yaml` = route `to: exec:echo`; Camel.toml defining an `echo` profile (`executable = "echo"`, `args = { allow = "any" }`, `timeout_secs = 5`, `accepted_exit_codes = [0]`). Action: `run_observe_then_signal(dir, "context started", 4s)`. Assert: exit code == 0 AND output contains `"context started"` AND does NOT contain `"no profiles configured"`.

**Acceptance:**
- `cargo test -p camel-cli --test run_exec_guard_test` passes.
- `cargo clippy -p camel-cli --tests -- -D warnings` exits 0.

- [x] 2.2

## examples

### Task 3.1: Add camel-cli-no-exec example + workspace exclude

**Files:**
- `examples/camel-cli-no-exec/Camel.toml` (new)
- `examples/camel-cli-no-exec/routes/hello.yaml` (new)
- `examples/camel-cli-no-exec/README.md` (new)
- `Cargo.toml` (modified — workspace `exclude` array)

**Steps:**
1. Create `examples/camel-cli-no-exec/Camel.toml`: `[default]` with `routes = ["routes/*.yaml"]`, `log_level = "INFO"`, and NO `[components.exec]`.
2. Create `examples/camel-cli-no-exec/routes/hello.yaml`: route `id: hello`, `from: timer:tick?period=2000`, step `log: "Hello without exec! Exchange #${header.CamelTimerCounter}"`.
3. Create `examples/camel-cli-no-exec/README.md` documenting: purpose (run a route via `camel run` without exec); the pre-fix bug and that it now starts on a default-features build; reproduce command (`cargo build -p camel-cli` then `cd examples/camel-cli-no-exec && ../../target/debug/camel run --no-watch`); one-line root cause; and the hot-reload known limitation (first exec usage introduced only via reload requires a restart; exec configured at startup permits later exec routes).
4. Add `"examples/camel-cli-no-exec"` to the workspace `exclude` array in the root `Cargo.toml`, adjacent to `"examples/camel-cli-run"` (this example has no Cargo.toml — CLI-run style).

**Tests:**
- `workspace_metadata_valid`: `cargo metadata --no-deps --format-version 1 >/dev/null` → exit 0 (the new exclude entry does not break the workspace).
- `example_files_present`: the dir contains exactly `Camel.toml`, `routes/hello.yaml`, `README.md`.
- Behavioral coverage of "timer→log starts without exec config" is provided automatically by Task 2.2's `non_exec_route_starts_without_exec_config`; this task is the human-facing example + docs.

**Acceptance:**
- `cargo metadata --no-deps` exits 0.
- The example dir contains exactly the three files above.
- `cargo build -p camel-cli` then `camel run --no-watch` in the example dir starts the route (manual mirror of 2.2's automated check).

- [x] 3.1
