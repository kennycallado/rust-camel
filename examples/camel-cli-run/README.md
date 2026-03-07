# camel-cli-run example

A ready-to-run project layout showing how to use `camel run` with a
`Camel.toml` config file and YAML routes.

## Layout

```
camel-cli-run/
├── Camel.toml          # Context configuration + route patterns
└── routes/
    ├── hello.yaml      # Timer route: logs a greeting every 2s
    └── transform.yaml  # Timer route: set_body + set_header every 5s
```

## Running

First build or install the CLI:

```bash
# from the workspace root — build without installing
cargo build -p camel-cli

# then run from this directory
cd examples/camel-cli-run
../../target/debug/camel run
```

Or install globally:

```bash
cargo install camel-cli
cd examples/camel-cli-run
camel run
```

## Hot-reload

Hot-reload is **disabled by default**. Enable it with `--watch` or via the
`development` profile (which sets `watch = true` in `Camel.toml`):

```bash
# Enable hot-reload via flag
../../target/debug/camel run --watch

# Enable hot-reload via profile
CAMEL_PROFILE=development ../../target/debug/camel run

# Explicitly disable (overrides Camel.toml)
../../target/debug/camel run --no-watch
```

While the watcher is active, edit any file in `routes/` and save — changes
are picked up within ~300 ms without restarting:

```bash
# In a second terminal, while `camel run` is active:
echo 'routes:
  - id: "hello"
    from: "timer:tick?period=2000"
    steps:
      - log: "message=Hot-reloaded! counter=${header.CamelTimerCounter}"
' > routes/hello.yaml
```

You will see the log message change on the next tick.

## Profiles

The `Camel.toml` ships with `development` and `production` profiles:

```bash
CAMEL_PROFILE=development ../../target/debug/camel run   # DEBUG logs + watch=true
CAMEL_PROFILE=production  ../../target/debug/camel run   # WARN logs, watch=false
```

## Overriding routes via flag

The `--routes` flag takes precedence over `Camel.toml`:

```bash
../../target/debug/camel run --routes "routes/hello.yaml"
```
