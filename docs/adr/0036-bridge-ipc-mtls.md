# ADR-0036: Bridge IPC mutual TLS

Date: 2026-07-09

## Status

Accepted

## Context

Bridge subprocesses (JMS, XML, CXF) communicate with the Rust runtime via gRPC
over localhost. Until this ADR, the channel used HTTP/2 cleartext (h2c).
Sensitive data flowing through plaintext included:

- JMS messages + broker credentials
- SOAP/XML payloads + WS-Security credentials
- Transformation payloads (arbitrary user data)

ADR-0033 identified this as a defense-in-depth gap: any local process (or
container in a shared network namespace) can sniff or intercept bridge traffic.

## Decision

Replace h2c with mutual TLS (mTLS) using ephemeral certificates:

1. **Cert generation**: The Rust parent generates an ephemeral CA + server cert
   + client cert via rcgen on each `BridgeProcess::start()`. Certs are valid for
   90 days and written to a 0700 TempDir cleaned up on drop.

2. **Quarkus native image**: Build-time TLS properties (`use-separate-server`,
   `plain-text`, `ssl.client-auth`, `insecure-requests`, `tls-configuration-name`)
   are hardcoded in `application.yml`. Placeholder self-signed certs in
   `src/main/resources/tls/` enable SSL at native build time. Runtime cert paths
   are overridden via `${ENV:default}` expressions.

3. **Fail-closed guard**: PortAnnouncer checks that resolved cert paths do NOT
   contain `placeholder-`. If env vars are absent or unresolvable, the bridge
   aborts before readiness.

4. **Connection retry**: `connect_channel()` retries the TLS handshake 10×100ms
   because Quarkus PortAnnouncer fires on `StartupEvent`, which can precede full
   SSL listener readiness in native images.

5. **`quarkus-config-yaml`**: Required Gradle dependency. Without it, Quarkus
   silently ignores `application.yml` — all YAML config is inert.

## Consequences

- Bridge IPC is encrypted and mutually authenticated.
- No persistent cert management needed — certs are ephemeral per process lifecycle.
- Bridge binaries must be rebuilt when TLS config changes (build-time fixed props).
- JVM-mode tests require test profile override (`src/test/resources/application.properties`).
- Native build skips Java tests (`-x test`) because TLS config breaks them in JVM mode.
