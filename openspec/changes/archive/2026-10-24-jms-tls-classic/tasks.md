# Tasks: jms-tls-classic

Single-phase change (no `## Phase N` grouping). Tasks are ordered; each ends
with its progress checkbox. Spec references point at the blessed deltas in
`specs/bridge-transport-security/spec.md` (BTS) and `specs/jms/spec.md` (JMS).

## camel-jms: Rust config surface and validation

### Task 1.1: BrokerTlsConfig struct + fail-closed validation + composite failover parsing

**Files:**
- `crates/components/camel-jms/src/config.rs` (modified)

**Steps:**
1. Add `#[derive(Clone, PartialEq, serde::Deserialize, Debug)] #[serde(deny_unknown_fields)] pub struct BrokerTlsConfig { pub ca_cert_path: Option<String>, pub client_cert_path: Option<String>, pub client_key_path: Option<String>, pub server_name: Option<String> }` next to `BrokerConfig` (config.rs, after line 539). Plain `Debug` is compliant: the fields are paths/names, never secrets — file CONTENTS are never read in Rust, so they cannot appear in any output (the spec prohibits contents, not paths).
2. Add field `pub tls: Option<BrokerTlsConfig>` to `BrokerConfig` (kebab-case serde, consistent with existing fields).
3. Add `fn composite_members(broker_url: &str) -> Vec<String>` — for `failover://host:port?opts` returns `["host:port?opts"]`-style member (strip the `failover://` prefix); for `failover:(m1,m2,…)?opts` returns the parenthesized member list verbatim (never rewritten); for a plain `scheme://…` URL returns the URL itself. Handle nested parentheses absence (Classic members do not nest), surrounding whitespace, and an empty member list (error).
4. Implement `fn validate_broker_tls(broker: &BrokerConfig) -> Result<(), String>` called from the existing broker validation path, enforcing, in order: (a) `broker_type == BrokerType::Artemis && tls.is_some()` → error naming the Artemis TLS follow-up; (b) transport-security uniformity INDEPENDENT of tls presence: for a composite, all members must carry the same security class — any mix of `tcp` and `ssl` members is rejected with an error naming the mixed members (plaintext downgrade would be silent during failover); for a plain URL this is vacuous; (c) if `tls` present, every member must parse to scheme `ssl` — a `tcp` member (standalone or in a composite) with a `tls` table is rejected; (d) `client_cert_path` XOR `client_key_path` → error naming the incomplete pair; (e) `server_name` present but URL has no `ssl` member → error; `server_name` empty, an IP literal (`Ipv4Addr`/`Ipv6Addr` parse or contains `:`), or not a syntactically valid DNS hostname (each label 1-63 chars matching `[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?`, dot-separated, total length ≤253) → error naming the rule; (f) on any `ssl` member, member query options outside the `jms.` namespace are rejected; on composite URLs, outer options are restricted to `jms.*`, `maxReconnectAttempts`, `startupMaxReconnectAttempts` — the `nested.` prefix and anything else is rejected; (g) URL containing query key `jms.clientID` (any member or outer) → error naming client identity as endpoint-scoped; (h) scheme allowlist per member for `activemq`: `tcp`, `ssl`, `failover` — `ws`, `wss`, `nio`, `nio+ssl`, and anything unknown → error naming the unsupported transport (this also enforces the L11 ws/wss rejection and keeps nio rejected); (i) each configured `tls` path must exist, be readable, and its ENTIRE configured path value must not contain the substring `placeholder-` (catches directory markers like `/etc/placeholder-material/ca.pem`, not only file names).
5. All error strings route the broker URL through the existing `redact_broker_url` before inclusion, and never include file contents.
6. Scheme parsing of members reuses the existing known-scheme logic style from `config.rs:678-699` (extend, do not duplicate).

