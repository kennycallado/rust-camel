# camel-cli-no-exec example

A minimal `camel run` project that starts a route without the exec
component. This example runs on a default-features build with no exec
configuration.

## Purpose

Demonstrate that `camel run` starts non-exec routes without requiring exec
profiles. Before this fix, the exec fail-closed validation aborted startup
for any route when no exec profiles were configured.

## Reproduce

```bash
cargo build -p camel-cli
cd examples/camel-cli-no-exec
../../target/debug/camel run --no-watch
```

The route starts and logs a message every 2 seconds. Press Ctrl+C (or send
SIGTERM) to stop.

## Root cause

The exec component is a default cargo feature. Its fail-closed validate()
rejected zero profiles unconditionally at startup. Now the CLI registers
the exec bundle only when a route uses `exec:` or the operator declares
`[components.exec]`.

## Known limitation (hot-reload)

If exec was neither used nor declared at startup, introducing the first
`exec:` usage via hot-reload (`--watch`) fails to resolve that endpoint
and requires a restart. If exec was configured at startup, later exec
routes via reload work normally.
