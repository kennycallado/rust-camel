## ADDED Requirements

### Requirement: JMS broker TLS table validates fail-closed at startup

The JMS component SHALL accept an optional provider-neutral `tls` table on
each broker entry with keys `ca_cert_path`, `client_cert_path`,
`client_key_path`, and `server_name`, rejecting unknown keys. Validation at
startup SHALL enforce: a `tls` table requires every URL member (composite
members included) to use `ssl://`; `client_cert_path` and `client_key_path`
are required together; `server_name` is a non-empty syntactically valid DNS
hostname (never an IP literal) allowed only on TLS URLs; configured paths
exist, are readable, and contain no `placeholder-` marker; a `tls` table on an
`artemis` broker is rejected; `ws://`/`wss://` URLs are rejected for
`activemq` brokers while `nio://`/`nio+ssl://` remain rejected; a broker URL
carrying `jms.clientID` is rejected; on any `ssl` URL member member query
options outside the `jms.*` namespace are rejected; and on composite URLs
outer options are restricted to `jms.*` plus `maxReconnectAttempts` and
`startupMaxReconnectAttempts`, with the `nested.` prefix rejected. Broker
URLs in errors
SHALL pass through redaction, and no TLS key or file content SHALL appear in
Debug or error output. The bridge process SHALL receive the material only
through spawn environment variables `BRIDGE_BROKER_TLS_CA_CERT_PATH`,
`BRIDGE_BROKER_TLS_CLIENT_CERT_PATH`, `BRIDGE_BROKER_TLS_CLIENT_KEY_PATH`,
and `BRIDGE_BROKER_TLS_SERVER_NAME`, emitted per broker; because the child
inherits the parent environment, the spawn SHALL remove all four variables
before injecting the broker's values, so a stale operator export cannot reach
a broker without a `tls` table. The legacy Artemis keystore environment
contract SHALL remain untouched.

#### Scenario: Client cert without key is rejected

- **GIVEN** a `tls` table with `client_cert_path` but no `client_key_path`
- **WHEN** the Rust pool config validates
- **THEN** validation fails with a config-class error naming the incomplete
  client identity pair

#### Scenario: server_name must be a DNS hostname

- **GIVEN** a `tls` table whose `server_name` is an IP literal or empty
- **WHEN** the Rust pool config validates
- **THEN** validation fails naming the `server_name` rule

#### Scenario: tls table on a plaintext URL is rejected

- **GIVEN** `broker_url = "tcp://broker:61616"` and a `tls` table
- **WHEN** the Rust pool config validates
- **THEN** validation fails naming the URL scheme and the `tls` table

#### Scenario: Placeholder material path aborts

- **GIVEN** a `tls` table whose `ca_cert_path` value contains the
  `placeholder-` marker or names a missing file
- **WHEN** the Rust pool config validates
- **THEN** startup validation fails naming the unusable path

#### Scenario: Artemis broker with tls table is rejected

- **GIVEN** `broker_type = "artemis"` and a `tls` table
- **WHEN** the Rust pool config validates
- **THEN** validation fails naming the Artemis TLS follow-up

#### Scenario: ws scheme rejected for Classic

- **GIVEN** an `activemq` broker entry whose URL uses `ws://` or `wss://`
- **WHEN** the Rust pool config validates
- **THEN** validation fails naming the unsupported transport

#### Scenario: jms.clientID in broker URL is rejected

- **GIVEN** an `activemq` broker URL containing a `jms.clientID` query option
- **WHEN** the Rust pool config validates
- **THEN** validation fails naming client identity as endpoint-scoped

#### Scenario: TLS material reaches the bridge through env only

- **GIVEN** a valid `tls` table on a Classic broker
- **WHEN** the bridge process is spawned
- **THEN** the four `BRIDGE_BROKER_TLS_*` variables carry the configured
  paths and server name, and no TLS material appears in any log, proto
  message, or Debug output

#### Scenario: Stale TLS env cannot leak into a broker without a tls table

- **GIVEN** a parent environment that exports `BRIDGE_BROKER_TLS_CA_CERT_PATH`
  and a Classic broker entry with no `tls` table
- **WHEN** the bridge process is spawned
- **THEN** the child environment contains none of the four
  `BRIDGE_BROKER_TLS_*` variables

### Requirement: JMS bridge classifies broker failures without echoing remote text