**Tests:** (in the existing `mod tests` of config.rs, following its table-test style)
- `tls_config_accepts_pinned_ca_and_mtls`: broker `ssl://mq:61617` + tls{ca,client pair} → `validate_broker_tls` returns Ok.
- `tls_on_artemis_rejected`: artemis broker + tls table → Err mentioning "artemis".
- `tls_on_plaintext_url_rejected`: `tcp://` + tls → Err mentioning the scheme.
- `mixed_composite_rejected`: `failover:(tcp://a:61616,ssl://b:61617)` (with and without tls) → Err mentioning mixed.
- `all_ssl_composite_accepted`: `failover:(ssl://a:61617,ssl://b:61617)` + tls → Ok.
- `client_cert_without_key_rejected`: tls{client_cert only} → Err naming the pair.
- `server_name_ip_literal_rejected`: server_name = "10.0.0.5" → Err; empty → Err.
- `server_name_on_plaintext_rejected`: server_name with `tcp://` → Err.
- `verify_hostname_socket_option_rejected`: member `ssl://h:1?socket.verifyHostName=false` → Err naming the forbidden option.
- `nested_outer_option_rejected`: `failover:(ssl://h:1)?nested.socket.verifyHostName=false` → Err naming nested.
- `max_reconnect_attempts_outer_accepted`: `failover:(ssl://h:1)?maxReconnectAttempts=10` → Ok.
- `jms_client_id_in_url_rejected`: `ssl://h:1?jms.clientID=x` → Err naming endpoint scope.
- `ws_and_nio_schemes_rejected`: `ws://`, `wss://`, `nio://` for activemq → Err each.
- `unknown_tls_key_rejected`: tls table with key `insecure_skip_verify = true` → serde deserialization fails (deny_unknown_fields), asserting no verification-weakening key exists.
- `placeholder_path_rejected`: ca_cert_path = "/tmp/placeholder-ca.pem" (create the file in the test's tempdir first) → Err naming placeholder; missing file → Err naming unreadable.
- `debug_renders_paths_not_contents`: format!("{:?}", tls) contains the configured path strings (paths are not secrets) and can never contain file contents (contents are never read in Rust — asserted by the struct holding only `Option<String>` paths).
- `valid_dns_server_names_accepted`: server_name = "mq.internal.example", "a.b", "x-y.example" → Ok (guarding the label regex against over-rejection).
- `placeholder_directory_marker_rejected`: ca_cert_path = "<tempdir>/placeholder-material/ca.pem" (create dirs + file) → Err naming placeholder.

**Acceptance:**
- `cargo test -p camel-component-jms --lib config` passes with all new tests green.
- `cargo clippy -p camel-component-jms --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.
- No `unwrap()`/`expect()` added (existing lint-unwrap policy).

- [x] 1.1

## camel-bridge: TLS env emission

### Task 2.1: Emit BRIDGE_BROKER_TLS_* with env_remove isolation on the spawn Command

**Files:**
- `crates/services/camel-bridge/src/process.rs` (modified)
- `crates/components/camel-jms/src/component.rs` (modified — the caller that constructs `BridgeProcessConfig::jms`)

**Steps:**
1. Add four `pub const` strings next to the existing `JMS_POOL_IDLE_TIMEOUT_MS` consts: `BRIDGE_BROKER_TLS_CA_CERT_PATH`, `BRIDGE_BROKER_TLS_CLIENT_CERT_PATH`, `BRIDGE_BROKER_TLS_CLIENT_KEY_PATH`, `BRIDGE_BROKER_TLS_SERVER_NAME` (values identical to the names).
2. Add `pub struct JmsTlsEnv { pub ca_cert_path: Option<String>, pub client_cert_path: Option<String>, pub client_key_path: Option<String>, pub server_name: Option<String> }` in process.rs (plain data — camel-bridge does not depend on camel-component-jms types).
3. Extend `BridgeProcessConfig::jms(process.rs:342)` with a `tls: Option<JmsTlsEnv>` parameter: when `Some`, push the four keys as `(key, value)` pairs into `env_vars` for each present field. ALSO add a new field `pub env_remove_keys: Vec<String>` to `BridgeProcessConfig` (empty for non-JMS configs) set to the four TLS key names for JMS bridges — these are the keys the spawn must REMOVE so inherited operator exports can never reach the child.
4. In the spawn site (the `start()` function building the `Command`, around process.rs:738-764): BEFORE the existing `for (key, value) in &config.env_vars { command.env(key, value); }` loop, apply `for key in &config.env_remove_keys { command.env_remove(key); }` — the removal targets the actual child `Command`, which is the only place inherited environment can be cut. The subsequent injection loop then sets the configured values. Thread the same config through both the initial and the replacement-startup paths (both construct their Command through this same start() helper — verify and, if replacement startup builds its own Command, apply the identical remove-then-inject sequence there).
5. Update the `BridgeProcessConfig::jms` call site(s) in `crates/components/camel-jms/src/component.rs` to pass `tls` built from the broker's `BrokerTlsConfig` (Task 1.1): map `ca_cert_path` etc. directly; `None` when the broker has no `tls` table.
6. Do not touch `BRIDGE_BROKER_KEYSTORE_*` handling or any other env logic.

**Tests:** (follow the existing `pool_env_test.rs` pattern; the tests assert on the built `Command`'s environment the same way existing process tests observe env — via `std::os::unix::process::CommandExt`-free introspection the existing tests already use, or by asserting on the config fields + a small spawn-to-`env` helper if the existing pattern exposes one)
- `tls_env_emitted_for_configured_broker`: `BridgeProcessConfig::jms(..., tls: Some(JmsTlsEnv{ca, client pair, server_name}))` → `env_vars` contains all four keys with the configured values.
- `tls_env_absent_keys_removed`: config with `tls: None` → `env_remove_keys` contains all four TLS key names (the spawn applies them via `command.env_remove`, verified by the existing spawn-path test pattern asserting the remove list is consumed).
- `partial_tls_emits_only_present`: tls{ca only} → CA pair present in `env_vars`; client cert/key and server_name absent from `env_vars` but their keys still in `env_remove_keys`.

**Acceptance:**
- `cargo test -p camel-bridge --lib process` passes with the new tests.
- `cargo test -p camel-component-jms --lib component` passes (call-site update compiles; NOTE: the package name is `camel-component-jms` while the directory stays `crates/components/camel-jms/`).
- `cargo clippy -p camel-bridge --all-targets -- -D warnings` exits 0; `cargo clippy -p camel-component-jms --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] 2.1

## proto: additive health fields (both copies)

### Task 3.1: BrokerFailureClass enum + HealthResponse capabilities/failure_class

**Files:**
- `crates/components/camel-jms/proto/jms_bridge.proto` (modified)
- `bridges/jms/src/main/proto/jms_bridge.proto` (modified)

**Steps:**
1. In BOTH files, make the identical additive edit after the `HealthResponse` message's existing fields 1-5: add `repeated string capabilities = 6;` and `BrokerFailureClass failure_class = 7;`. Add above `HealthResponse`: `enum BrokerFailureClass { BROKER_FAILURE_CLASS_UNSPECIFIED = 0; NETWORK = 1; TLS_TRUST = 2; TLS_HOSTNAME = 3; TLS_CLIENT_AUTH = 4; TLS_PROTOCOL = 5; TLS_PLAINTEXT_PEER = 6; UNSUPPORTED_TRANSPORT = 7; AUTH = 8; CLIENT_ID_IN_USE = 9; OTHER = 10; }`. No other proto change; fields 1-5 and all existing messages keep their numbers.
2. Do NOT add any config message (mission correction: TLS material travels via env only).
3. Regenerate/verify the Rust side builds (`build.rs` uses tonic-prost-build; generated types are build-time, no checked-in generated code to edit).
4. Extend the copy-equality coverage: the existing test at `crates/components/camel-jms/src/bridge_client_test.rs:65-80` asserts byte-identity of the two proto copies — it must keep passing unchanged (it reads the files, so no edit needed unless it hardcodes line counts).

**Tests:**
- `proto_copies_byte_identical`: existing `bridge_client_test.rs` test → still passes with both files edited identically.
- `health_response_new_fields_compile`: in `crates/components/camel-jms/src/health.rs` tests (or a new small unit test there), construct a `HealthResponse` with `capabilities: vec!["broker-tls-v1".into()]` and `failure_class: BrokerFailureClass::TlsHostname as i32` → compiles and serializes; `default`-constructed HealthResponse has empty capabilities and `Unspecified`.

**Acceptance:**
- `cargo test -p camel-component-jms --lib bridge_client` passes.
- `cargo build -p camel-component-jms` exits 0.
- `git diff` shows the two .proto files with identical added lines (`diff <(git show :crates/components/camel-jms/proto/jms_bridge.proto) <(git show :bridges/jms/src/main/proto/jms_bridge.proto)` exits 0 — the copies remain byte-identical as whole files).

- [x] 3.1

## bridges/jms: transport registration and scheme self-check

### Task 4.1: Bridge-owned ssl transport factory + native registration + startup scheme self-check

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/CamelSslTransportFactory.java` (new)
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsClientFactory.java` (modified)
- `bridges/jms/src/main/java/org/rustcamel/jms/NativeReflectionConfig.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/SslTransportRegistrationTest.java` (new)

