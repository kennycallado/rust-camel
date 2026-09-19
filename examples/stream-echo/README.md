# stream-echo

Interactive stdio echo: lines from stdin come back on stdout with an
`echo: ` prefix. A `Enter text:` prompt goes to stderr. The route is
data only — no log component, no tracer sink.

```
stdin ──▶ stream:in ──▶ transform (echo: ${body}) ──▶ stream:out ──▶ stdout
                                        └── set_body("Enter text:") ──▶ stream:err ──▶ stderr
```

The context boots through the full `camel run` component cascade
(`camel_bundles::boot`, ADR-0069 section 10). The cascade registers the
stream component (ADR-0080) under scheme `stream`.

## Run it

```sh
cargo run -p stream-echo
```

## Interactive (a TTY)

Type a line and press Enter. The line comes back on stdout as
`echo: <line>`; the prompt re-appears on stderr. Leave with Ctrl-C
(graceful, exit 0).

A prompt cannot appear before the first line you type: the route runs
once per exchange, and no exchange exists until stdin delivers a line.
So the prompt postcedes the transform and re-shows after each echoed
line.

## Batch (a pipe)

```sh
printf 'alpha\nbeta\n' | cargo run -p stream-echo
```

Lifecycle: EOF completes the `stream:in` consumer's ROUTE, not the
process. Like `camel run`
(`crates/camel-cli/src/commands/run.rs`), this process is
signal-managed: it waits for the first SIGINT/SIGTERM, then tears down
with exit 0. Under a pipe, wait for the echoed lines to drain, then
send one signal to the background process:

```sh
cargo run -p stream-echo & pid=$!
printf 'alpha\nbeta\n'          # the pipe feeds stdin
# after `echo: alpha` and `echo: beta` appear:
kill -TERM $pid                 # graceful exit 0
```

A practical one-shot form that routes the payload through `camel run`
directly (this is the parity test scenario,
`crates/camel-cli/tests/stream_run_pipe.rs`):

```sh
printf 'alpha\nbeta\n' | \
  RUST_LOG=off camel run --routes crates/camel-cli/tests/fixtures/stream-echo-route.yaml
```

Wait for the `echo: ` lines, then send SIGTERM or SIGINT; the process
exits 0.

## Data and diagnostics discipline

- `stdout` is data: only `echo: ` lines.
- `stderr` is diagnostics: the prompt, the banner, shutdown notes.
- The runtime's general log layer writes to stdout. When you pipe this
  example or `camel run`, set `log_level = "WARN"` (this example's
  `Camel.toml`) or `RUST_LOG=off` so log lines cannot mix into the
  data. If the tracer stdout sink is also enabled, `camel run` warns
  about the collision at boot (ADR-0080 posture: warn, never re-route).
