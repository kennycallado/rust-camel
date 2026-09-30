## MODIFIED Requirements

### Requirement: Reject unsupported compile-time assets

The compiler SHALL embed the R2 asset matrix — certificates, private keys, and client-CA files in TLS and listener document contexts; TLS endpoint URI parameters (`tlsCert`/`tlsKey` on HTTP and WS endpoints; `serverCertPath`/`serverKeyPath`/`clientCaPath` on gRPC server endpoints and `caCertPath`/`clientCertPath`/`clientKeyPath` on gRPC client endpoints); the `protoFile` URI parameter on gRPC endpoints; `xslt` fields and `xslt:` URI operands; `xsd` fields and `validator:` URI operands; `sql:file:` URI operands; `static_dir` trees — and SHALL reject everything else fail-closed: `wasm:` URI operands, file-valued secret fields, dynamic `${env:}` placeholders and absolute paths inside asset fields and file-valued URI parameters, compile-time `CAMEL_*` configuration overrides — the signing input `CAMEL_COMPILE_SIGNING_KEY` under `--sign` excepted, whose stray presence without `--sign` still rejects — embedded `Camel.toml` `[beans.<name>]` `plugin` entries, embedded `Camel.toml` `[security.permissions.<name>]` WASM-provider `path` declarations, and asset references that escape the selected root or go missing. TLS-class references — document `tls` blocks, the `tlsCert`/`tlsKey` URI parameters, and the gRPC `*Path` family — SHALL resolve embedded-only: each reference resolves to an embedded virtual-store entry or compilation fails, and NO host-filesystem fallback exists at compile time or runtime. The gRPC `protoFile` parameter follows the same embedded-only resolution: its bytes come from the store through the confined per-boot materialization, and the sealed artifact never reads the proto from the deployment directory. The runtime gRPC URI parse accepts an absolute `protoFile` value only when its canonicalized path stays inside the OS temp directory that hosts the per-boot materialization directory; every other absolute path and every `..` traversal keeps failing closed. The document `wasm` field is a security-policy registry name, not an embedded asset, and SHALL compile as ordinary document data; there is no `sql` document field and no `plugin` document field. Certificate, key, and CA fields outside TLS or listener contexts remain ordinary document data. Runtime endpoint URI paths, deployment-time `${env:}` values, and deploy-side network/file I/O remain permitted. The `wasm:` URI-operand, `[beans]` plugin, and `[security.permissions]` WASM-provider-path rejections are R2-only deferrals recorded for their roadmap owner, not permanent rejections.

#### Scenario: Unsupported asset names the reason

- **GIVEN** a document containing a file-valued secret field or a stray compile-time `CAMEL_*` variable — any `CAMEL_*` name other than `CAMEL_COMPILE_SIGNING_KEY` supplied under `--sign`
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 without writing a usable artifact and names the rejected asset class

#### Scenario: Beans plugin declaration fails closed

- **GIVEN** a selected `Camel.toml` declaring a `[beans.<name>]` entry with a `plugin` field
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 naming the bean and explaining that cwd-relative plugin loading cannot be embedded, and no artifact is written

#### Scenario: Security-policy WASM provider path fails closed

- **GIVEN** a selected `Camel.toml` declaring `[security.permissions.<name>]` with a WASM-provider `path`
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 naming the policy and explaining that the path-taking policy constructor cannot consume embedded bytes in R2, and no artifact is written

#### Scenario: Dynamic asset placeholder is rejected

- **GIVEN** a TLS context field or TLS URI parameter whose certificate path is a dynamic `${env:}` placeholder
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 naming the field and explaining that an unknowable asset path cannot be embedded

#### Scenario: Absolute TLS URI parameter fails closed

- **GIVEN** an HTTP endpoint URI with `tlsCert=/etc/pki/cert.pem`
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 naming the parameter and requiring a root-relative compile-known path

#### Scenario: gRPC protoFile compiles into the store

- **GIVEN** a route with `from: grpc://127.0.0.1:50051/helloworld.Greeter/SayHello?protoFile=protos/helloworld.proto&transport=plaintext` and the proto file present under the selected root
- **WHEN** the operator invokes compilation
- **THEN** the proto embeds as a typed asset entry with the non-secret `proto file` class (no `--embed-secrets` opt-in required), the `protoFile` parameter receives a Uri-context substitution-table entry, and compilation exits 0

#### Scenario: Runtime parse confines absolute protoFile to the OS temp directory

- **GIVEN** a gRPC endpoint URI whose `protoFile` value is an absolute path
- **WHEN** the runtime parses the URI
- **THEN** a path canonically confined under the OS temp directory parses, while paths outside it and paths carrying `..` traversal fail closed naming the proto path

#### Scenario: Absolute protoFile fails closed

- **GIVEN** a gRPC endpoint URI with `protoFile=/etc/service/helloworld.proto`
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 naming the parameter and requiring a root-relative compile-known path

#### Scenario: Embedded job sources are allowed

- **GIVEN** a job document with route sources and configuration selected through explicit compile inputs
- **WHEN** the operator invokes compilation
- **THEN** the route sources and configuration are embedded in the virtual store and the job does not require external source files at runtime

#### Scenario: Job configuration is self-contained

- **GIVEN** a job document that depends on external config, profiles, includes, or a route source outside the document
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 before output creation and names the dependency

#### Scenario: TLS assets compile into the store

- **GIVEN** a route declaring certificate, private-key, and client-CA files under a `tls` block, with the files present under the selected root
- **WHEN** the operator invokes compilation
- **THEN** the files embed as typed asset entries and compilation exits 0

#### Scenario: TLS URI parameters compile into the store

