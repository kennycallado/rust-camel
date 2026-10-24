# Design: jms-tls-classic

## Approach

Provider-neutral per-broker TLS table, Classic-only implementation, shared seams.
Decisions D1-D6 from the pre-design are binding.

**Rust config (`camel-jms/src/config.rs`).** `BrokerTlsConfig
{ ca_cert_path, client_cert_path, client_key_path, server_name }` (all
`Option<String>`, `deny_unknown_fields`) as `Option<BrokerTlsConfig>` field
`tls` on `BrokerConfig`. Validation at startup, config-class permanent:

1. A `tls` table requires every URL member to be `ssl://`; a `tcp://` member
   with `tls`, or a mixed `tcp`+`ssl` composite, is rejected (plaintext
   downgrade would be silent).
2. `client_cert_path` ⇔ `client_key_path`.
3. `server_name` non-empty, syntactically valid DNS hostname, not an IP literal
   (`SNIHostName` rejects IPs); allowed only on TLS URLs.
4. Paths must exist, be readable, contain no `placeholder-` marker.
5. `broker_type = artemis` + `tls` rejected in v1 (M4: rc-jce8q, rc-ewfom).
6. `ws://`/`wss://` rejected for `activemq` (L11: no such transport in the
   5.19.10 client jar); `nio://`/`nio+ssl://` remain rejected.
7. A broker URL containing `jms.clientID` is rejected (L3: collides with
   endpoint-scoped identity, breaks pooled connections).
8. Debug/error output redacts; broker URLs pass through `redact_broker_url`.
9. On any `ssl` member (standalone or composite), member query options outside
   the `jms.*` namespace are rejected — this kills the
   `socket.verifyHostName=false` / `transport.*` verification-weakening class
   before factory construction. On composite URLs, outer options are
   restricted to `jms.*` plus the documented reconnect knobs
   (`maxReconnectAttempts`, `startupMaxReconnectAttempts`); the `nested.`
   prefix and any other outer option are rejected (`nested.` forwards into
   member transports and would bypass member validation).

Composite parsing (D4): both `failover://member` and canonical
`failover:(m1,m2,…)?opts` are accepted for Classic; members are parsed (never
rewritten) and each validated against `{tcp, ssl}`.

**Env channel (`camel-bridge/src/process.rs`).** `BridgeProcessConfig::jms`
emits `BRIDGE_BROKER_TLS_CA_CERT_PATH`, `BRIDGE_BROKER_TLS_CLIENT_CERT_PATH`,
`BRIDGE_BROKER_TLS_CLIENT_KEY_PATH`, `BRIDGE_BROKER_TLS_SERVER_NAME` — paths
only, no secrets — exactly like `JMS_POOL_*` knobs. No new proto config
message. Because the child inherits the parent environment, the spawn removes
(`.env_remove`) all four `BRIDGE_BROKER_TLS_*` keys before injecting this
broker's values, so a stale operator export can never reach a broker without
a `tls` table. Legacy Artemis `BRIDGE_BROKER_KEYSTORE_*` behaviour untouched.

**Java (`bridges/jms`).** The TLS seam activates by URL transport security,
not by env presence: any Classic factory whose URL has an `ssl` member
(standalone or composite) is built as `ActiveMQSslConnectionFactory`; key and
trust managers are installed only when the env is present, otherwise the
platform default trust manager applies (logged at `info!`). Managers are
built at run time from PEM (`CertificateFactory` / PKCS#8 `KeyFactory`; never
static initializers — image heap) and installed with
`setKeyAndTrustManagers(km, tm, secureRandom)`. `ca_cert_path` replaces
platform trust (never adds). A bridge-owned `SslTransportFactory` subclass
registered for scheme `ssl` (`initNativeImageWorkarounds` eager registration
+ `knownServices` + `NativeReflectionConfig` defence in depth) removes the
native `scheme NOT recognized` class by construction.

