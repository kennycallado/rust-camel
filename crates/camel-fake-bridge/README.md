# camel-fake-bridge

Static fake JMS bridge used as a stand-in bridge process inside shell-less
container smoke tests.

- Test-fixture only: `publish = false`, never shipped.
- Serves the `jms_bridge.BridgeService` proto from the real JMS bridge so the
  wire contract matches; only the `health` RPC is implemented.
- TLS-only gRPC: reads `QUARKUS_HTTP_SSL_PORT` (default `0`) and the
  `QUARKUS_TLS_BRIDGE_*` PEM environment variables.
- Prints exactly one ready line after binding: `{"status":"ready","port":N}`.
- Exits cleanly on SIGTERM.

Build for containers with:

```sh
cargo build -p camel-fake-bridge --release --target x86_64-unknown-linux-musl
```