- **GIVEN** a route with `https://…?tlsCert=certs/cert.pem&tlsKey=certs/key.pem` and a gRPC endpoint carrying `transport=tls&serverCertPath=…&serverKeyPath=…`, all files present under the selected root
- **WHEN** the operator invokes compilation
- **THEN** every referenced file embeds as a typed asset entry, the URI parameters receive substitution-table entries, and compilation exits 0

#### Scenario: Registry-name wasm field compiles unchanged

- **GIVEN** a document whose security policy declares `wasm` with a registry-name value
- **WHEN** the operator invokes compilation
- **THEN** the field compiles as ordinary document data and no asset entry is created for it

#### Scenario: Wasm URI operand fails closed

- **GIVEN** a document whose endpoint URI carries a `wasm:` operand naming a module file
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 without writing a usable artifact, naming the operand and explaining that `wasm:` module embedding is a recorded R2 deferral

### Requirement: Long-running route-server artifacts

A compiled route artifact SHALL run like `camel run` in a deployment
posture: after successful boot it SHALL keep running — with every
listener the embedded documents and embedded configuration declare
bound and serving — until the first stop signal. The SIGINT and SIGTERM
streams SHALL be armed before boot so a signal arriving during boot is
buffered and consumed by the shutdown wait rather than default-killing
the process. The first SIGINT or SIGTERM SHALL start a graceful
shutdown: in-flight listener work is given the configured drain budget
to complete, then listeners close, and the process exits 0, writing the
completed route report when `--report` was given; work still in flight
when the budget expires is dropped. A second SIGINT or SIGTERM received
while teardown is still running SHALL force-exit with code 1 without
waiting for teardown — the same first-graceful/second-force SIGNAL
contract `camel run` and `camel job` implement (outcome codes differ by
kind: a signal-stopped route exits 0; a job interruption keeps the job
taxonomy codes). A signal burst of identical signals MAY coalesce
before delivery; the force-exit hatch MUST accept either signal.
Compiled job artifacts SHALL keep the bounded exit-after-completion
behavior and SHALL NOT serve until a signal. For a signed artifact,
envelope verification SHALL finish before any listener binds: a failed
or missing required envelope exits 2 with no port ever bound.

#### Scenario: Route artifact serves its declared listener until SIGTERM

- **GIVEN** a compiled route artifact whose document declares a REST
  listener on a free local port
- **WHEN** the artifact starts and an HTTP request is sent to the
  listener after boot completes
- **THEN** the listener answers with the route's response while the
  process keeps running, and the artifact's `--manifest` output lists
  the listener

#### Scenario: gRPC consumer serves from a sealed artifact until SIGTERM

- **GIVEN** a compiled route artifact whose document declares a
  `grpc://` consumer route carrying
  `?protoFile=…&transport=plaintext`, deployed into a fresh
  source-free directory with the proto resolved from the embedded
  store
- **WHEN** the artifact boots and a client completes the HTTP/2
  connection preface against the declared port after the consumer
  reports it is serving
- **THEN** the server answers with its SETTINGS frame, the process
  keeps serving (a liveness probe observes it still running), the first
  SIGTERM drains gracefully with exit 0, the report file contains
  `{"kind":"route","status":"completed","error":null}`, and the
  artifact's `--manifest` output reports `artifact_kind` `server` with
  `grpc` among the components

#### Scenario: WebSocket consumer drains an in-flight exchange

- **GIVEN** a serving compiled route artifact started with
  `--report <path>` whose `ws://` consumer route holds a connected
  WebSocket exchange inside a delay step that completes within the
  configured drain budget
- **WHEN** SIGTERM arrives while the exchange is in flight
- **THEN** the in-flight WebSocket response still completes inside the
  drain budget, the process exits 0, and the report file contains
  `{"kind":"route","status":"completed","error":null}`

#### Scenario: First signal drains gracefully and exits 0

- **GIVEN** a serving compiled route artifact started with
  `--report <path>` and a listener request in flight whose route
  completes within the configured drain budget
- **WHEN** SIGTERM arrives while the request is in flight
- **THEN** the in-flight request completes inside the drain budget,
  listeners close, the process exits 0, and the report file contains
  `{"kind":"route","status":"completed","error":null}`

#### Scenario: Second signal force-exits during teardown

- **GIVEN** a booting compiled route artifact receives SIGINT followed
  by SIGTERM, both buffered before the shutdown wait begins
- **WHEN** boot finishes and the buffered signals are consumed — the
  first starting graceful teardown, the second already queued for the
  force-exit guard
- **THEN** the process exits with code 1 without waiting for teardown
  to complete

#### Scenario: Route artifact signal during boot is buffered

- **GIVEN** a compiled route artifact is loading configuration or
  booting components
- **WHEN** SIGTERM arrives mid-boot
- **THEN** the signal is buffered, boot reaches the shutdown wait, and
  the process shuts down gracefully with exit 0 instead of dying to the
  default disposition

#### Scenario: Deployment-equivalence with camel run

- **GIVEN** the same listener-bearing route document run once via
  `camel run --routes <doc> --no-watch` and once as a compiled artifact
- **WHEN** the listener is queried and SIGTERM is sent in both runs
- **THEN** both runs answer the query before the signal and exit 0
  after it — identical serve, drain, and exit behavior

#### Scenario: Job artifacts stay bounded

- **GIVEN** a compiled job artifact whose embedded routes include a
  direct consumer
- **WHEN** the artifact runs
- **THEN** it completes its bounded send and exits with the job outcome
  taxonomy codes, never serving until a signal

#### Scenario: Envelope verification precedes listener binding

- **GIVEN** a compiled route artifact whose manifest marks the detached
  signature envelope required and whose envelope bytes were corrupted
  after signing, leaving the artifact trailer itself valid
- **WHEN** the artifact starts
- **THEN** it exits 2 naming the failed signature-verification step and
  no declared listener port is ever bound
