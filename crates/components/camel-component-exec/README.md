# camel-component-exec

Fail-closed system command execution component for [rust-camel](https://crates.io/crates/camel-api).

## Features

- **Profile-pinned, fail-closed** — Zero profiles → nothing executes. Each profile is a named capability bundle (executable + arg-policy + env + cwd + caps) pinned at startup.
- **No shell (execvp)** — Commands run via `execvp`-style semantics: binary path + literal argument array. No shell interpretation, no string concatenation injection surface.
- **Per-element arg-policy** — Every argument is validated against `ArgPolicy`: `any`, `exact { values }`, or `prefix { values }`. `deny_flags` (broad prefix match) always wins over allow.
- **Environment sanitization** — Child starts with empty environment. Three-layer control: `env.allow` (inherit from host), `env.set` (explicit KEY=VALUE), then `global.deny_env` glob patterns applied last/always-win.
- **cwd confinement** — `working_dir` validated at startup relative to `canonical_workspace_root`. No `..` escape, no absolute paths, no silent directory creation.
- **Timeout with process-group tree-kill** — Unix: `SIGKILL` to entire process group. Windows v1: `child.start_kill()` (process-group tree-kill deferred).
- **Non-error outcomes** — Timeout and exit-code-not-in-accepted-list return `Ok(exchange)` with result JSON + headers, NOT `Err`. Routes branch on `CamelExecExitAccepted` via `choice()`.
- **Audit** — Every execution emits an `ExecAuditEvent` with profile, args, env keys, exit code, and duration (redacted).

## Quick Start

Add to `Camel.toml`:

```toml
[components.exec]
workspace_root = "."
default_timeout_secs = 30

[[components.exec.profiles]]
name = "echo"
executable = "echo"
args = { allow = "any" }
working_dir = "."
timeout_secs = 10
accepted_exit_codes = [0]
```

Use in a route:

```yaml
routes:
  - id: "exec-echo"
    from: "timer:tick?period=5000"
    steps:
      - setHeader:
          name: "CamelExecArgs"
          value: ['{"args":["hello","world"]}']
      - to: "exec:echo"
      - to: "log:info"
```

Or programmatically:

```rust
use camel_component_exec::ExecBundle;
ctx.add_bundle(ExecBundle::from_toml(config))?;
```

## URI Scheme

```
exec:{profile-name}
```

The target profile is determined by the endpoint URI path. There is NO dynamic override from exchange headers or body. Conditional dispatch to different profiles is achieved via `choice()` or `recipient_list()` in the route definition.

## Security Model

See [ADR-0037](https://github.com/kenny/rust-camel/blob/main/docs/adr/0037-exec-component-fail-closed-capability-model.md) for the full capability model.

Key tenets:
- **Fail-closed by default** — No profiles → nothing executes. No shell injection surface. No PATH hijacking at runtime (canonical executable pinned at startup).
- **Profile-pinned capabilities** — Exchange data cannot select executables or modify profile configuration.
- **Injection-resistant** — execvp-style argument passing, no string concatenation. Arg-policy per-element with deny-first ordering.
- **Auditable** — Every execution recorded with redacted env/args in audit events.

## Configuration Reference

### `[components.exec]` (global)

| Field                  | Type             | Default | Description |
|------------------------|------------------|---------|-------------|
| `workspace_root`       | String           | `"."`   | Base directory for cwd confinement |
| `default_timeout_secs` | u64              | `30`    | Default timeout per profile (seconds) |
| `default_concurrency`  | usize            | `1`     | Default producer semaphore capacity |
| `deny_env`             | Vec\<String\>    | (see below) | Global env-var deny glob patterns applied LAST |

Default `deny_env` patterns: `LD_*`, `DYLD_*`, `PYTHONPATH`, `RUSTFLAGS`, `GIT_*`, `SSH_AUTH_SOCK`, `*_TOKEN`, `*_KEY`.

### `[[components.exec.profiles]]`

| Field                | Type             | Default | Description |
|----------------------|------------------|---------|-------------|
| `name`               | String           | —       | Profile name (referenced as `exec:{name}`) |
| `executable`         | String           | —       | Binary name (PATH-resolved) or absolute path |
| `args`               | ArgPolicy        | `exact` with empty values | Argument policy |
| `deny_flags`         | Vec\<String\>    | `[]`    | Flag denylist (broad prefix match, always wins) |
| `allow_shell`        | bool             | `false` | Allow shell binary as executable |
| `env.allow`          | Vec\<String\>    | `[]`    | Host env var names to inherit |
| `env.set`            | HashMap          | `{}`    | Explicit KEY=VALUE pairs |
| `working_dir`        | String           | `"."`   | Working directory (relative to workspace_root) |
| `timeout_secs`       | Option\<u64\>    | global default | Per-profile timeout override |
| `accepted_exit_codes`| Vec\<i32\>       | `[0]`   | Exit codes treated as success |
| `concurrency`        | Option\<usize\>  | global default | Per-profile concurrency cap |

### ArgPolicy Modes

| Mode | Description |
|------|-------------|
| `any` | Every element accepted |
| `exact { values: ["a", "b"] }` | Element must string-equal one of `values` |
| `prefix { values: ["--"] }` | Element must byte-start-with one of `values` |
| Omitted | Deny all non-empty args (fail-closed default) |

## Exchange Headers

| Header | Direction | Description |
|--------|-----------|-------------|
| `CamelExecArgs` | Input | JSON array of argument strings |
| `CamelExecExitCode` | Output | Exit code of the process |
| `CamelExecExitAccepted` | Output | `true` if exit code is in profile's accepted list |
| `CamelExecTimedOut` | Output | `true` if the process was killed by timeout |
| `CamelExecStderr` | Output | Stderr content (base64 string) |
| `CamelExecStdoutTruncated` | Output | `true` if stdout exceeded cap |
| `CamelExecStderrTruncated` | Output | `true` if stderr exceeded cap |

The exchange body contains an `ExecResult` JSON object with `exit_code`, `stdout`, `stderr`, `timed_out`, `duration_ms`, `profile`, and truncation flags.

## Architecture Decisions

- [ADR-0037](https://github.com/kenny/rust-camel/blob/main/docs/adr/0037-exec-component-fail-closed-capability-model.md) — Exec Component Fail-Closed Capability Model

## Example

See [`examples/exec-example/`](https://github.com/kenny/rust-camel/tree/main/examples/exec-example) for a working example.

## License

Licensed under either of Apache License, Version 2.0 or MIT License at your option.