**Steps:**
1. Create `CamelSslTransportFactory extends SslTransportFactory` (org.apache.activemq.transport.tcp) — in this task an empty subclass; the socket-factory seam lands in Task 5.1.
2. In `JmsClientFactory.initNativeImageWorkarounds` (JmsClientFactory.java:467 area), register eagerly next to the existing `tcp` registration: `TransportFactory.registerTransportFactory("ssl", new CamelSslTransportFactory());`, and add `"META-INF/services/org/apache/activemq/transport/ssl"` to the `knownServices` list (JmsClientFactory.java:475-486) so native resolution needs no reflective FactoryFinder fallback.
3. Add `org.rustcamel.jms.CamelSslTransportFactory` (and keep the existing entries) to `NativeReflectionConfig.java` as defence in depth, following the existing `TcpTransportFactory` registration shape (reflect-config pattern at NativeReflectionConfig.java:23-26).
4. Extract the registration into a package-visible static method `static void registerTransportFactories()` (idempotent: guard with a boolean) called from `initNativeImageWorkarounds`, so tests can invoke it without the full native workaround init.
5. Add a startup self-check `static void verifySchemesResolvable(String brokerUrl)` in `JmsClientFactory`: parse the URL via the same member logic as Rust (`failover://`, `failover:(…)`, plain), resolve each member scheme with `TransportFactory.findTransportFactory(new URI(member))` inside a try/catch for `IOException`; wrap any failure in `IllegalStateException("UNSUPPORTED_TRANSPORT: no transport factory for scheme <scheme> in <redacted url>")`. Call it from the broker-connection setup path before any connect attempt (same place the broker URL is first consumed). Redact the URL with the existing redaction helper before including it in the message; never echo remote text.

**Tests:** (`SslTransportRegistrationTest`)
- `sslSchemeResolvesAfterRegistration`: setup = call `JmsClientFactory.registerTransportFactories()` → act = `TransportFactory.findTransportFactory(new URI("ssl://localhost:61617"))` → assert returns a `CamelSslTransportFactory` instance (no IOException).
- `knownServicesContainsSsl`: reflect on the `knownServices` list (or expose a package-visible accessor) → assert contains `META-INF/services/org/apache/activemq/transport/ssl`.
- `verifySchemesRejectsUnknown`: setup = registered factories → act = `verifySchemesResolvable("qls://h:1")` → assert IllegalStateException with message containing `UNSUPPORTED_TRANSPORT` and `qls`, and NOT containing any host credential text.
- `verifySchemesAcceptsComposite`: `verifySchemesResolvable("failover:(ssl://a:1,ssl://b:2)?maxReconnectAttempts=3")` → does not throw.
- `registrationIsIdempotent`: call `registerTransportFactories()` twice → no duplicate-registration exception.

**Acceptance:**
- `APP_HOME=bridges/jms bridges/jms/gradlew -p bridges/jms test --tests "*SslTransportRegistrationTest*"` passes (run from the repository root; the project is the standalone Gradle project at `bridges/jms/` with its own `bridges/jms/settings.gradle.kts` and `bridges/jms/gradlew`).
- `grep -c "org.rustcamel.jms.CamelSslTransportFactory" bridges/jms/src/main/java/org/rustcamel/jms/NativeReflectionConfig.java` ≥ 1 (exact bridge-owned class registered).
- `git diff` on JmsClientFactory shows no change to the existing `tcp` registration or option handling.

- [x] 4.1

## bridges/jms: TLS factory seam

### Task 5.1: PEM key/trust managers + identity-binding socket factory + composite downgrade policy

