# Proposal: jms-tls-classic

## Why

camel-112 cannot use `ssl://` against ActiveMQ Classic. Rust accepts the scheme
(`camel-jms/src/config.rs` known-schemes) but the Classic Java path hands the raw
URL to `new ActiveMQConnectionFactory(url)` with no TLS seam
(`JmsClientFactory.buildActiveMqFactory`), and in native mode the `ssl` transport
factory is not registered, so the connect fails with
`Transport scheme NOT recognized: [ssl]`. A TLS-enabled Classic broker is
therefore unusable in every mode. bd rc-c4fbo (M1). Pre-design: Decisions D1-D6
in `.opencode/fleet/inbox/e_opus-rc-c4fbo-investigation.md` — not re-litigated here.

## What Changes

- Per-broker `tls` table on `BrokerConfig` (`ca_cert_path`, `client_cert_path`,
  `client_key_path`, `server_name`), provider-neutral by construction, with
  fail-closed validation (D2 matrix, rules 1-8).
- Spawn env vars `BRIDGE_BROKER_TLS_{CA_CERT_PATH,CLIENT_CERT_PATH,CLIENT_KEY_PATH,SERVER_NAME}`
  on the established env channel (no new proto config message).
- Java: `buildActiveMqFactory` builds an `ActiveMQSslConnectionFactory` and
  installs key/trust managers from PEM in code; bridge-owned `ssl` transport
  factory registration (JVM + native reflection config); `server_name`
  support by binding it as the socket's peer host (public JDK APIs — the JDK
  trust manager falls back to the dialed host on SNI mismatch, so SNI alone
  is insufficient); startup scheme self-check; `BrokerFailureClass`
  classifier that never echoes remote text (D6). The health probe shares the
  factory seam, so it is covered automatically.
- Proto (additive only): `HealthResponse.capabilities = 6` (repeated string,
  `"broker-tls-v1"`) and `HealthResponse.failure_class = 7` (new
  `BrokerFailureClass` enum). Rust refuses TLS when the capability is absent (D5).
- Classic `failover:(...)` composite syntax accepted (D4); mixed `tcp`+`ssl`
  members rejected; outer composite options restricted to `jms.*` plus the
  two documented reconnect knobs (`nested.` rejected); ssl composites get
  broker-directed transport replacement disabled programmatically;
  `ws`/`wss` rejected for Classic (L11); `jms.clientID` in a
  broker URL rejected (L3).
- Docs/specs: `bridges/jms/README.md` Classic+ssl claim fixed (L1), `jms.md` TLS
  section with by-IP guidance, spec amendments to `bridge-transport-security`
  and `jms` (bridge-owned transport registration allowed; still no URL
  transport options).

Excluded (M2/M3/later): durable topic subscribers, Artemis TLS parity (M4,
rc-jce8q / rc-ewfom), selector plumbing (rc-d1cz9), unknown-endpoint-param
policy (rc-d8svj), `ws`/`wss` support, pool changes, version bumps.

## Acceptance criteria

- `ssl://` Classic works with pinned CA, platform trust, and mTLS configurations;
  verification (chain + name) is always on; no skip knob exists.
- The trust/identity matrix is covered per D2, including its negatives and the
  rows a first pass might skip: platform-trust mTLS (client pair, no CA),
  `needClientAuth` without a client certificate fails with `TLS_CLIENT_AUTH`,
  an IP-SAN certificate dialed by IP succeeds, and a pinned private CA
  replaces (never adds to) platform trust — a certificate trusted by the
  platform but not by the pinned CA fails with `TLS_TRUST`.
- Hostname mechanism proven on the JVM including the by-IP case with and without
  `server_name`; wrong CA, hostname mismatch, and mixed `tcp`+`ssl` failover
  fail closed in tests.
- Bridge reports `broker-tls-v1`; Rust kills the bridge and fails permanently
  when TLS is configured but the capability is absent.
- Composite failover cannot downgrade: a mixed `tcp`+`ssl` composite is
  rejected at validation, and with TLS configured the bridge disables
  broker-advertised failover URI updates so a `tcp://` replacement can never
  take effect at runtime.
- Handshake/transport failures classify into the fixed `BrokerFailureClass` set;
  `TLS_HOSTNAME` renders a local message naming both remediations.
- Native TLS against a real Classic broker is the only full proof and belongs to
  CI; an ignored smoke entry point is left in `camel-tests`.
- Full AGENTS.md gate registry plus Java bridge module tests pass.

## Risk budget

Medium: the native transport/introspection surface is the main risk (mitigated
by eager registration + reflection config + JVM tests); SNI-as-identity relies
on JDK trust-manager behaviour, pinned by the JVM hostname tests (residual R1).
Acceptable: hermetic JVM/Rust tiers only locally; native live proof deferred to
CI. Out of bounds: weakening verification, global `javax.net.ssl.*` system
properties, static TLS material initialization (native image heap), URL
rewriting.

Affected crates: `camel-jms`, `camel-bridge` (service), `bridges/jms` (Java),
`camel-tests` (ignored smoke), `docs`.
