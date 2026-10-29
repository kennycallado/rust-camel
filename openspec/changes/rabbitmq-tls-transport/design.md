# Design: rabbitmq-tls-transport

## Approach

lapin 4.12.0 exposes exactly one TLS injection point: `OwnedTLSConfig { identity,
cert_chain }` handed to `DefaultConnectionBuilder::with_tls_config`. `cert_chain`
is a PEM string that tcp-stream parses and adds to the rustls connector's root
store. amq-protocol-tcp 10.6.3 derives the TLS SNI and certificate-verification
name from the AMQPUri authority host (`lib.rs:148`) with no override seam, and
lapin has no adopt-a-preconnected-stream API. Verified against the vendored
sources and ruled on by e_gpt (2026-10-29, mission 446): implement `ca_cert`,
fail closed on `server_name`.

### Config layer (`config.rs`)

`RabbitBrokerConfig` gains `tls: Option<RabbitTlsConfig>`. `RabbitTlsConfig` is
`deny_unknown_fields` with `ca_cert: Option<String>` (PEM path) and
`server_name: Option<String>`. Debug redaction is preserved; `ca_cert` is a path
and adds no secret material.

### Boot validation — ALL brokers, named diagnostics

Validation runs at boot in `RabbitMqBundle::from_toml` (the fallible parse
point) through a new `RabbitComponentConfig::validate() -> Result<,
CamelError>` that iterates every configured broker with its NAME, so an unused
broker cannot escape validation. Per broker, checked in this exact precedence:

1. `tls.server_name` present (any value, including empty or equal to the URL
   host) → boot error, verbatim ruling text:
   `rabbitmq broker '{broker}': tls.server_name is unsupported by lapin 4.12.0;
   TLS SNI and certificate verification use the broker URL host; remove
   tls.server_name`. Unconditional — it wins over checks 2 and 3. Never
   includes the override value, broker URL, credentials, certificate paths, PEM
   contents, or underlying errors.
2. `tls` present and the broker URL scheme is `amqp` → boot error naming the
   broker: TLS parameters require `amqps://`. URL and credentials are not
   echoed (scheme name only).
3. `tls.ca_cert` present → read + PEM-parse at boot: a missing or unreadable
   file, a file with no PEM certificates, or malformed certificate data →
   boot error naming the broker and the configured path. PEM contents,
   credentials, and URL are never included.

### Transport wiring (`connection.rs`)

`RabbitConnectionManager::from_broker_config` becomes
`try_from_broker_config(broker, retry) -> Result<Self, CamelError>`: it
re-loads and re-parses the CA bundle at manager (endpoint/connection)
construction — failing closed with the same named-broker error if the file
changed or vanished since boot — and installs a TLS-aware `ConnectFn` closure
capturing `OwnedTLSConfig { identity: None, cert_chain: Some(pem) }` connecting
through `DefaultConnectionBuilder`. The `amqp://` path keeps the existing
plaintext `connect_with_lapin` adapter. `Plain` means "no custom CA capture",
never "plaintext for amqps": an `amqps://` URL may select the `Plain` adapter
(no custom CA configured), and lapin's AMQPS scheme handling still performs
the TLS handshake — TLS is chosen from the URL scheme alone, so no plaintext
downgrade exists. Reconnects reuse the captured closure, so TLS settings
survive every reconnect. Trust policy: system native roots (lapin
`rustls-native-certs`
feature) PLUS the custom CA — additive. Ring provider via the existing
`rustls--ring` lapin feature; PEM parsing uses `rustls-pki-types` (already in
the dependency graph via rustls; no new crates).

## Affected crates

- `camel-component-rabbitmq`: config struct + boot validation, connect seam,
  tests, README.
- `docs/src/components/rabbitmq.md`: TLS block, `amqps://` requirement,
  URL-host verification/SNI behavior, all fail-closed cases.

## Architecture boundaries

Components-layer only. No Runtime, DSL, Services, Languages, or Functions
changes. Errors follow the component-error-taxonomy (`CamelError::Config`).
Crypto provider stays ring per rc-2bofp; lapin features unchanged, no aws-lc
edge introduced.

## Test coverage model

- Unit (no broker): config parse/reject matrix; boot validation matrix
  (precedence, verbatim server_name text, named broker); PEM load failures;
  production-path seam assertions — an `amqps://` broker installs the
  TLS-carrying `ConnectFn` (assert captured `cert_chain` equals the PEM
  contents and `identity` is None), an `amqp://` broker keeps the plaintext
  adapter, and the captured TLS config is the one reused across reconnects.
- Local TLS harness (no broker, runs in the default `cargo test`): a tokio-rustls
  test server (static committed fixtures: test CA, server cert/key for
  `localhost`, a wrong CA) proves at the REAL connection boundary that the
  captured `OwnedTLSConfig` performs the handshake — the client closure
  (`amqps://localhost:<ephemeral port>`) completes a TLS handshake against the
  fixture CA twice (repeated invocation / reconnect reuse); the plain adapter
  fed an `amqp://` URL shows plaintext AMQP bytes server-side (the `amqps://`
  scheme always TLS-handshakes even through the plain adapter — `Plain` means
  "no custom CA", never "plaintext for amqps"), and the harness fails if
  `with_tls_config` injection is removed (system roots reject the self-signed
  server cert).
- Broker-gated e2e (env gate `RABBITMQ_TLS_ITEST`, mirroring the crate's
  `RABBITMQ_ITEST` docker-gate convention — early-return skip notice, never a
  silent failure): custom-CA handshake success (`RABBITMQ_TLS_URL` +
  `RABBITMQ_TLS_CA`), untrusted-CA rejection (checked-in wrong-CA fixture),
  hostname-mismatch rejection (`RABBITMQ_TLS_MISMATCH_URL`). NOT `#[ignore]`:
  the repo's ignore-test-policy (ADR-0054) forbids `#[ignore]` on
  external-service tests and `lint-ignore` enforces it; the mission's
  `#[ignore]` wording is reconciled to this env-gate mechanism (mission intent
  — e2e never required in CI — is preserved; the park reports the
  reconciliation). If no TLS broker is available, the park states e2e was not
  runnable and lists the unit + harness coverage in its place.

## Alternatives considered

- Honour `server_name` by URI-host rewrite: rejected — silently changes the TCP
  connect target; not an SNI override (e_gpt ruling).
- Bump lapin/amq-protocol for a server_name seam: rejected — unverified,
  dependency-bump risk outside this mission's scope.
- webpki bundled roots: rejected — lapin is compiled with `rustls-native-certs`;
  switching the root policy would change behavior for existing `amqps://` users.
- Validate lazily in `manager_for`: rejected (bless finding) — unused brokers
  would escape validation; boot must reject all configured brokers.

## Spec note

No delta spec: no rabbitmq transport capability spec exists in
`openspec/specs/` (mission 446 Required 5). `skip_specs: true` is set in
`.openspec.yaml`; the park reports this.
