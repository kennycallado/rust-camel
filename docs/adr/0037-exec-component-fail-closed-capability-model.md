# ADR-0037: Exec Component Fail-Closed Capability Model

**Date:** 2026-07-07
**Status:** Accepted
**Amends:** none
**Cross-refs:** ADR-0032 (exchange-data trust boundary), ADR-0033 (fail-closed startup
validation), ADR-0034 (ControlBus capability authz — profile-pinning lesson)

## Context

rust-camel needs a system-command-execution component for the agentic and tool-execution
use case. Direct shell execution (e.g., `sh -c` or `cmd /C`) is unsafe — it permits
argument injection, PATH hijacking, and environment-based privilege escalation. Apache
Camel's `exec` component allows arbitrary commands, which is incompatible with
rust-camel's fail-closed security posture (ADR-0033) and exchange-data trust boundary
(ADR-0032).

ADR-0034 (ControlBus) established that exchange-data-driven capability selection is
dangerous — capabilities must be **profile-pinned** at configuration time, not dynamically
selected from runtime data. The exec component should not repeat that mistake.

## Decision

Eleven locked decisions govern the component's design:

### 1. Producer-only, execvp semantics (no shell)

The component is producer-only (no consumer/inbound). It executes commands via
`execvp`-style semantics: binary path + literal argument array. There is no
`exec:shell` variant. Shells (`sh`, `bash`, `zsh`, `powershell`, `cmd`) are rejected
at runtime unless `allow_shell=true` is explicitly set on the profile. Even with
`allow_shell=true`, the binary must be the shell itself (not a `/bin/sh -c` wrapper)
— the shell runs with explicit argv, not a concatenated string.

### 2. Allowlist fail-closed (ADR-0033)

The component refuses to start unless at least one profile is configured. With zero
profiles, `ExecGlobalConfig::validate()` returns an error at startup. This is the
Require-Explicit-Choice disposition from ADR-0033 — the operator must explicitly
declare every executable they intend to run.

### 3. Profile-pinned by endpoint URI only

The target profile is determined by the endpoint URI (`exec:{profile-name}`). There is
NO dynamic override from exchange headers or body (ADR-0032, ADR-0034). Conditional
dispatch to different profiles is achieved via `choice()` or `recipient_list()` in
the route definition — where the route author, not exchange data, controls the
branching logic.

### 4. Canonical executable pinning at startup

Each profile's executable is resolved once at startup validation: either from PATH
(lookup by name) or as an absolute path (used as-is). The resolved path is stored in
`canonical_executable: Option<PathBuf>`. At runtime, the producer uses this pinned
path — no re-resolving from PATH, which eliminates PATH hijacking.

**Important caveat:** The pinned path is NOT canonicalized via
`std::fs::canonicalize()`. Multi-call binaries (BusyBox, uutils on NixOS/coreutils)
dispatch based on `argv[0]` — `canonicalize()` would resolve the symlink to the
multi-call binary, breaking `argv[0]` dispatch. The `which()` implementation stores
the directory-path as-found. Inode/dev same-path-replacement detection is deferred to
post-v1.

### 5. Arg-policy per-element, first-class in v1

All args passed via the `CamelExecArgs` header (JSON array of strings) are validated
per-element against the profile's `ArgPolicy`. Four modes:

- `any` — every element accepted (explicit opt-in, operator-curated).
- `exact { values }` — every element must string-equal one of `values`.
- `prefix { values }` — every element must byte-start-with one of `values`.
- Default (omitted) = `exact { values: [] }` — deny all non-empty args (fail-closed).

`deny_flags` is applied first with a broad prefix match (e.g., `--` matches any flag
starting with `--`), then `allow` is evaluated. An arg that matches both `deny_flags`
and `allow` is **denied** — deny always wins.

Regex-based arg policy is deferred to post-v1.

### 6. Environment sanitized by default

The child process starts with an **empty environment** — the host environment is NOT
inherited. Three layers control env:

- `env.allow` — var names from the host env that the child may receive.
- `env.set` — explicit `KEY=VALUE` pairs.
- `global.deny_env` — glob patterns applied LAST, always win over allow/set.

Default `deny_env` patterns (`LD_*`, `DYLD_*`, `PYTHONPATH`, `RUSTFLAGS`, `GIT_*`,
`SSH_AUTH_SOCK`, `*_TOKEN`, `*_KEY`) block the most common secret-injection and
library-preload vectors. `PATH` is opt-in via `env.allow` — operators must explicitly
allow it for PATH-dependent executables.

### 7. cwd confinement

Every profile's `working_dir` is validated at startup relative to the pinned
`canonical_workspace_root`. The validation:

- Rejects absolute paths.
- Rejects paths containing `..`.
- Requires the resolved path to `starts_with` the workspace root.
- Does NOT create the directory if missing (fail-closed: operator must pre-create).

At runtime, the producer uses `canonical_workspace_root` (startup-pinned, never a
`"."` fallback — I-6 in the spec).

### 8. Non-error outcomes (timeout, exit-code mismatch)

Timeout and exit-code-not-in-accepted list return `Ok(exchange)` with the `ExecResult`
JSON body and headers — NOT `Err`. This is forced by the `Service<Exchange>` contract:
the Tower `Service` trait discards the mutated exchange on `Err`, and these outcomes
have useful output the route should inspect.

Only pre/during-spawn failures (arg policy denial, shell rejection, spawn failure,
stdin too large, workdir escape) return `Err`. Routes that want to branch on
exit-code outcomes use `choice()` checking `CamelExecExitAccepted`.