**Files:**
- `bridges/jms/src/test/resources/tls-fixtures/test-ca.pem` (new)
- `bridges/jms/src/test/resources/tls-fixtures/server-dns-san.pem` + `server-dns-san.key` (new)
- `bridges/jms/src/test/resources/tls-fixtures/server-ip-san.pem` + `server-ip-san.key` (new)
- `bridges/jms/src/test/resources/tls-fixtures/server-other-name.pem` + `server-other-name.key` (new)
- `bridges/jms/src/test/resources/tls-fixtures/client-cert.pem` + `client-key-pkcs8.pem` (new)
- `bridges/jms/src/test/resources/tls-fixtures/client-key-encrypted.pem` (new)
- `bridges/jms/src/main/java/org/rustcamel/jms/BrokerTlsMaterial.java` (new)
- `bridges/jms/src/main/java/org/rustcamel/jms/BrokerTlsIdentitySocketFactory.java` (new)
- `bridges/jms/src/main/java/org/rustcamel/jms/ServerNamePreservingSSLSocket.java` (new)
- `bridges/jms/src/main/java/org/rustcamel/jms/BridgeActiveMqConnectionFactory.java` (new)
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsClientFactory.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/BrokerTlsMaterialTest.java` (new)
- `bridges/jms/src/test/java/org/rustcamel/jms/FailoverTlsPolicyTest.java` (new)

**Steps:**
0. Generate the shared TLS fixtures ONCE in this task (Task 7.1 reuses them — do not regenerate there): `test-ca` (self-signed CA), `server-dns-san` (leaf signed by test-ca, SAN DNS:mq.internal.example only), `server-ip-san` (SAN IP:127.0.0.1 only), `server-other-name` (SAN DNS:other.example only), `client-cert`/`client-key-pkcs8` (client leaf signed by test-ca, unencrypted PKCS#8 key), `client-key-encrypted` (PKCS#8 ENCRYPTED key for the negative test). Use `openssl req`/`openssl x509`/`openssl pkcs8` (commands documented in the BrokerTlsMaterial class javadoc) and COMMIT all fixtures. Keys are test-only; file headers carry no more than the standard PEM armor.
1. `BrokerTlsMaterial`: static builders, all constructed at RUN TIME (never static initializers — native image heap): `static TrustManager[] buildTrustManagers(String caPemPath)` — parse the PEM bundle with `CertificateFactory.getInstance("X.509")`, load into an in-memory `KeyStore`, `TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm()).init(ks)`; when `caPemPath == null` return the platform default (init(null)); `static KeyManager[] buildKeyManagers(String certPemPath, String keyPemPath)` — parse the PEM chain via `CertificateFactory`, the unencrypted PKCS#8 key via `PKCS8EncodedKeySpec` + `KeyFactory.getInstance("RSA")` (fall back to `EC` when RSA construction throws `InvalidKeySpecException`), assemble an in-memory KeyStore with a throwaway alias/password, `KeyManagerFactory` init. `static SecureRandom newSecureRandom()` — created per factory build. File reads use `Files.readAllBytes` and never log contents.
2. `ServerNamePreservingSSLSocket extends SSLSocket`: delegating wrapper over a platform `SSLSocket` (constructor takes the delegate plus `serverName`; EVERY method forwards to the delegate — the wrapper has no functional `super` behaviour, all state lives in the wrapped socket). Override `setSSLParameters(SSLParameters p)`: call `delegate.setSSLParameters(p)`, then build a corrected copy with `setServerNames(List.of(new SNIHostName(serverName)))` and `setEndpointIdentificationAlgorithm("HTTPS")` and call `delegate.setSSLParameters(corrected)` — so ActiveMQ's `initialiseSocket` overwrite of serverNames (it sets them from the URL host) is corrected on the actual TLS implementation before the handshake. Also in the constructor, immediately apply the corrected parameters to the delegate. The TlsTransportMatrixTest (Task 7.1) proves the corrected SNI reaches the real handshake.
3. `BrokerTlsIdentitySocketFactory extends SSLSocketFactory`: wraps a delegate `SSLSocketFactory` (from the bridge `SSLContext`, see step 5) plus `serverName` (nullable). `createSocket()` (no-arg) throws `java.net.SocketException("bridge TLS socket factory requires the connected-socket path")` — this forces ActiveMQ `TcpTransport` down its connected-socket branch (TcpTransport catches SocketException from the no-arg call and proceeds to connect explicitly). `createSocket(InetAddress host, int port)`: when `serverName != null`, relabel via `InetAddress.getByAddress(serverName, host.getAddress())` (public API; dials the same resolved IP, no DNS lookup), create the platform socket unconnected, wrap in `ServerNamePreservingSSLSocket`, `connect(new InetSocketAddress(relabelled, port), connectTimeout)`; when `serverName == null` delegate with the original host. `createSocket(String host, int port)`: resolve, then the same path (used by non-ActiveMQ callers/tests). Remaining `createSocket` overloads delegate to the platform factory with wrapping when `serverName != null`. Default cipher/protocol lists are never restricted.
4. `BridgeActiveMqConnectionFactory extends ActiveMQSslConnectionFactory` — the SINGLE bridge-owned factory class used by BOTH selection branches (so the downgrade guard applies with or without a `tls` table). Constructor `(String url, KeyManager[] km, TrustManager[] tm, String serverName)`: when `tm` is non-null, `setKeyAndTrustManagers(km, tm, new SecureRandom())` (TLS env branch); when null, skip (platform default trust, no client identity — the platform-trust branch). Extract the downgrade policy into a package-visible static helper `static void applyCompositeTlsPolicy(Transport transport, boolean urlHasSslMember)` in this class: narrows `transport.narrow(FailoverTransport.class)`; when non-null AND `urlHasSslMember`: `ft.setUpdateURIsSupported(false); ft.setReconnectSupported(false);` (leave `updateURIsURL` unset; plain `ssl://` narrows to null, skip). The override `protected ActiveMQConnection createActiveMQConnection(Transport transport, management.JMSStatsImpl stats)` (exact 5.19.10 signature) calls the helper with the configured URL's ssl-membership BEFORE calling super. The override `public Connection createConnection(String userName, String password)` (and the no-arg overload delegating to it) establishes the TLS identity state BEFORE super runs — `super.createConnection` builds the transport (`createTransport()` consults `CamelSslTransportFactory.createSocketFactory()` DURING construction, before `createActiveMQConnection` ever runs), so: set the bridge `TlsConnectionState` — a static `InheritableThreadLocal` carrying (serverName, tlsActive) — before calling super and clear it in `finally`. `InheritableThreadLocal` (not plain ThreadLocal) because failover member creation for members ≥2 happens on the failover task-runner thread spawned during `transport.start()` under the still-set parent state (child threads inherit at creation); this mirrors the lifetime envelope of ActiveMQ's own `broker.SslContext` thread-local, and broker-directed member replacement is already disabled by the guard, bounding the async surface. The helper is the SINGLE production guard site and the shared seam tests use. The hook runs inside `createConnection` before `transport.start()`, so before any I/O or ConnectionControl.
5. `JmsClientFactory.buildActiveMqFactory(url, user, pass)` seam selection by URL transport security: if the URL (composite members included) has an `ssl` member: read env `BRIDGE_BROKER_TLS_CA_CERT_PATH`, `BRIDGE_BROKER_TLS_CLIENT_CERT_PATH`, `BRIDGE_BROKER_TLS_CLIENT_KEY_PATH`, `BRIDGE_BROKER_TLS_SERVER_NAME` (constants local to the class). With TLS env present: `BridgeActiveMqConnectionFactory` with KM/TM from PEM (its `SslContext` thread-local feeds `SslTransportFactory`). Without TLS env: `BridgeActiveMqConnectionFactory` with null KM/TM (platform default trust managers, no client identity), logged at `info!` — the SAME class, so the composite downgrade guard covers platform-trust composites too. Both branches keep the existing ID-prefix and `setSendAcksAsync(false)` behaviour identical. The identity socket factory is installed through the TRANSPORT factory seam: `CamelSslTransportFactory` (Task 4.1 singleton) overrides the socket-factory creation method (verify the exact overridable signature in the 5.19.10 sources — the class constant pool carries a `()Ljavax/net/SocketFactory;` creator, i.e. `createSocketFactory()`) to return `new BrokerTlsIdentitySocketFactory(delegate, serverName)` where `delegate` is the socket factory of the current `org.apache.activemq.broker.SslContext` when present (the thread-local `setKeyAndTrustManagers` installs), else `SSLContext.getDefault().getSocketFactory()`, and `serverName` comes from the `TlsConnectionState` `InheritableThreadLocal` established by `BridgeActiveMqConnectionFactory.createConnection` before transport construction (run-time only, native-safe).
6. `verifySchemesResolvable` from Task 4.1 is called before building the factory (already wired).

