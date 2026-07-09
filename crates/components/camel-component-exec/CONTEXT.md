# camel-component-exec

## Language

**Profile**:
A named, pre-configured capability bundle (executable + arg-policy + env + cwd + caps)
pinned at startup. The unit of allowlisting. Referenced by endpoint URI as `exec:{name}`.
_Avoid_: task, command, action (too generic; a Profile is a config-time concept)

**ExecBundle**:
The `ComponentBundle` impl that owns `[components.exec]` in TOML. Calls
`ExecGlobalConfig::validate()` for fail-fast startup validation (canonical pinning,
cwd confinement, dup-check). Registers `ExecComponent` via `register_all`.
_Avoid_: plugin, package

**ExecComponent**:
The `Component` impl (scheme `"exec"`). Owns `Arc<ExecGlobalConfig>`. Creates
`ExecEndpoint`s from URIs, verifying the requested profile exists at endpoint
creation time (not deferred to produce).
_Avoid_: executor, runner

**ExecEndpoint**:
An endpoint bound to a specific profile name. Holds `Arc<ExecGlobalConfig>` and
creates `ExecProducer` instances.
_Avoid_: command endpoint, job endpoint

**ExecProducer**:
The `Service<Exchange>` impl that runs the enforcement flow: resolve args,
arg-policy eval, shell reject, env build, cwd confinement, stdin cap, semaphore
acquire, spawn+timeout+drain, result population, metrics+audit. Materialized-only
in v1.
_Avoid_: command runner, process executor

**ArgPolicy**:
Per-element argument validation (any/exact/prefix). Applied to every element of
the `CamelExecArgs` header array. Default (omitted) = deny all non-empty args.
Regex deferred to post-v1.
_Avoid_: arg filter, arg validator

**deny_flags**:
Optional flag denylist applied FIRST with broad prefix match (e.g., `--` matches
any flag starting with `--`). Always wins over `allow` — an arg matching both is
denied.
_Avoid_: flag blocklist, denied args

**EnvPolicy**:
Three-layer env construction: `allow` (host env var names to inherit), `set`
(explicit KEY=VALUE), then `global.deny_env` glob patterns applied LAST (always
win). Host env is NOT inherited by default — child starts empty.
_Avoid_: env config, environment

**deny_env**:
Global env-var deny globs applied LAST over every profile. Default patterns
(`LD_*`, `DYLD_*`, `PYTHONPATH`, `RUSTFLAGS`, `GIT_*`, `SSH_AUTH_SOCK`, `*_TOKEN`,
`*_KEY`) block library-preload and secret-leakage vectors. `PATH` is opt-in.
_Avoid_: env blacklist, secret filter

**Canonical executable / pin**:
The executable resolved once at startup validation; reused at runtime (no PATH
re-scan). NOT canonicalized via `std::fs::canonicalize()` — multi-call binary
compat (BusyBox/uutils) requires preserving the symlink path as `argv[0]`.
_Avoid_: resolved path, locked binary

**Non-error outcome**:
Timeout and exit-code-not-in-accepted-list return `Ok(exchange)` with result JSON
+ headers (not `Err`). Only pre/during-spawn failures are `Err`. Routes branch on
`CamelExecExitAccepted` via `choice()`.
_Avoid_: partial error, soft error

**ExecResult**:
Materialized result placed in the exchange body as JSON. Contains `exit_code`,
base64 `stdout`/`stderr`, `timed_out`, `duration_ms`, `profile`, and truncation
flags. `#[non_exhaustive]`.
_Avoid_: command result, process output

**Fail-closed**:
Refuses to execute unless profiles are explicitly configured. Zero-profiles config
fails `validate()` at startup (ADR-0033). No "default profile" or "allow all" mode.
_Avoid_: secure by default, safe mode

**Cwd confinement**:
`working_dir` validated at startup relative to `canonical_workspace_root`. Rejects
absolute paths, `..`, paths that don't `starts_with` root. Directory must exist
(no silent creation).
_Avoid_: workdir jail, sandbox (reserved for v2 Sandbox enum)

## Example dialogue

> "How do I run a command with the exec component?"
> "First create a profile in your `[components.exec]` config — for example an `echo` profile
> with `executable = "echo"`, `args = { allow = "any" }`. Then use `exec:echo` in your route.
> Pass arguments via the `CamelExecArgs` header as a JSON array. The component handles
> spawning, timeout, and output capture — output comes back as JSON in the exchange body."
>
> "What happens if the command times out?"
> "The producer returns `Ok(exchange)` — not an error. The exchange body contains the
> partial output, headers include `CamelExecTimedOut: true` and `CamelExecExitAccepted: false`.
> Your route can inspect these in a `choice()` to branch on timeout vs success."
>
> "Can I run a shell command like `ls -la | grep foo`?"
> "No — the component uses execvp semantics (binary + literal argv). There is no shell
> interpretation. Pipe each command through separate profiles and compose with route EIPs,
> or use `allow_shell=true` to run a shell binary directly with explicit arguments."

## Log-level policy

| Site | Level | Rationale |
|------|-------|-----------|
| Producer denied by arg policy | `warn!` | Operator signal; route handler owns `error!` |
| Profile shell rejected | `warn!` | Operator misconfiguration (@audit) |
| Timeout fired | `warn!` | Operational signal, captured in headers |
| Non-zero exit (non-accepted) | `info!` | Normative; route branches on `CamelExecExitAccepted` |
| Non-zero exit (accepted) | `debug!` | Normative per profile config |
| Spawn failure | `error!` | System resource issue; no route handler running |
| Config validation error | `error!` | Startup, fatal |

## Breaking changes (0.x)

This is a new component introduced in 0.x — no breaking changes yet.
