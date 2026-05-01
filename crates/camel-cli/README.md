# camel-cli

Command-line interface for Apache Camel in Rust.

## Installation

```bash
cargo install camel-cli
```

## Usage

```
camel <COMMAND>

Commands:
  new      Scaffold a new Camel project
  run      Start a Camel context from YAML route files with hot-reload
  journal  Inspect a runtime journal file
  help     Print help

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## `camel new`

Scaffolds a new project directory with a `Camel.toml`, `routes/hello.yaml`, `README.md`, and `.gitignore`.

```bash
camel new <NAME>

Arguments:
  <NAME>  Project name and directory to create

## Overview

Command-line interface for Apache Camel in Rust.

Options:
  --template <TEMPLATE>       Template to use (default: basic)
  --profile-layout <LAYOUT>   Profile layout: simple or env (default: env)
  --force                     Overwrite files if the directory already exists
```

### Examples

```bash
# Create a project with default (env) profile layout
camel new my-integration

# Single-profile layout (no development/production sections)
camel new my-integration --profile-layout simple

# Overwrite an existing directory
camel new my-integration --force
```

### Generated layout

```
my-integration/
├── Camel.toml          # Route patterns, log level, watch, profiles
├── README.md
├── .gitignore
└── routes/
    └── hello.yaml      # Timer route that logs every 2s
```

Then run it:

```bash
cd my-integration
camel run
```

## `camel run`

Starts a Camel context and loads routes from YAML files. Hot-reload is **disabled
by default** — enable it with `--watch` or via `Camel.toml`.

```bash
camel run [OPTIONS]

Options:
  --routes <GLOB>   Glob pattern for route YAML files
  --config <FILE>   Path to Camel.toml (default: Camel.toml)
  --watch           Enable file-watcher hot-reload
  --no-watch        Disable file-watcher hot-reload (overrides Camel.toml)
  --health-port <PORT>  Override health server port (enables standalone health server)
```

### Quick start — no config file

```bash
# Runs routes/*.yaml without hot-reload
camel run --routes "routes/*.yaml"

# Same but with hot-reload enabled
camel run --routes "routes/*.yaml" --watch
```

### With a `Camel.toml`

Create a `Camel.toml` next to your route files:

```toml
[default]
routes = ["routes/**/*.yaml"]
log_level = "INFO"
watch = false

[default.supervision]
max_attempts = 5
initial_delay_ms = 1000
backoff_multiplier = 2.0
max_delay_ms = 60000

[development]
log_level = "DEBUG"
watch = true

[production]
log_level = "ERROR"
watch = false
```

Then run:

```bash
camel run
# or with an explicit profile (development enables watch = true):
CAMEL_PROFILE=development camel run
# or force watch regardless of profile:
camel run --watch
```

The `--routes` flag overrides whatever `routes` is set to in the config file.
`--watch` / `--no-watch` override the `watch` field in `Camel.toml`.

### Hot-reload

Hot-reload is **off by default**. Enable it with `--watch`, or set `watch = true`
in the active profile of `Camel.toml`:

```toml
[development]
watch = true
```

```bash
CAMEL_PROFILE=development camel run   # watch ON via profile
camel run --watch                      # watch ON via flag
camel run --no-watch                   # watch OFF, overrides Camel.toml
```

### Health

```bash
camel run routes/*.yaml --health-port 8080
```

While the watcher is active, edit any watched YAML file and save — the route
diff is computed and applied within ~300 ms:

| Change | Effect |
|--------|--------|
| Edit pipeline steps | Atomic `swap_pipeline` — zero downtime |
| Add a new route | Route compiled and started |
| Delete a YAML file | Corresponding routes stopped and removed |

### Route YAML format

```yaml
routes:
  - id: "my-route"
    from: "timer:tick?period=1000"
    steps:
      - log: "message=Hello from ${routeId}"
      - to: "mock:out"
```

See [camel-dsl](../crates/camel-dsl) for the full step reference.

## Configuration reference

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `routes` | `[String]` | `[]` | Glob patterns for route YAML files |
| `watch` | `bool` | `false` | Enable file-watcher hot-reload |
| `runtime_journal_path` | `String?` | disabled | Optional flag to enable local runtime durability/replay |
| `log_level` | `String` | `"INFO"` | Tracing log level |
| `timeout_ms` | `u64` | `5000` | Default operation timeout (ms) |
| `components.timer.period` | `u64` | `1000` | Default timer period (ms) |
| `components.http.connect_timeout_ms` | `u64` | `5000` | HTTP connect timeout (ms) |
| `components.http.max_connections` | `usize` | `100` | HTTP connection pool size |
| `observability.metrics_enabled` | `bool` | `false` | Enable metrics endpoint |
| `observability.metrics_port` | `u16` | `9090` | Metrics server port |
| `observability.health.enabled` | `bool` | `false` | Enable standalone health server |
| `observability.health.port` | `u16` | `8080` | Health server port |
| `supervision.max_attempts` | `u32?` | `5` | Max route restart attempts (`null` = unlimited) |
| `supervision.initial_delay_ms` | `u64` | `1000` | Initial restart delay (ms) |
| `supervision.backoff_multiplier` | `f64` | `2.0` | Backoff multiplier per retry |
| `supervision.max_delay_ms` | `u64` | `60000` | Max restart delay cap (ms) |

Environment variable overrides use the `CAMEL_` prefix, e.g.
`CAMEL_LOG_LEVEL=DEBUG`.

## Example

See [`examples/camel-cli-run`](../../examples/camel-cli-run) for a ready-to-run
project layout with a `Camel.toml` and example routes.

## License

Apache-2.0
