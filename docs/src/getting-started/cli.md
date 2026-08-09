# CLI usage

The `camel` CLI runs, scaffolds, and inspects integration routes from the terminal. Install it with `cargo install camel-cli`.

## Quick reference

| Command | Purpose | Example |
|---------|---------|---------|
| `run` | Start routes from a config file | `camel run` |
| `new` | Scaffold a new project | `camel new my-integration` |
| `journal inspect` | Read events from a journal file | `camel journal inspect runtime.db` |
| `plugin new` | Scaffold a WASM plugin | `camel plugin new my-plugin` |
| `plugin build` | Compile and install a WASM plugin | `camel plugin build` |
| `openapi generate` | Emit an OpenAPI document from REST routes | `camel openapi generate routes.yaml` |

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

With `--watch`, the CLI monitors route files for changes. The watcher groups rapid edits behind a 300 ms debounce window. Set `watch_debounce_ms` in `Camel.toml` to change it. Edits take effect without a restart.

```console
camel run --watch
```

See `crates/camel-config/CONTEXT.md` for the debounce default.

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

## camel plugin

Scaffold and build WASM plugins. Plugins extend the runtime with custom processors, beans, or authorization policies. The CLI ships two subcommands: `new` and `build`.

### camel plugin new

Create a plugin project from a template.

```console
camel plugin new my-plugin
```

| Flag | Description |
|------|-------------|
| `<name>` (positional) | Plugin name (letters, digits, hyphens, underscores) |
| `--type <TYPE>` | `processor` (default), `bean`, or `authorization-policy` |
| `--force` | Overwrite files if the directory already exists |

Flag definitions live in `crates/camel-cli/src/commands/plugin.rs`.

### Expected output

```text
Created camel processor plugin 'my-plugin'

Next steps:
  cd my-plugin
  camel plugin build
```

### camel plugin build

Compile a plugin to the `wasm32-wasip2` target and install the artifact into the project plugins directory.

```console
camel plugin build
```

Run this from inside the plugin directory, or pass a path:

```console
camel plugin build ./my-plugin
```

| Flag | Description |
|------|-------------|
| `<path>` (positional, optional) | Plugin directory (default: current directory) |
| `--debug` | Build without `--release` |

The CLI copies the compiled `.wasm` into the plugins directory. It reads the directory from `[default.components.wasm].plugins_dir` in `Camel.toml`. The default is `plugins`. See `crates/camel-cli/src/commands/plugin.rs` for the resolution rules.

### Expected output

```text
Built and installed plugin 'my-plugin'
  source: /path/to/my-plugin/target/wasm32-wasip2/release/my_plugin.wasm
  installed: /path/to/camel-root/plugins/my-plugin.wasm
```

## camel openapi

Emit an OpenAPI 3.0.3 document from the `rest:` blocks in a YAML or JSON route file. The CLI ships one subcommand: `generate`.

```console
camel openapi generate routes.yaml
```

| Flag | Description |
|------|-------------|
| `<file>` (positional) | Path to the route file (`.yaml`, `.yml`, or `.json`) |
| `--title <TITLE>` | API title for the `info` section (default: `Generated API`) |
| `--version <VER>` | API version for the `info` section (default: `1.0.0`) |

Flag definitions live in `crates/camel-cli/src/commands/openapi.rs`.

### Expected output

The command prints a pretty JSON document to stdout. The top-level `openapi` field is `3.0.3`. Each `rest:` block becomes a path entry. Each operation becomes a verb under that path.

```json
{
  "openapi": "3.0.3",
  "info": {
    "title": "Generated API",
    "version": "1.0.0"
  },
  "paths": {
    "/api/users": {
      "get": { "operationId": "listUsers" }
    }
  }
}
```

If a file has no `rest:` blocks, the command exits with an error. Validation warnings print to stderr.

## Next steps

- See [First route in YAML](yaml-route.md) for a complete walkthrough.
- See [YAML DSL](../yaml-dsl/index.md) for the full YAML reference.
- See [Operations](../operations/index.md) for health checks and monitoring.

**Reference**: [CLI crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-cli/CONTEXT.md)
