# Exec

The exec component runs external system processes from a route. It is producer-only. Each route binds to a named `Profile`. A Profile is a pre-configured capability bundle: executable, argument policy, environment, working directory, and caps. It is pinned at startup. The component runs with `execvp` semantics, not a shell. ADR-0037 defines the fail-closed capability model.

The `exec-example` shows two profiles wired against a timer source:

```rust,no_run
use camel_api::CamelError;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::ComponentBundle;
use camel_component_exec::ExecBundle;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

fn register_exec_bundle(ctx: &mut CamelContext) {
    let toml_str = r#"
workspace_root = "."

[[profiles]]
name = "echo"
executable = "echo"
args = { allow = "any" }
timeout_secs = 10
working_dir = "."
accepted_exit_codes = [0]

[[profiles]]
name = "date"
executable = "date"
timeout_secs = 5
working_dir = "."
accepted_exit_codes = [0]
"#;
    let value: toml::Value = toml::from_str(toml_str).expect("parse toml");
    let bundle = ExecBundle::from_toml(value).expect("bundle");
    bundle.register_all(ctx);
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    let mut ctx = CamelContext::builder().build().await?;
    register_exec_bundle(&mut ctx);
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let route = RouteBuilder::from("timer:tick?period=500&repeatCount=1")
        .route_id("exec-echo")
        .set_header(
            camel_component_exec::headers::CAMEL_EXEC_ARGS,
            serde_json::json!(["-n", "Hello", "World"]),
        )
        .to("exec:echo")
        .to("log:info?showBody=true&showHeaders=true")
        .build()?;
    ctx.add_route_definition(route).await?;

    ctx.start().await?;
    Ok(())
}
```

<details>
<summary>YAML equivalent</summary>

```yaml
# Camel.toml
[components.exec]
workspace_root = "."

[[components.exec.profiles]]
name = "echo"
executable = "echo"
args = { allow = "any" }
timeout_secs = 10
working_dir = "."
accepted_exit_codes = [0]

# Route
routes:
  - id: exec-echo
    from: "timer:tick?period=500&repeatCount=1"
    steps:
      - setHeader:
          name: CamelExecArgs
          value: ["-n", "Hello", "World"]
      - to: "exec:echo"
      - to: "log:info?showBody=true&showHeaders=true"
```