**Tests:**
- `BrokerTlsMaterialTest`:
  - `trustManagersFromPemBundle`: setup = PEM CA bundle under `src/test/resources/tls-fixtures/test-ca.pem` → build → assert `TrustManager[0]` is an `X509TrustManager` whose accepted issuers are non-empty.
  - `keyManagersFromPkcs8Pair`: fixtures `client-cert.pem` + `client-key-pkcs8.pem` → assert one `X509KeyManager` with a non-null private key.
  - `encryptedKeyRejected`: a PKCS#8 ENCRYPTED key fixture → assert throws with a message naming the PKCS#8 requirement (no content in message).
  - `factorySelectionByTransportSecurity`: package-visible helper of `buildActiveMqFactory` with `ssl://` member + no TLS env → returns `BridgeActiveMqConnectionFactory` in platform-trust mode (null KM/TM, logged info); with TLS env set (test-scoped env or injected parameter) → returns `BridgeActiveMqConnectionFactory` in pinned-material mode; with `tcp://` URL → the plain factory exactly as today (regression).
  - `platformTrustCompositeStillGuarded`: `failover:(ssl://a:1,ssl://b:2)` with NO TLS env (platform-trust mode) → the failover flags are still both disabled (the guard is class-wide, not TLS-env-conditional).
- `FailoverTlsPolicyTest` (pure JVM, real ActiveMQ transport objects, no broker):
  - `compositeSslDisablesUriUpdatesAndReconnects`: setup = `FailoverTransport` created from `failover:(ssl://a:1,ssl://b:2)` via `TransportFactory.findTransportFactory(...).compositeConnect(...)` inside a `BridgeActiveMqConnectionFactory.createActiveMQConnection`-driven path (or narrow directly on the returned transport when construction without a broker is not possible) → assert `isUpdateURIsSupported()==false && isReconnectSupported()==false`.
  - `plainSslDoesNotTouchFailoverFlags`: transport from `ssl://a:1` → narrow(FailoverTransport.class) is null → no exception, flags untouched.
  - `tcpCompositeUntouched`: `failover:(tcp://a:1)` without ssl member → flags remain default true.

**Acceptance:**
- `APP_HOME=bridges/jms bridges/jms/gradlew -p bridges/jms test --tests "*BrokerTlsMaterialTest*" --tests "*FailoverTlsPolicyTest*"` passes.
- `grep -rn "javax.net.ssl.system" bridges/jms/src/main` shows no `System.setProperty("javax.net.ssl...")` calls (grep for `javax.net.ssl.keyStore|trustStore` setProperty) — no global JSSE properties.
- No static initializer in any new class touches key/trust material (code review assertion; `grep -n "static {"` on the new files shows none, or only trivial non-crypto ones).
- `cargo xtask lint-unwrap` unaffected (Java side, but keep null-safety: no `orElse(null)` chains that can NPE in the transport walk).

- [x] 5.1

## bridges/jms: failure classification and health wiring