**Key invariant:** There are no "partial errors." A timeout produces a complete
`ExecResult` with partial output captured before the kill.

### 9. Timeout with process-group kill

Timeout uses `tokio::select!` (biased: child wait checked first, then timeout). The
`Child` handle is held **outside** the timeout region (spawned before `select!`) so
`kill_tree` can fire after timeout elapses.

- **Unix:** `libc::kill(-pgid, SIGKILL)` kills the entire process group.
- **Windows v1:** `child.start_kill()` kills the immediate child only. Process-group
  tree-kill via Windows Job Objects is deferred to post-v1.

After kill, `child.wait().await` reaps the zombie. Drain tasks (stdin write, stdout
read, stderr read) continue running concurrently; when pipes close after kill, the
drain tasks finish and partial output is collected.

`kill_on_drop(true)` is set on the `Command` as defense-in-depth.

### 10. Error surface: `CamelError::ProcessorErrorWithSource`

All exec errors surface as:

```
CamelError::ProcessorErrorWithSource(msg, Arc<ExecError>)
```

`ExecError` is `#[non_exhaustive]` with the following variants:
`NotAllowlisted`, `ArgPolicyDenied`, `ShellRejected`, `InvalidWorkDir`,
`StdinTooLarge`, `InvalidArgs`, `Spawn(#[from] std::io::Error)`.

### 11. Configuration structure

Config lives under `[components.exec]` in TOML:

```toml
[components.exec]
workspace_root = "."
default_timeout_secs = 30
default_concurrency = 1
deny_env = ["LD_*", "DYLD_*", "PYTHONPATH", ...]

[[components.exec.profiles]]
name = "echo"
executable = "echo"
args = { allow = "any" }
working_dir = "."
timeout_secs = 10
accepted_exit_codes = [0]
```

## Spec Amendments

The following deviations from the original blessed spec's illustrative blocks are
recorded here as authoritative corrections:

- **`ExecResult.stdout` / `stderr` are base64 `String`**, not `Bytes`. `Bytes` lacks a
  `Serialize` impl without a serde feature flag, and raw byte arrays would be
  pathological JSON. Base64 is the standard encoding for binary data in JSON.
- **`ExecResult.duration_ms: u64`**, not `Duration`. `Duration` has no stable JSON
  `Serialize` implementation.
- **`ExecError::Io` is collapsed into `Spawn(#[from] std::io::Error)`**. They are the
  same type — `std::io::Error` covers all spawn failures. `#[non_exhaustive]` on the
  enum keeps the variant set consumer-safe for future additions.
- **Windows v1 kill is child-only.** Process-group tree-kill via `start_kill()` on the
  immediate child is the v1 implementation. Windows Job Object tree-kill is deferred
  to post-v1.
- **Timeout captures partial output.** Drain tasks (stdout/stderr) continue running
  inside `tokio::spawn` during the `select!`. When `kill_tree` fires and the pipes
  close, the drain tasks complete with whatever bytes they accumulated. The partial
  output is included in the `ExecResult`.

## Cross-crate Change

The `MetricsCollector` trait in `camel-api` gained a new default method:

```rust
fn record_counter(&self, name: &str, value: f64, labels: &[(&str, &str)]) { }
```

This is a backward-compatible addition — all existing implementors receive the
default no-op behavior. The exec component needs monotonic counters
(`exec_policy_denials_total`, `exec_timeouts_total`, `exec_exit_code`,
`exec_stdout_truncated_total`) which `record_histogram(1.0)` could not represent.

## Consequences

### Positive

- **Fail-closed by default:** No profiles → nothing executes. No shell injection
  surface. No PATH hijacking at runtime (pinned at startup).
- **Injection-resistant:** execvp-style argument passing, no string concatenation.
  Arg-policy per-element with deny-first ordering.
- **Profile-pinned capabilities:** Exchange data cannot select executables or modify
  profile configuration (ADR-0032, ADR-0034).
- **Non-error outcomes enable route-level branching:** Routes can `choice()` on
  `CamelExecExitAccepted` to handle success/failure without losing output.
- **Auditable:** Every execution emits an `ExecAuditEvent` with profile, args, env
  keys, exit code, and duration.

### Negative

- **No shell convenience:** Operators cannot write `exec:shell?cmd=ls -la | grep foo`.
  Every command must be a named profile. This is intentional but increases config
  overhead for simple ad-hoc commands.
- **Operator must pre-create working directories:** The component does NOT create
  `working_dir` — fail-closed over silent creation. Missing directories fail at
  startup validation.
- **Windows tree-kill is best-effort in v1:** Only `child.start_kill()` is used; job
  objects require a post-v1 change. `kill_on_drop(true)` is the only defense-in-depth
  for process cleanup on Windows.
- **Multi-call binary detection deferred:** Same-path-replacement attacks are a
  theoretical concern in v1. Inode/dev resolution would break BusyBox/NixOS dispatch
  and requires post-v1 design.
- **Metrics backends do not yet wire `record_counter`/`record_histogram`:** The exec
  component emits `exec_policy_denials_total`, `exec_timeouts_total`, `exec_exit_code`,
  `exec_stdout_truncated_total`, and `exec_duration_secs` via the trait API, but
  `PrometheusMetrics` and `OtelMetrics` inherit the default no-op for both generic
  methods. These metrics are silently dropped in production until the backends override
  them (tracked as a follow-up). Only `record_exchange_duration`/`record_circuit_breaker_change`
  are wired today.