The Java bridge SHALL classify broker connection and handshake failures into
the closed set `NETWORK`, `TLS_TRUST`, `TLS_HOSTNAME`, `TLS_CLIENT_AUTH`,
`TLS_PROTOCOL`, `TLS_PLAINTEXT_PEER`, `UNSUPPORTED_TRANSPORT`, `AUTH`,
`CLIENT_ID_IN_USE`, `OTHER` by inspecting the exception cause chain by type
first, falling back to message matching only for JDK-authored texts (alert
strings and the JDK's fixed hostname-mismatch text family: `No name matching
… found`, `No subject alternative names matching IP address … found`,
`No subject alternative DNS name matching … found`, `No subject alternative
names present`); remote broker text is
never matched or echoed. The classifier SHALL report the class in the health
response; Java SHALL log exception details locally without key material and
SHALL NOT place remote error text on the wire. Rust SHALL render a local,
actionable message per class and never echo
remote text; the `TLS_HOSTNAME` message SHALL name the expected identity
(`server_name` or URL host), the redacted connected host, and both
remediations (use an address the certificate names, or set
`brokers.<name>.tls.server_name`). At startup the bridge SHALL resolve every
scheme in the configured broker URL, composite members included, against the
registered transport factories, and SHALL abort with `UNSUPPORTED_TRANSPORT`
before any connect attempt when a scheme is unresolvable. The native-image
configuration SHALL register the bridge-owned `ssl` transport factory so
`scheme NOT recognized` cannot occur for `ssl`.

#### Scenario: Hostname mismatch renders both remediations

- **GIVEN** a handshake that fails because the certificate does not match the
  expected identity
- **WHEN** the failure reaches the Rust consumer diagnostics
- **THEN** the rendered message carries class `TLS_HOSTNAME`, the expected
  name, the redacted host, and both remediations, and no remote exception
  text

#### Scenario: Classification is by exception type

- **GIVEN** an exception chain rooted in a certification-path validation
  error
- **WHEN** the classifier runs
- **THEN** the class is `TLS_TRUST`, derived from the cause-chain types
  without depending on broker-specific message text

#### Scenario: Hostname failure is distinguished from other certificate failures

- **GIVEN** two exception chains, one rooted in a JDK hostname-mismatch
  `CertificateException` and one in a certification-path validation error
- **WHEN** the classifier runs on each
- **THEN** the first classifies `TLS_HOSTNAME` and the second `TLS_TRUST`,
  using type plus the JDK's fixed hostname texts only

#### Scenario: Unresolvable scheme aborts before connecting

- **GIVEN** a configured URL whose member scheme has no registered transport
  factory
- **WHEN** the bridge starts
- **THEN** startup aborts with class `UNSUPPORTED_TRANSPORT` naming the
  scheme, and no connect attempt is made

#### Scenario: Health carries class and capability

- **GIVEN** a bridge built from this change
- **WHEN** the health RPC responds
- **THEN** the response carries the `broker-tls-v1` capability and a
  `BrokerFailureClass` value, with additive field numbering over the existing
  health fields

## MODIFIED Requirements

### Requirement: JMS bridge applies ActiveMQ jms.* URI options in native images

The native JMS bridge SHALL apply ActiveMQ Classic broker-URL options in the
`jms.*` namespace, including `jms.connectionIDPrefix`. The native-image
configuration SHALL register `org.apache.activemq.ActiveMQConnectionFactory` at
class level with invocation-capable public methods (`allPublicMethods` or an
explicit invocable setter set), not a query-only registration.

`JmsClientFactory` SHALL honor an explicit `jms.connectionIDPrefix` as the
effective connection-ID prefix base and SHALL append a per-process token so
connection IDs stay unique across processes. When no explicit prefix is set it
SHALL apply a random per-process token alone. The native proof SHALL observe the
effective connection identity from a live broker connection, not echoed input
configuration. Existing cross-process unique-ID regressions SHALL remain. No
package-level entry and no transport option outside `jms.*` SHALL be added.
Bridge-owned registration of ActiveMQ transport factories (the `ssl` transport
factory and its service entry) SHALL be permitted as code-level registration
and native-image configuration; this does not relax the prohibition on URL
transport options outside `jms.*`.

#### Scenario: Invocation-capable class-level registration

- **GIVEN** the native-image `reflect-config.json`
- **WHEN** a reviewer inspects the ActiveMQ entry
- **THEN** `org.apache.activemq.ActiveMQConnectionFactory` SHALL be registered at class level
- **AND** its public methods SHALL be invocation-capable, not query-only
- **AND** no package-level ActiveMQ entry SHALL be added

#### Scenario: Explicit prefix survives with uniqueness

- **GIVEN** a broker URL carrying `jms.connectionIDPrefix=<sentinel>`
- **WHEN** `JmsClientFactory` builds the factory
- **THEN** the effective connection-ID prefix SHALL contain `<sentinel>`
- **AND** it SHALL carry a per-process unique token

#### Scenario: Live-broker connection observes the sentinel

- **GIVEN** a live ActiveMQ Classic broker and the 0.8.0 native bridge
- **WHEN** the proof runs two bridge processes
- **THEN** the health response SHALL report a connected broker
- **AND** each effective broker connection ID SHALL contain the sentinel
- **AND** the two connection IDs SHALL differ
- **AND** no option-application or reflection failure SHALL appear

#### Scenario: The native build still succeeds

- **GIVEN** the new reflection entry
- **WHEN** the native image builds
- **THEN** the build SHALL succeed and `cargo xtask lint-ignore` SHALL pass

#### Scenario: Bridge-owned ssl transport registration

- **GIVEN** the native bridge and a `ssl://` broker URL
- **WHEN** the transport factory for scheme `ssl` is resolved
- **THEN** the bridge-owned registered factory is used
- **AND** no URL transport option outside `jms.*` is applied
