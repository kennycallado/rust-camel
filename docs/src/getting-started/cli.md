# CLI usage

The `camel` CLI runs, scaffolds, and inspects integration routes from the terminal. Install it with `cargo install camel-cli`.

## Quick reference

| Command | Purpose | Example |
|---------|---------|---------|
| `run` | Start routes from a config file | `camel run` |
| `new` | Scaffold a new project | `camel new my-integration` |
| `journal inspect` | Read events from a journal file | `camel journal inspect runtime.db` |
| `lint` | Lint a route file | `camel lint routes/api.yaml` |
| `test` | Run declarative mock tests | `camel test tests/` |
| `job` | Run or list jobs | `camel job report-nightly` |
| `compile` | Pack a route or job into an artifact | `camel compile routes.yaml -o app` |

## camel run

Start a Camel context from YAML route files.

> **Trust model.** `camel run` executes route scripts, WASM modules, and beans that the current working directory supplies. Run it only from a trusted directory.

The CLI reads `Camel.toml` from the current directory. The file defines route file patterns, log levels, component settings, and supervision policies.

```console
camel run
```

### Config file

```toml
{{#include ../../../examples/camel-cli-run/Camel.toml:config}}
```

The `[default]` profile sets `routes = ["routes/*.yaml"]` to discover route files. The `[development]` and `[production]` profiles override log level and watch mode.

### Route file

```yaml
{{#include ../../../examples/camel-cli-run/routes/hello.yaml:hello-route}}
```

### Flags

| Flag | Description |
|------|-------------|
| `--routes <GLOB>` | Override the route file pattern from `Camel.toml` |
| `--config <FILE>` | Path to `Camel.toml` (default: `Camel.toml`) |
| `--watch` | Enable file-watcher hot-reload |
| `--no-watch` | Disable file-watcher hot-reload |
| `--otel` | Enable OpenTelemetry export |
| `--otel-endpoint <URL>` | OTLP endpoint URL (implies `--otel`) |
| `--service-name <NAME>` | OTel service name (implies `--otel`) |
| `--health-port <PORT>` | Start a standalone health server on this port |

Flag definitions live in `crates/camel-cli/src/main.rs`.

### Expected output

The CLI starts the context, discovers routes that match the glob, and runs them. For `hello.yaml` above, the route logs one message every two seconds. The message text repeats with an increasing counter:

```text
Hello from camel-cli! Exchange #1
Hello from camel-cli! Exchange #2
```

Press Ctrl+C (or send `SIGTERM`) to stop.

### Profiles

Set the active profile with the `CAMEL_PROFILE` environment variable:

```console
CAMEL_PROFILE=development camel run
```

The `development` profile sets `log_level = "DEBUG"` and `watch = true`. The `production` profile sets `log_level = "WARN"` and `watch = false`.

### Hot-reload

Hot-reload is off by default. With `--watch`, the CLI monitors route files for changes. The watcher groups rapid edits behind a 300 ms debounce window. Set `watch_debounce_ms` in `Camel.toml` to change it. Edits take effect without a restart.

```console
camel run --watch
```

### Minimal config

A route can start without exec components or complex setup:

```toml
{{#include ../../../examples/camel-cli-no-exec/Camel.toml:config}}
```

```yaml
{{#include ../../../examples/camel-cli-no-exec/routes/hello.yaml:hello-route}}
```

## camel new

Scaffold a new Camel project with a `Camel.toml` and a `routes/` directory.

```console
camel new my-integration
cd my-integration
camel run
```

| Flag | Description |
|------|-------------|
| `<name>` (positional) | Project name (letters, digits, hyphens, underscores) |
| `--template <NAME>` | Template to use (default: `basic`) |
| `--profile-layout <LAYOUT>` | `simple` or `env` (default: `env`) |
| `--force` | Overwrite files if the directory already exists |

Layout `simple` writes only a `[default]` profile. Layout `env` adds `[development]` and `[production]`. Flag definitions live in `crates/camel-cli/src/commands/new.rs`.

### Expected output

```text
Created camel project: my-integration

Next steps:
  cd my-integration
  camel run
  camel run --watch
```

## camel journal inspect

Read events from a redb runtime journal file. Use this command for offline debugging of a previous session.

```console
camel journal inspect runtime.db
```

| Flag | Description |
|------|-------------|
| `<path>` (positional) | Path to the `.db` journal file |
| `--limit <N>` | Show only the last N events (default: 100) |
| `--route <ID>` | Filter to a specific route id |
| `--format <FMT>` | `table` (default) or `json` |

Flag definitions live in `crates/camel-cli/src/commands/journal.rs`.

### Expected output

The default table format prints one row per event:

```text
SEQ        TIMESTAMP                   EVENT                    ROUTE_ID
--------------------------------------------------------------------------------
00000001   2026-08-08T12:00:00.000Z    RouteRegistered          hello
00000002   2026-08-08T12:00:00.100Z    RouteStartRequested      hello
00000003   2026-08-08T12:00:00.250Z    RouteStarted             hello
```

Pass `--format json` to pipe events into another tool.

## More commands

This page covers the commands you use to start. The rest of the CLI surface:

- [`camel job`](../cli/job.md) runs and lists jobs from `*.job.yaml` documents, with typed declared arguments.
- [`camel compile`](../cli/compile.md) packs a route or job document into a self-contained executable artifact.
- [`camel plugin new` and `camel plugin build`](../cli/openapi-plugin.md) scaffold and build WASM plugins.
- [`camel openapi generate`](../cli/openapi-plugin.md) emits an OpenAPI 3.0.3 document from `rest:` blocks.
- [`camel test`](../testing/index.md) runs declarative mock tests.
- The full command table lives in the [CLI reference](../cli/index.md).

## Next steps

- See [First route in YAML](yaml-route.md) for a complete walkthrough.
- See [YAML DSL](../yaml-dsl/index.md) for the full YAML reference.
- See [Operations](../operations/index.md) for health checks and monitoring.

**Reference**: [CLI crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-cli/CONTEXT.md)