### Task 6.1: BrokerFailureClassifier + HealthResponse capabilities/failure_class population

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/BrokerFailureClass.java` (new)
- `bridges/jms/src/main/java/org/rustcamel/jms/BrokerFailureClassifier.java` (new)
- `bridges/jms/src/main/proto/jms_bridge.proto` (already modified in 3.1 — no further edit)
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsBrokerProbe.java` (modified)
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsBridgeService.java` (modified, health RPC response build)
- `bridges/jms/src/test/java/org/rustcamel/jms/BrokerFailureClassifierTest.java` (new)

**Steps:**
1. `BrokerFailureClass` enum with the ten values (names matching the proto: `NETWORK, TLS_TRUST, TLS_HOSTNAME, TLS_CLIENT_AUTH, TLS_PROTOCOL, TLS_PLAINTEXT_PEER, UNSUPPORTED_TRANSPORT, AUTH, CLIENT_ID_IN_USE, OTHER`) and a `toProto()` mapping onto the generated `BrokerFailureClass` proto enum.
2. `BrokerFailureClassifier.classify(Throwable t)` — cause-chain walk, type-first, deterministic, covering ALL ten enum values: `java.net.ConnectException` / `SocketTimeoutException` → NETWORK; `CertPathValidatorException` OR `CertPathBuilderException` in the chain (PKIX path building throws the builder subtype — `SunCertPathBuilderException` extends `CertPathBuilderException`, NOT the validator type) → TLS_TRUST; `SSLHandshakeException` whose message carries a client-auth alert (`bad_certificate`, `certificate_required` — JDK alert strings) → TLS_CLIENT_AUTH; `SSLHandshakeException` with any other alert (`handshake_failure`, `protocol_version`) → TLS_PROTOCOL; `CertificateException` with no CertPathValidatorException cause whose message matches the JDK hostname text family (`No name matching`, `No subject alternative names matching IP address`, `No subject alternative DNS name matching`, `No subject alternative names present`) → TLS_HOSTNAME; other `CertificateException`/`SSLException` with `Unsupported or unrecognized SSL message` → TLS_PLAINTEXT_PEER; other `SSLException`/`SSLHandshakeException` → TLS_PROTOCOL; `JMSSecurityException` → AUTH; `IllegalStateException` (or a wrapped cause) whose message starts with `UNSUPPORTED_TRANSPORT:` (the bridge's own startup self-check marker from Task 4.1) → UNSUPPORTED_TRANSPORT; `javax.jms.IllegalStateException` → CLIENT_ID_IN_USE BY TYPE ONLY (no message matching — the blessed rule limits message matching to JDK-authored TLS texts; the JMS client-ID restriction is a type-level contract for M2), and this classification must not depend on exception text; default OTHER. Remote text is never returned — the classifier returns ONLY the enum; Java logs the full exception locally at `warn!` with the class at the call site.
3. Health wiring: wherever `HealthResponse` is built (health RPC path in `JmsBridgeService`), add `capabilities = ["broker-tls-v1"]` (constant `CAP_BROKER_TLS_V1`) and `failure_class` from the classifier applied to the last broker/probe failure (keep a volatile `lastBrokerFailure` Throwable field set by the probe's connect path; UNSPECIFIED when none). The probe (`JmsBrokerProbe`) wraps its factory-building/connect call in try/catch → classify → store class + log `warn!` locally (never key material, remote text stays in the local log only).
4. Do not change existing health fields' semantics (`healthy`, `broker_connected`, `message`, `status`, `age_ms` keep their meaning; `message` stays as-is — Rust rendering of TLS_HOSTNAME comes from Task 8.1 and does not depend on Java message text).

**Tests:** (`BrokerFailureClassifierTest`, constructed exception chains, no broker)
- `pkixCauseClassifiesTlsTrust`: BOTH path-exception roots classify TLS_TRUST — `CertPathValidatorException("pkix")` and `CertPathBuilderException("pkix")` (each wrapped as the cause of an SSLHandshakeException) → TLS_TRUST each.
- `hostnameTextsClassifyTlsHostname`: parametrized over the four JDK texts (`No name matching mq.internal found`, `No subject alternative names matching IP address 10.0.0.5 found`, `No subject alternative DNS name matching mq.internal found.`, `No subject alternative names present`) wrapped as CertificateException under SSLHandshakeException → TLS_HOSTNAME each.
- `connectExceptionClassifiesNetwork`: ConnectException → NETWORK.
- `plaintextPortClassifiesPlaintextPeer`: SSLException("Unsupported or unrecognized SSL message") → TLS_PLAINTEXT_PEER.
- `alertMessageClassifiesTlsProtocol`: SSLHandshakeException("Received fatal alert: handshake_failure") → TLS_PROTOCOL.
- `clientAuthAlertsClassifyTlsClientAuth`: SSLHandshakeException("Received fatal alert: bad_certificate") and `("Received fatal alert: certificate_required")` → TLS_CLIENT_AUTH each.
- `unsupportedTransportMarkerClassifiesUnsupported`: IllegalStateException("UNSUPPORTED_TRANSPORT: no transport factory for scheme qls") → UNSUPPORTED_TRANSPORT; the same wrapped as the cause of a RuntimeException → still UNSUPPORTED_TRANSPORT.
- `securityExceptionClassifiesAuth`: JMSSecurityException → AUTH.
- `unknownClassifiesOther`: RuntimeException("broker said whatever") → OTHER.
- `clientExceptionByTypeNotText`: javax.jms.IllegalStateException with arbitrary broker-specific message → CLIENT_ID_IN_USE; a generic RuntimeException carrying the SAME message text → OTHER (proving text cannot select the class).
- `classifierReturnsEnumOnly`: for every case, the returned object is the enum (no message text carried).
- `probeReportsCapabilityAndClass`: unit-test the health response builder (mock or real builder): response capabilities contain `broker-tls-v1`; after injecting a stored TLS_TRUST failure, `failure_class` maps to proto `TLS_TRUST`; with no failure, UNSPECIFIED.

**Acceptance:**
- `APP_HOME=bridges/jms bridges/jms/gradlew -p bridges/jms test --tests "*BrokerFailureClassifierTest*"` passes.
- Message inspection in the classifier is CONFINED to the permitted text families: every `getMessage()` use in `BrokerFailureClassifier.java` compares against named constants only (`JDK_HOSTNAME_TEXTS`, `CLIENT_AUTH_ALERTS`, `PROTOCOL_ALERTS`, `PLAINTEXT_PEER_MESSAGE`, `UNSUPPORTED_TRANSPORT_MARKER`) — verified by review of the classifier diff; no free-form message matching exists.
- Health response construction compiles against the proto from Task 3.1 (both copies still byte-identical).

- [x] 6.1

## bridges/jms: JVM TLS matrix (hostname/trust through the real transport path)

### Task 7.1: In-process TLS matrix tests

**Files:**
- `bridges/jms/src/test/resources/tls-fixtures/test-ca.pem` (new)
- `bridges/jms/src/test/resources/tls-fixtures/server-dns-san.pem` + `server-dns-san.key` (new)
- `bridges/jms/src/test/resources/tls-fixtures/server-ip-san.pem` + `server-ip-san.key` (new)
- `bridges/jms/src/test/resources/tls-fixtures/server-other-name.pem` + `server-other-name.key` (new)
- `bridges/jms/src/test/resources/tls-fixtures/client-cert.pem` + `client-key-pkcs8.pem` (new)
- `bridges/jms/src/test/java/org/rustcamel/jms/TlsTransportMatrixTest.java` (new)

**Steps:**
1. Fixtures: reuse the fixtures committed in Task 5.1 step 0 — do NOT regenerate. `TlsTransportMatrixTest` harness: an in-process `SSLServerSocket` on an ephemeral loopback port, `needClientAuth` toggleable, SSLContext built from the chosen server fixture (key) + test-ca trust; the CLIENT side is the real bridge path: `JmsClientFactory.registerTransportFactories()` + `TransportFactory.findTransportFactory(new URI("ssl://127.0.0.1:<port>")).connect(...)` with the bridge `BrokerTlsIdentitySocketFactory` installed through the same seam Task 5.1 uses, TLS env set via system properties the factory reads in tests only if needed (prefer direct construction over env mutation).
2. Matrix cases (each asserts the handshake outcome AND, on failure, `BrokerFailureClassifier.classify` of the caught exception):
   - `dnsSanByIpWithoutServerNameFails`: server=dns-san fixture, dial 127.0.0.1, no server_name → handshake fails, class TLS_HOSTNAME.
   - `dnsSanByIpWithServerNamePasses`: same server, server_name=mq.internal.example → handshake succeeds (socket connected, server sees a client); the test ALSO registers a server-side `HandshakeCompletedListener`, casts the event's session to `ExtendedSSLSession`, and asserts `getRequestedServerNames()` contains exactly `mq.internal.example` — proving the delegate-forwarded SNI correction reached the real handshake (capture happens in the listener, where the completed session is available; `ExtendedSSLSession` is the type carrying `getRequestedServerNames()`).
   - `wrongServerNameFailsEvenWhenDialedHostWouldMatch`: server=ip-san fixture (SAN matches the dialed IP), server_name=other.example → fails, TLS_HOSTNAME. This is the pass-3 critical negative: peer-host relabeling + SNI preservation must make it fail through the REAL transport path.
   - `ipSanByIpPasses`: server=ip-san fixture, no server_name → succeeds.
   - `wrongCaFailsClosed`: client trusts a DIFFERENT CA bundle than the one that signed the server cert → fails, TLS_TRUST.
   - `platformTrustFailsPrivateCa`: no TLS env at all (no `tls` table) dialing the test-ca-signed server → handshake fails, TLS_TRUST — proving absence of a pinned CA means platform trust, which does not know the private CA (hermetic negative for the "Absent tls table means platform trust" scenario; the platform-trust POSITIVE against a public CA cannot be simulated hermetically and is stated as a CI residual in the module docs).
   - `needClientAuthWithoutClientCertFails`: server needClientAuth=true, client trust-only → fails; the client-side class is TLS_CLIENT_AUTH, asserted deterministically via the classifier's client-auth alert mapping (`bad_certificate`/`certificate_required`); the test also asserts the server socket observed a handshake failure.
   - `needClientAuthWithClientCertPasses`: server needClientAuth=true, client installs KM from client fixtures → succeeds.
   - `plainServerOnTlsClientFailsPlaintextPeer`: plain (non-SSL) ServerSocket, client dials ssl:// → fails, TLS_PLAINTEXT_PEER.
   - `ordinaryFailoverReachesSecondMember`: the BTS "ordinary failover between configured members SHALL remain" clause, runnable at the TRANSPORT level (an SSLServerSocket provides TLS but no OpenWire, so JMS `createConnection` is NOT asserted here — the full JMS-level failover round-trip is the CI smoke row): reserve a port by bind-then-close (never a fixed low port), start the live in-process TLS server on `<livePort>`, build `failover:(ssl://127.0.0.1:<deadPort>,ssl://127.0.0.1:<livePort>)?randomize=false` via `TransportFactory.findTransportFactory(...).compositeConnect(...)`, apply the PRODUCTION guard through the shared seam `BridgeActiveMqConnectionFactory.applyCompositeTlsPolicy(transport, true)` (the same helper the factory override calls — not a test-local reimplementation), register a `TransportListener` whose `transportResumed()` completes a `CompletableFuture`, then `transport.start()` → the dead member refuses, failover cycles to the live TLS member → assert `future.get(10, TimeUnit.SECONDS)` returns (bounded wait — no `Thread.sleep`) AND the downgrade-guard flags are still both false — proving both-switch disabling does not break member failover.
   - `failoverSecondMemberReceivesServerName`: same two-member harness, but with `TlsConnectionState` set (server_name=mq.internal.example, mirroring what `createConnection` establishes) and the live member's server capturing `ExtendedSSLSession.getRequestedServerNames()` via its `HandshakeCompletedListener` → after the bounded resume, the SERVER-side requested SNI on the SECOND member is exactly `mq.internal.example` — proving the InheritableThreadLocal state survives asynchronous failover member creation.
   - `serverNameThroughProductionFactoryPath`: the production path end-to-end at JVM level: construct `BridgeActiveMqConnectionFactory(url=ssl://127.0.0.1:<livePort>, km=null, tm=BrokerTlsMaterial.buildTrustManagers(<test-ca.pem>), serverName="mq.internal.example")` — REAL trust material (the test-CA-signed server would otherwise be rejected by platform trust before identity matters; the default-trust rejection stays covered by `platformTrustFailsPrivateCa`) — call `createConnection()` through the production override (no OpenWire needed for the socket-establishment segment; if full `createConnection` blocks on wire-format negotiation, assert instead up to the server-side captured SNI via a server that completes the TLS handshake and then closes, accepting the resulting client exception) → the server-side captured requested SNI is exactly `mq.internal.example`, proving state is established before transport construction through the real factory entry point.
3. Each test tears down the server socket in a finally block; ports are ephemeral (no fixed ports, no sleeps — `lint-test-sleep`/`lint-unbounded-wait` clean).

**Tests:** the eight matrix cases above ARE the tests.

**Acceptance:**
- `APP_HOME=bridges/jms bridges/jms/gradlew -p bridges/jms test --tests "*TlsTransportMatrixTest*"` passes with all 12 green.
- No `Thread.sleep` in the test file (grep = 0 hits).
- The fixtures directory contains no keystore binaries — PEM only.

- [x] 7.1

## camel-jms: capability gate and diagnostics rendering

### Task 8.1: Rust broker-tls-v1 gate + failure-class local rendering

**Files:**
- `crates/components/camel-jms/src/component.rs` (modified)
- `crates/components/camel-jms/src/consumer.rs` (modified)
- `crates/components/camel-jms/src/diagnostics.rs` (new)
- `crates/components/camel-jms/src/lib.rs` (modified — `pub mod diagnostics;` if the crate exposes it, else `mod diagnostics;` matching module privacy conventions)
- `crates/components/camel-jms/src/diagnostics_test.rs` or `mod tests` in diagnostics.rs (new)

**Steps:**
1. In the startup Health handling (component.rs phase-2 health call, around component.rs:1378): after a successful Health response, if the broker config carries `tls.is_some()` and `!response.capabilities.iter().any(|c| c == "broker-tls-v1")` → kill the bridge process (the existing stop/kill helper used elsewhere in component.rs) and return a permanent (config-class) error: `bridge {version_or_path} lacks broker-tls-v1; use the pinned bridge` (do not retry; map to the existing permanent-error channel). When `tls.is_none()`, a missing capability is NOT fatal.
2. New `diagnostics.rs`: `pub fn render_failure_message(failure_class: i32, broker_name: &str, expected_identity: &str, connected_host: &str) -> String` mapping the proto `BrokerFailureClass` values to local actionable text. Callers compute `expected_identity` as the broker's `server_name` when configured, else the URL host. `TLS_HOSTNAME` renders exactly the D1 contract: `broker '<name>': TLS certificate does not match '<expected_identity>' (connected to <redacted url host>). Use a broker address the certificate names (DNS or IP SAN), or set brokers.<name>.tls.server_name to a DNS name in the certificate.` — the connected host passes through `redact_broker_url`. Other classes get one-line local remedies (TLS_TRUST → check ca_cert_path against the broker's CA; TLS_CLIENT_AUTH → broker requires a client certificate, configure the pair; TLS_PROTOCOL → protocol/cipher mismatch, JDK defaults in use; TLS_PLAINTEXT_PEER → the port is not TLS; UNSUPPORTED_TRANSPORT → scheme has no transport, check the URL; NETWORK → broker unreachable; AUTH → check username/password; CLIENT_ID_IN_USE → another connection holds the client id; OTHER → see bridge logs). No remote text is ever included — the function takes only enum + config strings.
3. Delivery of the failure class to the consumer: the existing health monitor path (component.rs:1043-1059) currently discards the failure class — extend it to STORE the last observed `(failure_class, expected_identity, connected_host)` triple from the Health response (new `LastBrokerFailure` state on the component's broker handle, updated on every Health poll; the `expected_identity` is the broker's `server_name` when configured else the URL host, computed once at validation time and stored alongside). The consumer error path (consumer.rs:169-180 area) reads that stored triple through the existing consumer↔component channel (the same handle the consumer already holds for broker identity) and, when a class other than UNSPECIFIED is present for a connection failure, renders via `render_failure_message` instead of any remote message; the existing policy of never echoing remote text is preserved (this task ADDS the local rendering; it does not change retry classification).
4. Integration-style wiring test: a TLS_HOSTNAME diagnostic test exercising the full path — component health handling receives a Health response with `failure_class = TLS_HOSTNAME` → stored triple → consumer renders the D1 message (assert rendered text contains expected identity, redacted host, both remediations, no remote text).