The full example, with a second `date` profile, lives in [`examples/exec-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/exec-example).

</details>

## URI

```
exec:{profile-name}
```

The path segment names the `Profile` to run. The component verifies the profile exists at endpoint creation time. A missing profile fails route startup, not the first invocation.

| Aspect | Behavior |
| --- | --- |
| Direction | producer only |
| Profile selection | endpoint URI, not headers or body |
| Shell | rejected unless `allow_shell = true` |
| Default args policy | deny all non-empty args |
| Default exit codes | `[0]` |

## Configuration

Config lives under `[components.exec]` in `Camel.toml`. The `ExecBundle` deserializes it and calls `ExecGlobalConfig::validate()` at startup. Validation pins the canonical executable path, validates every `working_dir` against the canonical workspace root, and rejects duplicate profile names.

| Field | Default | Description |
| --- | --- | --- |
| `workspace_root` | `"."` | Base for `working_dir` confinement |
| `default_timeout_secs` | `30` | Per-profile timeout when the profile omits one |
| `default_concurrency` | `1` | Producer semaphore capacity per profile |
| `deny_env` | (see below) | Glob patterns stripped from every child env, last and always winning |

Default `deny_env` patterns: `LD_*`, `DYLD_*`, `PYTHONPATH`, `RUSTFLAGS`, `GIT_*`, `SSH_AUTH_SOCK`, `*_TOKEN`, `*_KEY`. They block library-preload and secret-injection vectors. `PATH` is opt-in.

Each `[[components.exec.profiles]]` entry has:

| Field | Default | Description |
| --- | --- | --- |
| `name` | — | Referenced as `exec:{name}` |
| `executable` | — | Binary name (PATH-resolved at startup) or absolute path |
| `args` | `exact` with empty values | `ArgPolicy` mode: `any`, `exact { values }`, or `prefix { values }` |
| `deny_flags` | `[]` | Prefix-matched denylist applied before `allow`. Always wins |
| `allow_shell` | `false` | Permit shell binaries as `executable` |
| `env.allow` | `[]` | Host env var names the child may inherit |
| `env.set` | `{}` | Explicit `KEY=VALUE` pairs |
| `working_dir` | `"."` | Must be relative to `workspace_root` and must exist |
| `timeout_secs` | global default | Process timeout. Force-kills the process group on Unix |
| `accepted_exit_codes` | `[0]` | Exit codes treated as success |
| `concurrency` | global default | Per-profile semaphore capacity |

A profile with zero profiles fails startup with `no profiles configured (fail-closed: refusing to execute anything)`. There is no default profile, no allow-all mode, and no shell convenience syntax.

## Security model

ADR-0037 fixes eleven decisions. The full text is the authority. The ones that shape every route:

**Profile-pinned, fail-closed.** A capability lives in a profile declared at startup. Exchange data cannot select an executable, change a path, or modify a policy. The component refuses to start with zero profiles. There is no `exec:shell?cmd=...` shortcut.

**No shell by default.** Commands run as `binary + literal argv`. No string concatenation, no `sh -c` wrapper. The component rejects known shells (`sh`, `bash`, `zsh`, `cmd.exe`, `pwsh`, …) at runtime unless the profile sets `allow_shell = true`. Even with `allow_shell`, the binary is the shell itself, called with explicit argv, never a concatenated command string.

**Canonical pin at startup.** The producer resolves the executable once during `validate()`. At runtime it uses the pinned path, never a fresh `PATH` lookup. The pin is not symlink-resolved, because multi-call binaries (BusyBox, uutils) dispatch on `argv[0]`. Canonicalization would break them.

**Per-element argument policy.** Every element in `CamelExecArgs` runs through `ArgPolicy`. `deny_flags` is applied first with prefix match. An arg that matches both `deny_flags` and the allow mode is denied. The default policy is `exact { values: [] }`, which denies every non-empty arg. The route must opt in to `any` or specify `values`.

**Empty environment by default.** The child starts with no host env. Three layers compose: `env.allow` (copy from host), `env.set` (explicit pairs), then global `deny_env` (strip globs, last and always winning). Operators must allow `PATH` explicitly for PATH-dependent binaries.

**Working-directory confinement.** `working_dir` is validated at startup against the canonical workspace root. Absolute paths fail. Paths containing `..` fail. Resolved paths that escape the root fail. The component does not create missing directories. The operator must pre-create them.

**No dynamic override from exchange data.** The profile is fixed by the endpoint URI. Conditional dispatch between profiles lives in route EIPs (`choice`, `recipient_list`), where the route author controls the branching, not the exchange payload. This is the lesson from ADR-0034 (ControlBus).

## Argument policy modes

| Mode | What passes |
| --- | --- |
| `any` | Every element. Explicit opt-in. Operator-curated args only |
| `exact { values = ["a", "b"] }` | Element must string-equal one of `values` |
| `prefix { values = ["--"] }` | Element must byte-start-with one of `values` |
| omitted | Deny all non-empty args (fail-closed default) |

Combine `deny_flags = ["--upload-pack"]` with `args = { allow = "any" }` to accept arbitrary args but block a known-dangerous flag. The denylist always wins.

## Headers

Input and output headers travel on the Exchange.

| Header | Direction | Type | Description |
| --- | --- | --- | --- |
| `CamelExecArgs` | input | JSON array of strings | Argument list passed to the binary |
| `CamelExecProfile` | output | string | Effective profile name |
| `CamelExecExitCode` | output | integer | Process exit code (omitted on timeout) |
| `CamelExecExitAccepted` | output | bool | `true` if `exit_code` is in `accepted_exit_codes` |
| `CamelExecTimedOut` | output | bool | `true` if the timeout fired |
| `CamelExecStderr` | output | string | Lossy-UTF8 stderr, for route predicates |
| `CamelExecStdoutTruncated` | output | bool | `true` if stdout exceeded `stdout_max_bytes` |
| `CamelExecStderrTruncated` | output | bool | `true` if stderr exceeded `stderr_max_bytes` |

The body after a producer call is a JSON `ExecResult`:

```json
{
  "exit_code": 0,
  "stdout": "aGVsbG8K",
  "stderr": "",
  "stdout_truncated": false,
  "stderr_truncated": false,
  "timed_out": false,
  "profile": "echo",
  "duration_ms": 12
}
```

`stdout` and `stderr` are base64 strings. Raw bytes would make pathological JSON. The `CamelExecStderr` header is lossy-UTF8 for use inside `choice()` and `log:` predicates. The dual representation is intentional.

## Non-error outcomes

A timeout or an exit code outside `accepted_exit_codes` does not return `Err`. The producer returns `Ok(exchange)` with the `ExecResult` body and headers set. This is forced by the `Service<Exchange>` contract: the Tower trait discards the mutated exchange on `Err`, and these outcomes carry output the route should see.

Branch on outcome with `CamelExecExitAccepted`:

```yaml
- to: "exec:build"
- choice:
    when:
      - predicate: "${header.CamelExecExitAccepted} == true"
        steps:
          - to: "log:info?showBody=true"
      - predicate: "${header.CamelExecTimedOut} == true"
        steps:
          - to: "log:warn?showBody=true"
    otherwise:
      - to: "log:error?showBody=true"
```

Only pre- and during-spawn failures return `Err`: arg policy denial, shell rejection, workdir escape, stdin over the cap, and OS spawn errors. Those route to the route's `ErrorHandler`.

## Timeouts and process-group kill

`timeout_secs` bounds the whole spawn-to-exit window. The `Child` handle is held outside the `tokio::select!` so the kill path can fire after the timeout. On Unix, the producer sends `SIGKILL` to the entire process group (`libc::kill(-pgid, SIGKILL)`). On Windows v1, the producer calls `child.start_kill()` on the immediate child. Process-group tree-kill via Job Objects is a post-v1 change. `kill_on_drop(true)` is set as defense in depth.

When the timeout fires, drain tasks for stdout and stderr keep running. After the kill, pipes close and the tasks finish with whatever bytes they captured. The `ExecResult` carries the partial output plus `timed_out: true` and `exit_code: null`.

## Errors

Pre- and during-spawn failures surface as `CamelError::ProcessorErrorWithSource(msg, Arc<ExecError>)`. `ExecError` is `#[non_exhaustive]` with variants `NotAllowlisted`, `ArgPolicyDenied`, `ShellRejected`, `InvalidWorkDir`, `StdinTooLarge`, `InvalidArgs`, and `Spawn(#[from] std::io::Error)`.

Log levels: arg-policy denial, shell rejection, and timeout fire at `warn!`. A non-zero exit outside the accepted list logs at `info!` because the route is expected to branch on `CamelExecExitAccepted`. A non-zero exit inside the accepted list logs at `debug!`. A spawn failure logs at `error!` because no route handler is running.

Every execution emits an `ExecAuditEvent`. The event carries the profile name, resolved executable path, args, env keys, cwd, exit code, timeout flag, truncation flags, and duration.

## Metrics

The producer emits monotonic counters and histograms through `MetricsCollector::record_counter` and `record_histogram`. The full set:

| Metric | Type | Labels | Fires when |
| --- | --- | --- | --- |
| `exec_policy_denials_total` | counter | `reason`, `route` | Arg policy or shell rejection denies a call |
| `exec_timeouts_total` | counter | `route` | Timeout kills the process |
| `exec_exit_code` | counter | `code`, `route` | Process exits with a code |
| `exec_stdout_truncated_total` | counter | (none) | Stdout exceeds the cap |
| `exec_duration_secs` | histogram | `route` | Every call |

The default trait methods on `MetricsCollector` are no-ops. `PrometheusMetrics` and `OtelMetrics` do not yet override them, so these counters are silently dropped in production until a backend implements the trait methods.

**Reference**: [camel-component-exec CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-exec/CONTEXT.md), [ADR-0037: Exec Component Fail-Closed Capability Model](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0037-exec-component-fail-closed-capability-model.md). Example source: [`examples/exec-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/exec-example).