`server_name` identity binding (e_opus + e_gpt forensics, settled against
the pinned 5.19.10 jar bytecode 2026-10-23): the JDK trust manager FALLS BACK
to the dialed peer host when the SNI name mismatches, and ActiveMQ 5.19.10
`SslTransport.initialiseSocket` OVERWRITES the socket's server-name list with
`remoteLocation.getHost()` (verified: the class constant pool carries
`SNIHostName`/`setServerNames`/`getHost`, and no `sniHost` option exists).
Strict identity therefore requires BOTH halves, public JDK APIs only:
1. Peer-host relabeling — the bridge-owned `SSLSocketFactory` throws
   `SocketException` from no-arg `createSocket()` (forcing `TcpTransport`'s
   connected-socket path), then `createSocket(InetAddress, int)` relabels via
   `InetAddress.getByAddress(server_name, addr)` (resolved IP dialed, no
   extra DNS lookup), making the socket's peer host `server_name`.
2. SNI preservation — the factory returns a delegating `SSLSocket` whose
   `setSSLParameters` applies the caller's parameters and then re-asserts
   serverNames `[SNIHostName(server_name)]` with endpointIdentification
   algorithm `HTTPS`. This survives `initialiseSocket`'s overwrite (which
   runs during `connect()`, before the lazily started handshake).
With SNI equal to the peer host (`server_name`), a mismatch rethrows and the
dialed host is never consulted — either half alone is too weak (the other
name would pass through the fallback). Without `server_name`, the URL host
is dialed and verified normally (DNS→DNS SAN, IP→IP SAN). Mandatory JVM
tests run through the ACTUAL ActiveMQ transport path (SslTransport + the
bridge factory, not raw sockets): SAN = dialed host only + `server_name` set
to a different name → handshake MUST fail; SAN = `server_name` → passes. If
either pinned test fails, the worker STOPS and escalates; custom hostname
matchers or weakened checking are out of bounds.

Composite downgrade guard (e_opus code forensics 2026-10-23): ActiveMQ
`failover:` transports accept broker-directed replacements through TWO
independent switches — advertised URI lists (`updateURIsSupported`) and
`ConnectionControl.getReconnectTo()` (`reconnectSupported`) — so BOTH are
disabled. The bridge-owned factory subclass overrides the protected
`createActiveMQConnection(Transport, JMSStatsImpl)`, walks
`transport.narrow(FailoverTransport.class)` (null-safe: a plain `ssl://`
transport narrows to null), and sets `updateURIsSupported=false`,
`reconnectSupported=false`, leaving `updateURIsURL` unset. This hook runs
inside `createConnection` before `transport.start()`, i.e. before any I/O and
before any `ConnectionControl` can arrive. The guard applies to every
composite containing an `ssl` member, with or without a `tls` table
(verification is always on); ordinary failover between configured members
still works. A JVM test pins both flags.

Startup self-check resolves every configured URL member scheme (composite
included) against registered transport factories before any connect; failure
aborts with `UNSUPPORTED_TRANSPORT`. The probe reuses `buildActiveMqFactory`,
so it is covered by the same seam.

**Diagnostics (D6).** New `BrokerFailureClassifier` maps exception cause chains
by type first (`CertPathValidatorException`, `CertificateException`,
`SSLHandshakeException`+alert, `ConnectException`, `JMSSecurityException`, …)
to the closed enum `NETWORK, TLS_TRUST, TLS_HOSTNAME, TLS_CLIENT_AUTH,
TLS_PROTOCOL, TLS_PLAINTEXT_PEER, UNSUPPORTED_TRANSPORT, AUTH,
CLIENT_ID_IN_USE, OTHER`. Hostname mismatch has no dedicated public exception
type: a `CertificateException` in the chain with no
`CertPathValidatorException` cause is classified `TLS_HOSTNAME` when its
message matches the JDK's fixed hostname-mismatch text family (`No name
matching … found`, `No subject alternative names matching IP address …
found`, `No subject alternative DNS name matching … found`, `No subject
alternative names present`) —
these are JDK strings, not remote text; all other message matching stays
limited to JDK alert texts, and remote broker text is never placed on the
wire. Java logs details locally at `warn!`; the wire carries only the class.
Rust renders local actionable text per class and never echoes remote text
(`consumer.rs:169-180` policy). `TLS_HOSTNAME` renders: broker name, expected
`<host or server_name>`, redacted connected host, and both remediations
(certificate-naming address, or `brokers.<name>.tls.server_name`).

**Capability gate (D5).** Both proto copies gain, additively:
`repeated string capabilities = 6;` and `BrokerFailureClass failure_class = 7;`
(+ enum). Capabilities describe the bridge build (provider-neutral); the
bridge reports `broker-tls-v1`. Rust checks during the existing startup
Health call — the gate fires only when a `tls` table is configured:
capability absent → kill the bridge, permanent failure naming the
binary/version. Proto3 default (empty) fails closed for old bridges.

**Native live proof.** Hermetic tiers cannot prove transport registration,
JSSE-in-image, or the SNI path. That proof is a testcontainer Classic broker
with an `ssl://` connector (rcgen CA + `p12-keystore`, `support/cxf.rs`
pattern) and belongs to CI; this mission leaves an `#[ignore]`-gated smoke
entry point in `camel-tests` and says so in docs.