**Tests:**
- `gate_kills_bridge_when_capability_missing`: tls configured + Health with empty capabilities → the gate function returns the permanent error mentioning `broker-tls-v1` (unit-test the extracted gate fn `ensure_tls_capability(capabilities: &[String], tls_configured: bool) -> Result<(), BridgeError-ish>`; follow the crate's error type).
- `gate_passes_when_capability_present`: tls configured + `["broker-tls-v1"]` → Ok.
- `gate_skipped_without_tls`: no tls + empty capabilities → Ok.
- `tls_hostname_message_names_both_remedies`: render(TLS_HOSTNAME, "main", "mq.internal", "ssl://10.0.0.5:61617") → message contains `mq.internal`, `10.0.0.5`, both remediation phrases, and `brokers.main.tls.server_name`.
- `tls_trust_message_is_local_only`: render(TLS_TRUST, …) → contains ca_cert_path remedy, contains no `%` placeholders or remote text.
- `redacted_host_in_message`: connected host with `password=x` query → rendered message contains the redacted form only.
- `tls_hostname_diagnostic_wiring_end_to_end`: component health handling fed a Health response carrying `failure_class = TLS_HOSTNAME` → consumer renders the D1 message from the stored triple (asserts the delivery path of step 3, not just the pure function).
- `startup_gate_kills_bridge_and_fails_permanently`: an integration-style unit test following the existing component.rs process-lifecycle test pattern (spawn helper available in that module): tls-configured broker + a stubbed Health response with empty capabilities → the startup path KILLS the spawned bridge process (assert the process handle is terminated / the kill helper was invoked via the same seam the component tests observe) AND returns the permanent error mentioning `broker-tls-v1` AND no retry is scheduled (assert the permanent-error channel, e.g. the route fails without the backoff path being taken).

**Acceptance:**
- `cargo test -p camel-component-jms --lib diagnostics` and `cargo test -p camel-component-jms --lib component` pass.
- `cargo clippy -p camel-component-jms --all-targets -- -D warnings` exits 0; `cargo fmt --check` exits 0.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-jms --no-deps` exits 0 (doc gate applies to the new public fn).

- [x] 8.1

## docs + CI smoke entry point

### Task 9.1: Docs (TLS section, L1/L8 notes) + ignored native smoke module

**Files:**
- `docs/src/components/jms.md` (modified)
- `bridges/jms/README.md` (modified)
- `crates/camel-test/tests/jms_tls_test.rs` (new)
- `crates/camel-test/tests/support/tls_fixtures.rs` (new, only if the smoke harness needs shared fixture helpers; otherwise omit)

**Steps:**
1. `docs/src/components/jms.md`: add a "Broker TLS" section: the `tls` table with a TOML example (pinned CA, mTLS, `server_name`), the D2 matrix as a table, verification-always-on statement (no skip knob), by-IP guidance (IP SAN in the certificate, or `server_name` + DNS SAN), `server_name` DNS-only rule, the `TLS_HOSTNAME` message sample, failover composite rules (uniform security, mixed rejected, outer options restricted, broker-directed replacement disabled), and an L8 migration note: switching a broker `tcp://`→`ssl://` changes the journal broker key, so recorded poison counts reset.
2. `bridges/jms/README.md`: fix the L1 claim (line ~66): Classic + `ssl://` now configures TLS through the `tls` table (pinned CA / platform trust / mTLS); startup aborts only when configured material is unusable (placeholder/missing/unreadable) or the scheme has no transport. Keep the legacy Artemis keystore env contract section unchanged, marked Artemis-only.
3. `crates/camel-test/tests/jms_tls_test.rs`: a RUNNABLE (but `#[ignore]`d by default) native smoke harness — CI executes it with `cargo test -p camel-test --features integration-tests --test jms_tls_test -- --ignored`; this repo change leaves it green-when-run, and the module docs name CI as its owner. Module-level doc comment enumerates the FULL CI-owned native matrix (private CA OK; wrong CA → TLS_TRUST; DNS-only SAN by IP → TLS_HOSTNAME; +server_name OK; IP SAN by IP OK; needClientAuth without/with cert; failover:(ssl://…) OK; old-capability binary + tls → permanent refusal; tcp:// regression unchanged). The implemented smoke harness covers two of those rows end-to-end against a real containerized Classic broker: (a) `tls_roundtrip_positive` — generate an rcgen CA + server cert (pattern: `crates/camel-test/tests/support/redis.rs:85-98` rcgen usage), build a PKCS#12 broker keystore with `p12-keystore` (pattern: `crates/camel-test/tests/support/cxf.rs:227-250`), write an `activemq.xml` with an `sslContext` + `ssl://` connector, start the container (reuse the `support/activemq.rs` dedicated-broker harness), spawn the bridge via `support/jms_bridge_procs.rs` with a `tls` table pinning the rcgen CA, assert produce→consume round-trip AND health `UP` AND health capabilities contain `broker-tls-v1`; (b) `tls_wrong_ca_fails_closed` — same fixture but the Rust `ca_cert_path` pins a DIFFERENT rcgen CA → the bridge/probe surfaces the TLS_TRUST failure class (assert via health `failure_class` or the rendered diagnostic) and no message round-trip occurs. Gate the file behind the same feature the existing jms_test.rs uses (`integration-tests`), matching its crate-level conventions.
4. Cross-check `docs/src/components/jms.md` for statements contradicting the new behaviour (search "ssl" and "failover") and update only those lines.

**Tests:**
- `jms_tls_smoke_compiles`: `cargo check -p camel-test --features integration-tests` exits 0 with the new file present.
- `ignored_tests_listed`: `cargo test -p camel-test --features integration-tests --test jms_tls_test -- --list` lists both tests; both are `#[ignore]`d by default (CI runs them with `--ignored`).
- `mdbook_test_docs`: `cd docs && mdbook test` passes for the new section (doc gate).

**Acceptance:**
- `cargo xtask lint-ignore` exits 0 (new test file follows .gitignore rules; nothing ignored that must not be).
- `cd docs && mdbook test` exits 0.
- `grep -c "abort" bridges/jms/README.md` reflects the corrected Classic claim (no contradiction with the new behaviour remains).
- No `versions.toml` change and no `BRIDGE_VERSION` value change anywhere in the diff (`grep BRIDGE_VERSION crates/components/camel-jms/src/lib.rs` still shows `"0.9.0"`; lib.rs MAY gain the `diagnostics` module declaration from Task 8.1 — only the version constant must be untouched).

- [x] 9.1