## Affected crates

- `camel-jms`: `BrokerTlsConfig`, validation, proto copy, health handling,
  capability gate, failure-class rendering, consumer diagnostics.
- `camel-bridge` (service): TLS env emission + tests.
- `bridges/jms` (Java): factory seam, transport factory + native registration,
  classifier, proto, JVM tests.
- `camel-tests`: ignored native-live smoke module.
- `docs`: `jms.md` TLS section (by-IP guidance, L8 migration note), bridge
  READMEs (L1 fix).

## Architecture boundaries

Components: `camel-jms` owns config/validation/diagnostics. Services:
`camel-bridge` owns process spawn env only. Bridge (Java): owns all JSSE
plumbing; no global `SSLContext.setDefault`, no `javax.net.ssl.*` system
properties. Runtime/DSL/Languages: untouched.

## Artemis parity analysis (mission item 7; no Artemis code here)

| Surface | Classic after M1 | Artemis after M1 | Convergence blocker | Follow-up |
|---|---|---|---|---|
| `tls` table (PEM) | full (trust-only, mTLS) | none — `tls` table rejected at validation | Artemis path needs PKCS12 keystores or a pem-keystore dep; existing keystore env is process-global | M4 (rc-jce8q) |
| Trust-only TLS | yes | impossible (keystore mandatory, L5) | `buildArtemisFactory` requires a keystore even for trust | M4 (rc-jce8q) |
| Hostname verify | always on, SNI override supported | connector `verifyHost` default true but not explicit; no SNI override | `verifyHost`/`sniHost` connector params not surfaced; 2.36 default unread (L9) | M4 (rc-ewfom) |
| Env scope | per-broker `BRIDGE_BROKER_TLS_*` | legacy process-global `BRIDGE_BROKER_KEYSTORE_*` (widens to every broker in the pool) | child inherits parent env; no per-broker scoping | M4 (rc-jce8q) |
| `failover:` | `failover://` + `failover:(…)` accepted | all failover forms rejected (unchanged) | Artemis locator composition differs | M4 analysis |
| Capability | `broker-tls-v1` reported by the capable bridge build regardless of provider; gate fires only on a configured `tls` table (Classic-only in v1) | same build capability present; Artemis `tls` tables remain rejected at validation | gate is provider-neutral by construction | free (no work) |

Exact follow-up: implement M4 on the shared `BrokerTlsConfig` (it is
provider-neutral by construction), retiring the keystore env contract and
surfacing `verifyHost`/`sniHost` explicitly.

## Alternatives considered

- New proto config message for TLS material — rejected: mission correction; the
  established tuning channel is spawn env vars.
- URL keystore/`socket.*` options on `ssl://` URIs — rejected: URL transport
  options remain forbidden (`jms` spec), and reflective setter application
  fails in native.
- `javax.net.ssl.*` system properties — rejected: global, leaks to the gRPC
  stack, bakes into native images.
- SNI-only identity binding — rejected: the JDK trust manager falls back to
  the dialed peer host when the SNI name mismatches, so a wrong
  `server_name` would silently pass; superseded by peer-host relabeling.
- Custom hostname verifier/matcher — rejected: security-sensitive matching
  code; peer-host relabeling achieves the same guarantee with public JDK
  APIs only.
