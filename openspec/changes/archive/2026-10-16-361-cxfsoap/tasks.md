# Tasks: 361-cxfsoap

Detailed implementation plan and progress tracker for the six verified gaps
(A producer transport TLS, B envelope fidelity, C fault status, D signer
identity, E listener TLS, F schema set). Phases match `design.md ## Phases`
and are plan-blessed once. Each task block is independently dispatchable and
scoped to at most 30 minutes.

## Execution environment (apply to every command)

All work happens inside the worktree `/home/shared/rust-camel-worktrees/361-cxfsoap`.
Never build or test in the main checkout. Export once per shell:

```bash
export CARGO_TARGET_DIR=/home/shared/rust-camel-worktrees/361-cxfsoap/target
export CARGO_BUILD_JOBS=4
export GRADLE_USER_HOME=/home/shared/rust-camel-worktrees/361-cxfsoap/.gradle-user
export TMPDIR=/home/shared/tmp
```

## Global constraints (apply to every task)

- Before any build, read the free space of the filesystem that holds `target`.
  If usage is at or above 78 percent, stop and report; do not start a build.
- Never run `git fetch`, `git push`, `git tag`, or `bd close`.
- Never run a broad `--all-features` cargo command; use the exact `-p` package
  and `--test` target named in each task.
- Java tests are Gradle JUnit tests under `bridges/<name>/src/test/java`.
  Run them through the wrapper at `bridges/<name>/gradlew`.
- This change is data-plane only. Every proto-carrying value is untrusted and
  bounded (ADR-0032). Bridge IPC mTLS (ADR-0036) is unchanged.
- Signer metadata is never authentication: no `camel.auth.*` write, no
  `Principal`, no `store_principal_properties`.
- Phase 1 SHALL NOT modify any `.proto` file or any generated proto module;
  the proto bump and `timeout_ms` reconciliation are owned by Task 2.3.
- SOAP binding identity is the runtime Jakarta constant returned by
  `dispatch.getBinding().getBindingID()`:
  `jakarta.xml.ws.soap.SOAPBinding.SOAP11HTTP_BINDING` =
  `http://schemas.xmlsoap.org/wsdl/soap/http`, and
  `SOAPBinding.SOAP12HTTP_BINDING` =
  `http://www.w3.org/2003/05/soap/bindings/HTTP/`. The WSDL namespace strings
  `.../wsdl/soap/` and `.../wsdl/soap12/` are NOT binding IDs and SHALL NOT be
  used for version selection.

## Phase 1: Producer transport + envelope fidelity (A + B)

**Subdeliverables (ordered):** capture real SOAP wire RED first, then PAYLOAD
unwrap and binding-derived version, then SOAP 1.2/1.1 wire and explicit
binding failure, then payload/version verification and §B docs, then TLS
config plumbing, then conduit `TLSClientParameters` with pre-ready store
validation, then real-native mutual-TLS integration and docs.

### camel-component-cxf + bridges/cxf + camel-bridge + camel-test

#### Task 1.1: Capture the real producer SOAP wire (RED-first)

**Files:**
- `crates/camel-test/Cargo.toml` (modified)
- `crates/camel-test/tests/cxf_test.rs` (modified)
- `crates/camel-test/tests/support/cxf.rs` (modified)

**Steps:**
1. Add `quick-xml.workspace = true` to `[dev-dependencies]` in
   `crates/camel-test/Cargo.toml` (the workspace already declares
   `quick-xml = "0.42"`) so the captured wire can be parsed namespace-aware.
2. Build the native bridge that will produce the RED baseline:
   `cargo xtask build-cxf-bridge`. Do not modify any Java before the RED is
   captured; record the exact binary path used.
3. In `support/cxf.rs` add `enum SoapVersion { Soap11, Soap12 }` and
   `async fn start_capturing_mock_soap_service(version: SoapVersion) -> (SocketAddr, Arc<Mutex<Vec<CapturedRequest>>>)`
   where `CapturedRequest { headers: Vec<(String, String)>, body: String }`. The
   Axum `post` handler stores the request headers and body and returns a
   **version-matched** `sayHelloResponse` using the real fixture namespace:
   - `Soap11` replies with the SOAP 1.1 envelope
     (`http://schemas.xmlsoap.org/soap/envelope/`) and `content-type: text/xml`.
   - `Soap12` replies with the SOAP 1.2 envelope
     (`http://www.w3.org/2003/05/soap-envelope`) and
     `content-type: application/soap+xml` carrying the action.
   In both variants the body is
   `<hel:sayHelloResponse><hel:message>Hello World</hel:message>`
   `</hel:sayHelloResponse>` under the response envelope's `Body`. The fixture
   `hello.wsdl` (`examples/cxf-example/wsdl/hello.wsdl`) declares
   `targetNamespace="http://example.com/hello"`, request element
   `sayHelloRequest`, and response element `sayHelloResponse` — there is no
   `http://example.org/hello` namespace and no `sayHelloRequest`-less `sayHello`
   element, so every payload and assertion below uses the fixture values. The
   service SHALL NOT return a SOAP 1.1 response for a SOAP 1.2 request: the
   success roundtrip and the captured-wire assertion below run for BOTH
   versions.
4. In `cxf_test.rs` add `cxf_producer_wire_single_envelope_direct_operation_child()`:
   start the capturing service, build a CXF producer pool pointed at it using
   `examples/cxf-example/wsdl/hello.wsdl`, send a bare
   `<hel:sayHelloRequest xmlns:hel="http://example.com/hello">`
   `<hel:name>World</hel:name></hel:sayHelloRequest>` body through
   `send_to_direct`, then read the captured request.
5. Parse the captured body with `quick_xml::Reader` with namespace resolution
   and assert the namespace-aware structure: exactly one
   `{http://schemas.xmlsoap.org/soap/envelope/}Envelope`, exactly one
   `{...}Body`, and a `{http://example.com/hello}sayHelloRequest` direct child of
   `Body`. Do NOT count textual prefixes. Also assert the captured request
   headers contain a `text/xml` `content-type` and a non-empty `SOAPAction`.
6. Run the test on the pre-fix native binary and record the RED output:
   confirm the failing assertion is the duplicate inner `Envelope` (the parsed
   `Body` has a child `Envelope`), not a parse or compile error.

**Tests:**
- `cxf_producer_wire_single_envelope_direct_operation_child`: setup capturing
  Axum service with `SoapVersion::Soap11` plus producer route to
  `http://127.0.0.1:<port>/service` using
  `examples/cxf-example/wsdl/hello.wsdl`; action invoke operation `sayHello`
  with payload root `{http://example.com/hello}sayHelloRequest`; assert the
  success roundtrip returns the fixture response, and namespace-aware one
  `Envelope`, one `Body`, the `{http://example.com/hello}sayHelloRequest` element
  a direct child of `Body`, `content-type` contains `text/xml`, and `SOAPAction`
  present.
  Command: `cargo test -p camel-test --features integration-tests --test cxf_test cxf_producer_wire_single_envelope_direct_operation_child`.
  Expected: RED before Task 1.2 (fails on the duplicate inner `Envelope`),
  GREEN after Task 1.2 against the rebuilt native bridge.

**Acceptance:**
- The test exists and fails on the pre-fix native binary for the
  duplicate-envelope reason (captured body shows a second `Envelope`).
- The assertion parses namespaces and asserts DOM structure; it does not count
  textual prefix occurrences.

- [x] 1.1

#### Task 1.2: Remove manual envelope wrap; binding-derived version; reject full Envelope

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfBridgeService.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEnvelopeHelper.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEnvelopeHelperTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/CxfBridgeServiceTest.java` (modified)
- `crates/camel-test/tests/cxf_test.rs` (modified)

**Steps:**
1. In `CxfBridgeService.invoke` delete the `soap-version` header read
   (`CxfBridgeService.java:89`) and the `SoapEnvelopeHelper.wrapInEnvelope`
   call (`:90`); pass `payload` unchanged to the `Mode.PAYLOAD` dispatch.
2. After `getDispatch`, read `String bindingId = dispatch.getBinding().getBindingID()`.
   Accept `jakarta.xml.ws.soap.SOAPBinding.SOAP11HTTP_BINDING`
   (`http://schemas.xmlsoap.org/wsdl/soap/http`) and
   `SOAPBinding.SOAP12HTTP_BINDING`
   (`http://www.w3.org/2003/05/soap/bindings/HTTP/`). On any other value return
   `Status.INVALID_ARGUMENT.withDescription("unsupported WSDL binding ID: " + bindingId)`
   and send no request.
3. Before dispatch, parse the payload root; if the root element local name is
   `Envelope` in either SOAP envelope namespace, return
   `Status.INVALID_ARGUMENT.withDescription("payload is already a SOAP Envelope")`
   and send no request.
4. Delete `wrapInEnvelope` from `SoapEnvelopeHelper.java` and delete the
   corresponding test case from `SoapEnvelopeHelperTest.java`.
5. Update `CxfBridgeServiceTest.java` with
   `soapVersionComesFromBindingNotHeader` and
   `prewrappedEnvelopeIsRejectedBeforeInvoke` using a client stub that counts
   invokes and captures the payload handed to the dispatch. The mocked dispatch
   performs NO envelope serialization — CXF serializes the `Envelope` on the
   wire, not at the dispatch — so `soapVersionComesFromBindingNotHeader` asserts
   only that the payload reaching the dispatch is the bare operation element
   unchanged, that the binding ID was inspected and accepted, and that the
   `soap-version` header changed nothing. It SHALL NOT assert an envelope
   namespace at the mock; the real-wire tests (Task 1.1 and Tasks 1.3/1.4)
   prove the emitted envelope version.
6. Rebuild the native bridge with `cargo xtask build-cxf-bridge`, then confirm
   Task 1.1 is GREEN against the rebuilt binary.

**Tests:**
- `CxfBridgeServiceTest.soapVersionComesFromBindingNotHeader`: setup binding ID
  `SOAPBinding.SOAP11HTTP_BINDING` and header `soap-version: 1.2`, client stub
  counting invokes and capturing the dispatched payload; action invoke; assert
  the payload received by the dispatch is the bare `sayHelloRequest` element
  unchanged (no `Envelope`/`Body` added at the mock), the binding ID was read
  and accepted, and the header changed nothing. Do NOT assert envelope
  serialization here; the wire-level SOAP 1.1/1.2 assertions live in the
  real-wire tests. Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.CxfBridgeServiceTest"`.
  Expected: GREEN after this task.
- `CxfBridgeServiceTest.prewrappedEnvelopeIsRejectedBeforeInvoke`: setup payload
  root `{http://schemas.xmlsoap.org/soap/envelope/}Envelope`, client stub
  counting invokes; action invoke; assert `INVALID_ARGUMENT` and zero invokes.
  Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.CxfBridgeServiceTest"`. Expected: GREEN after this task.
- `cxf_producer_wire_single_envelope_direct_operation_child`: setup the
  capturing service from this task; action run the producer again after the
  native rebuild; assert one `{http://schemas.xmlsoap.org/soap/envelope/}Envelope`,
  one `{...}Body`, and a `{http://example.com/hello}sayHelloRequest` element as a
  direct `Body` child. Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_producer_wire_single_envelope_direct_operation_child`.
  Expected: GREEN.

**Acceptance:**
- `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck` exits 0.
- The Task 1.1 integration test exits 0 against the rebuilt native binary.
- `rg "wrapInEnvelope" bridges/cxf/src` returns no production hit.

- [x] 1.2

#### Task 1.3: SOAP 1.1/1.2 binding emits the matching wire; unrecognized binding fails

**Files:**
- `examples/cxf-example/wsdl/hello-soap12.wsdl` (new)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfBridgeService.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/CxfBridgeServiceTest.java` (modified)
- `crates/camel-test/tests/cxf_test.rs` (modified)

**Steps:**
1. Add `examples/cxf-example/wsdl/hello-soap12.wsdl`: a copy of `hello.wsdl`
   whose `wsdl:binding` uses the `http://schemas.xmlsoap.org/wsdl/soap12/`
   binding namespace with `soap12:binding transport="http://schemas.xmlsoap.org/soap/http"`
   and a `soap12:operation soapAction` so CXF reports the SOAP 1.2 HTTP
   binding ID `http://www.w3.org/2003/05/soap/bindings/HTTP/`.
2. Add `cxf_producer_soap12_binding_emits_soap12` in `cxf_test.rs`: start the
   capturing service with `SoapVersion::Soap12` (so the success roundtrip uses a
   SOAP 1.2 `application/soap+xml` response), drive the producer with the SOAP
   1.2 WSDL, assert the invocation succeeds, and assert the captured request
   uses the SOAP 1.2 envelope namespace
   (`http://www.w3.org/2003/05/soap-envelope`) with an
   `application/soap+xml` `content-type` carrying the action.
3. Add `cxf_producer_soap11_binding_sets_text_xml`: start the capturing service
   with `SoapVersion::Soap11`, drive the producer with `hello.wsdl`, assert the
   invocation succeeds, and assert the SOAP 1.1 envelope namespace
   (`http://schemas.xmlsoap.org/soap/envelope/`), a `text/xml` content type, and
   a `SOAPAction` header.
4. Add `CxfBridgeServiceTest.unsupportedBindingNoDispatch`: install a concrete
   custom `Binding` stub whose `getBindingID()` returns `urn:custom` (a real
   WSDL cannot advertise an unsupported binding ID, so the stub supplies it);
   assert `INVALID_ARGUMENT` naming `urn:custom` and zero client invokes.
5. Confirm the `soap-version` header has no effect on either binding.

**Tests:**
- `cxf_producer_soap12_binding_emits_soap12`: setup the SOAP 1.2 WSDL and a
  capturing service started with `SoapVersion::Soap12`; action invoke; assert
  the success roundtrip returns the fixture response and the captured request
  carries the SOAP 1.2 namespace plus `application/soap+xml`. Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_producer_soap12_binding_emits_soap12`.
  Expected: RED on the pre-Task-1.2 tree (SOAP version is not derived from the
  binding); the real-wire RED baseline is captured by Task 1.1 before any
  envelope fix; GREEN after Task 1.3 adds the SOAP 1.2 WSDL fixture on top of
  Task 1.2.
- `cxf_producer_soap11_binding_sets_text_xml`: setup the SOAP 1.1 WSDL and a
  capturing service started with `SoapVersion::Soap11`; action invoke; assert
  the success roundtrip returns the fixture response and the captured request
  carries the SOAP 1.1 namespace, `text/xml`, and `SOAPAction`. Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_producer_soap11_binding_sets_text_xml`.
  Expected: RED on the pre-Task-1.2 tree; GREEN after Task 1.3.
- `CxfBridgeServiceTest.unsupportedBindingNoDispatch`: setup a concrete custom
  `Binding` stub whose `getBindingID()` returns `urn:custom`; action invoke;
  assert `INVALID_ARGUMENT` naming `urn:custom` and no client invoke. Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.CxfBridgeServiceTest"`.
  Expected: GREEN after.

**Acceptance:**
- `cxf_producer_soap11_binding_sets_text_xml` and
  `cxf_producer_soap12_binding_emits_soap12` exit 0.
- `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck` exits 0.
- No production code path reads a `soap-version` header or a SOAP binding ID
  other than the two Jakarta constants.

- [x] 1.3

#### Task 1.4: Payload/version verification on the real wire and §B documentation

**Files:**
- `crates/camel-test/tests/cxf_test.rs` (modified)
- `docs/src/components/ws-soap.md` (modified)
- `crates/components/camel-cxf/CONTEXT.md` (modified)

**Steps:**
1. Add `cxf_producer_soap_version_header_ignored_on_wire`: send a request with
   header `soap-version: 1.2` through the SOAP 1.1 WSDL against the capturing
   service; assert the captured envelope is SOAP 1.1.
2. Add `cxf_producer_prewrapped_envelope_sends_no_request`: send a full
   `{http://schemas.xmlsoap.org/soap/envelope/}Envelope` payload; assert the
   invocation fails and the capturing server recorded zero requests.
3. Add `cxf_producer_unknown_binding_sends_no_request` only if a native case is
   needed. A real WSDL cannot advertise an unrecognized binding ID; pointing CXF
   at one fails WSDL/port parsing, which is a different purpose. If kept, use a
   WSDL with invalid ports and assert the invocation fails during WSDL/port
   resolution with zero captured requests. The no-dispatch assertion for an
   unsupported binding ID is owned by the Java
   `CxfBridgeServiceTest.unsupportedBindingNoDispatch` stub test in Task 1.3.
4. Document §B in `ws-soap.md` under CXF URI/producer: the payload is a bare
   operation element, `Mode.PAYLOAD` emits exactly one Envelope/one Body, the
   version derives from the selected WSDL port binding, `soap-version` is
   ignored, and a pre-wrapped Envelope is rejected before the network.
5. Record the version-source decision (binding ID over `soap-version`) and the
   pre-wrapped rejection in `crates/components/camel-cxf/CONTEXT.md`.

**Tests:**
- `cxf_producer_soap_version_header_ignored_on_wire`: setup SOAP 1.1 WSDL plus
  capturing service and header `soap-version: 1.2`; action invoke; assert SOAP
  1.1 envelope. Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_producer_soap_version_header_ignored_on_wire`.
  Expected: RED before Task 1.2, GREEN after.
- `cxf_producer_prewrapped_envelope_sends_no_request`: setup capturing service
  and a full-Envelope payload; action invoke; assert failure plus zero captured
  requests. Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_producer_prewrapped_envelope_sends_no_request`.
  Expected: RED before Task 1.2, GREEN after.
- `cxf_producer_unknown_binding_sends_no_request` (optional native case): setup
  a WSDL with invalid ports and a capturing service; action invoke; assert
  failure during WSDL/port resolution plus zero captured requests. This is a
  WSDL/port-resolution failure, not the unsupported-binding-ID no-dispatch path,
  which the Java stub test covers. Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_producer_unknown_binding_sends_no_request`.
  Expected: RED before Task 1.3, GREEN after.

**Acceptance:**
- All three tests exit 0 against the rebuilt native bridge.
- `ws-soap.md` and `crates/components/camel-cxf/CONTEXT.md` state the
  single-envelope shape, the binding-derived version, the ignored header, and
  the pre-wrapped rejection.
- This task introduces no `.proto` change.

- [x] 1.4

#### Task 1.5: Producer TLS config plumbing and startup validation

**Files:**
- `crates/components/camel-cxf/src/config.rs` (modified)
- `crates/services/camel-bridge/src/process.rs` (modified)
- `crates/components/camel-cxf/src/pool.rs` (modified)
- `crates/components/camel-cxf/src/pool_env_test.rs` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfileStore.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileStoreTest.java` (modified)

**Steps:**
1. In `config.rs` add `#[derive(Clone, Default, serde::Deserialize)] pub struct CxfTlsConfig`
   with `keystore_path`, `keystore_password`, `keystore_type`, `truststore_path`,
   `truststore_password`, `truststore_type`, and
   `#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
   pub enum KeyStoreType` with explicit `#[serde(rename = "PKCS12")]` on the
   default `Pkcs12` variant and `#[serde(rename = "JKS")]` on `Jks`, so TOML
   `PKCS12`/`JKS` deserialize and other spellings are rejected.
2. Implement `Debug` for `CxfTlsConfig` manually, redacting `keystore_password`
   and `truststore_password` as `<redacted>`, mirroring the existing
   `CxfSecurityFields` `Debug` (the derive would leak them). Add
   `#[serde(default)] pub tls: Option<CxfTlsConfig>` to `CxfSecurityFields`.
3. Validate in `CxfPoolConfig::validate` for every profile: for each of
   keystore and truststore, `*_path` and `*_password` SHALL be supplied
   together **(bidirectional)** — a path without a password and a password
   without a path both fail; a `truststore_path` always requires a password;
   `*_type` is `JKS` or `PKCS12`; return a `CamelError::Config` naming the
   missing pair member.
4. In `crates/services/camel-bridge/src/process.rs` extend `CxfProfileEnvVars`
   with `pub tls: Option<CxfTlsEnvVars>` where `CxfTlsEnvVars` uses
   `Redacted<String>` for passwords (matching the existing fields), and emit
   `CXF_PROFILE_<NAME>_TLS_KEYSTORE_PATH`, `_TLS_KEYSTORE_PASSWORD`,
   `_TLS_KEYSTORE_TYPE`, `_TLS_TRUSTSTORE_PATH`, `_TLS_TRUSTSTORE_PASSWORD`,
   `_TLS_TRUSTSTORE_TYPE` from `to_env_vars`.
5. Add `tls: None` (or the mapped value) to every `CxfProfileEnvVars` struct
   literal so the workspace compiles (grep-verified; all nine sites):
   `crates/services/camel-bridge/src/process.rs` (test literals at lines 676,
   696, 798, 837, 1047), `crates/components/camel-cxf/src/pool.rs` (profile
   construction at lines 312 and 511),
   `crates/components/camel-cxf/src/pool_env_test.rs:22`, and
   `crates/camel-test/tests/cxf_test.rs:337` (the `cxf_native_health_check_responds_within_5s`
   literal — the plan previously missed this one).
6. In `SecurityProfile.java` add `tlsKeystorePath`, `tlsKeystorePassword`,
   `tlsKeystoreType`, `tlsTruststorePath`, `tlsTruststorePassword`,
   `tlsTruststoreType` plus builder methods `tlsKeystore`/`tlsTruststore` and
   getters.
7. In `SecurityProfileStore.java` read the six `TLS_*` env keys; throw
   `IllegalStateException` before `ready` when a path is set without its
   password (or a password without a path) or a type is not `JKS`/`PKCS12`.

**Tests:**
- `config::tests::tls_path_without_password_fails_validation`: setup profile
  `tls.keystore_path` only; action `CxfPoolConfig::validate`; assert
  `CamelError::Config` naming `keystore_password`. Command:
  `cargo test -p camel-component-cxf tls_path_without_password_fails_validation`.
  Expected: RED before, GREEN after.
- `config::tests::tls_password_without_path_fails_validation`: setup
  `tls.truststore_password` only; action validate; assert `CamelError::Config`
  naming `truststore_path`. Command:
  `cargo test -p camel-component-cxf tls_password_without_path_fails_validation`.
  Expected: RED before, GREEN after.
- `config::tests::keystore_type_defaults_to_pkcs12`: setup a
  `keystore_path`/`keystore_password` pair with no type; action build env vars;
  assert `TLS_KEYSTORE_TYPE = PKCS12`. Command:
  `cargo test -p camel-component-cxf keystore_type_defaults_to_pkcs12`.
  Expected: RED before, GREEN after.
- `config::tests::keystore_type_rejects_unknown_spelling`: setup
  `keystore_type = "jks"`; action parse; assert deserialization fails.
  Command:
  `cargo test -p camel-component-cxf keystore_type_rejects_unknown_spelling`.
  Expected: RED before, GREEN after.
- `pool_env_test::tls_env_vars_are_prefixed_and_redacted`: setup a TLS profile;
  action build env vars and Debug-print them; assert the six
  `CXF_PROFILE_<NAME>_TLS_*` keys exist and neither password value appears in
  the keys list nor in `CxfTlsConfig`'s Debug. Command:
  `cargo test -p camel-component-cxf tls_env_vars_are_prefixed_and_redacted`.
  Expected: RED before, GREEN after.
- `SecurityProfileStoreTest.readsTlsKeysAndRejectsIncompletePair`: setup env
  source with a TLS path and no password; action construct the store; assert
  `IllegalStateException`. Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SecurityProfileStoreTest"`.
  Expected: GREEN after.

**Acceptance:**
- `cargo test -p camel-component-cxf` and `cargo test -p camel-bridge` exit 0.
- `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck` exits 0.
- `format!("{:?}", CxfSecurityFields { .. })` and `format!("{:?}", CxfTlsConfig { .. })`
  contain no password value.
- The enum deserializes `"PKCS12"`/`"JKS"` only; no other spelling is accepted.

- [x] 1.5

#### Task 1.6: Apply `TLSClientParameters` and eagerly validate stores before ready

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfClientManager.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfServerManager.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/CxfClientManagerTest.java` (modified)

**Steps:**
1. In `CxfClientManager.createDispatch`, when the profile has a `tls` block,
   build a `TLSClientParameters` from the profile's TLS stores
   (`KeyManagerFactory` from the keystore, `TrustManagerFactory` from the
   truststore) and attach it to the CXF `HTTPConduit` obtained from the
   dispatch's client conduit.
2. Default the store type to `PKCS12` when `tlsKeystoreType`/`tlsTruststoreType`
   is absent; load with the configured type otherwise.
3. When there is no `tls` block, install no `TLSClientParameters` so the
   dispatch uses JVM defaults.
4. Retain hostname verification; expose no option that disables it.
5. Add `CxfClientManager.initTlsStores()` that loads and validates every
   profile's TLS stores eagerly, and invoke it from the bridge startup sequence
   before the gRPC server announces `ready` (from `CxfServerManager` startup or
   the CDI `@PostConstruct` path). A store that cannot be opened (wrong
   password) SHALL throw during startup so the process exits before the
   `{"status":"ready",...}` line; a per-dispatch lazy open is not acceptable
   evidence.
6. Keep `createDispatch`'s per-dispatch store use as the runtime path.

**Tests:**
- `CxfClientManagerTest.tlsParametersAttachedWhenTlsConfigured`: setup profile
  with `tlsKeystore`/`tlsTruststore`; action `getDispatch`; assert the conduit
  carries `TLSClientParameters`. Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.CxfClientManagerTest"`.
  Expected: RED before, GREEN after.
- `CxfClientManagerTest.noTlsBlockLeavesJvmDefaults`: setup profile without
  `tls`; action `getDispatch`; assert no explicit `TLSClientParameters`.
  Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.CxfClientManagerTest"`. Expected: GREEN after.
- `CxfClientManagerTest.wrongStorePasswordFailsBeforeReady`: setup a wrong
  keystore password; action `initTlsStores()`; assert it throws before any
  dispatch and before any `ready` announcement. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.CxfClientManagerTest"`. Expected:
  RED before, GREEN after.
- `cxf_producer_wrong_tls_password_never_reaches_ready`: setup a CXF pool with
  a wrong store password; action start the bridge via `BridgeProcess::start`;
  assert startup fails and no `{"status":"ready"}` line is observed. Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_producer_wrong_tls_password_never_reaches_ready`.
  Expected: RED before, GREEN after. Requires the rebuilt native bridge.

**Acceptance:**
- `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck` exits 0.
- No config key or code path disables hostname verification.
- A wrong store password aborts startup before the gRPC `ready` line is
  emitted.

- [x] 1.6

#### Task 1.7: Real-native mutual-TLS integration, isolation, and producer TLS docs

**Files:**
- `crates/camel-test/tests/cxf_test.rs` (modified)
- `crates/camel-test/tests/support/cxf.rs` (modified)
- `docs/src/components/ws-soap.md` (modified)
- `crates/components/camel-cxf/CONTEXT.md` (modified)

**Steps:**
1. Add a TLS test server helper in `support/cxf.rs` that serves HTTPS with a
   CA-signed server certificate and requires a client certificate, parameterized
   by server cert/CA so wrong-CA and wrong-SAN variants can be constructed.
2. Add `cxf_producer_tls_presents_client_certificate`: against the real native
   bridge, assert the handshake completes and the trusted server accepts the
   configured client certificate.
3. Add `cxf_producer_tls_no_client_cert_is_rejected`,
   `cxf_producer_tls_wrong_ca_is_rejected`, and
   `cxf_producer_tls_wrong_san_is_rejected`: each establishes the TLS
   connection far enough to observe a specific failure (server rejects a
   missing client cert; client rejects an untrusted server CA; client rejects a
   certificate whose SAN does not match), NOT just "some error".
4. Add `cxf_producer_tls_cross_profile_isolation`: profile `baleares` with TLS
   material presents a certificate; profile `cyl` without TLS fails the
   handshake.
5. Add `cxf_producer_truststore_only_validates_server_without_client_cert`.
6. Document `[profiles.security.tls]` in `ws-soap.md` under CXF
   URI/security, including the `JKS`/`PKCS12` types, the bidirectional
   path/password pairing, the PKCS12 default, and that transport TLS is
   independent of the WSS message-layer stores; record the separation in
   `crates/components/camel-cxf/CONTEXT.md`.

**Tests:**
- `cxf_producer_tls_presents_client_certificate`: setup mTLS server and a TLS
  profile; action invoke; assert success and that the server recorded the
  expected client certificate subject. Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_producer_tls_presents_client_certificate`.
  Expected: RED before, GREEN after. Requires the native cxf bridge binary.
- `cxf_producer_tls_no_client_cert_is_rejected`: setup mTLS server requiring a
  client cert and a profile with only a truststore; action invoke; assert the
  server rejects the handshake (client-auth failure), not a timeout. Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_producer_tls_no_client_cert_is_rejected`.
  Expected: RED before, GREEN after.
- `cxf_producer_tls_wrong_ca_is_rejected`: setup a server whose certificate is
  signed by an unknown CA; action invoke; assert a certificate-path/trust
  failure. Command: same file with the test name. Expected: RED before, GREEN
  after.
- `cxf_producer_tls_wrong_san_is_rejected`: setup a valid trusted server whose
  SAN does not match the endpoint host; action invoke; assert a
  hostname-verification failure. Command: same file with the test name.
  Expected: RED before, GREEN after.
- `cxf_producer_tls_cross_profile_isolation`: setup profiles `baleares` (TLS)
  and `cyl` (none); action invoke both; assert `baleares` presents a
  certificate and `cyl` fails. Command: same file with the test name. Expected:
  RED before, GREEN after.
- `cxf_producer_truststore_only_validates_server_without_client_cert`: setup a
  truststore-only profile; action invoke; assert the server certificate is
  validated and no client certificate is presented. Command: same file with the
  test name. Expected: RED before, GREEN after.

**Acceptance:**
- The six named tests pass against the native bridge binary built by
  `cargo xtask build-cxf-bridge`.
- `cargo test -p camel-test --features integration-tests --test cxf_test`
  exits 0.
- The negative tests assert the specific TLS failure mode, not merely that the
  call returned an error.

- [x] 1.7

## Phase 2: Consumer production surface (C + D + E)

**Subdeliverables (ordered):** fault renderer, fault matrix, signer + timeout
proto, identity metadata, listener config, mutual listener.

### bridges/cxf + camel-component-cxf + camel-bridge + camel-test

#### Task 2.1: Version-matched, namespace-correct fault renderer

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherTest.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/WssSecurityProcessor.java` (modified)

**Steps:**
1. In `SoapEndpointPublisher.handleRequestBody`, detect the SOAP version before
   WSS from the envelope root namespace; when the root is unreadable, fall back
   to the content type: `application/soap+xml` selects SOAP 1.2, `text/xml` or
   absent selects SOAP 1.1; only when neither is usable default to SOAP 1.1
   for rendering.
2. Replace `buildSoapFault`/`buildFaultBody` with one fault builder that emits
   a version-matched envelope with declared QName prefixes. SOAP 1.1 uses
   `faultcode`/`faultstring` with a declared prefix (never an undeclared
   `soap:`). SOAP 1.2 uses `Code`/`Value` and `Reason`/`Text` with `xml:lang`.
3. Pass the detected version into WSS processing rather than re-detecting it
   after.

**Tests:**
- `SoapEndpointPublisherTest.soap11FaultPrefixIsDeclared`: setup SOAP 1.1
  fault; action render; assert the fault code prefix resolves to
  `http://schemas.xmlsoap.org/soap/envelope/`. Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"`.
  Expected: RED before, GREEN after.
- `SoapEndpointPublisherTest.soap12FaultUses12Structure`: setup SOAP 1.2 fault;
  action render; assert `Code/Value` and `Reason/Text` with `xml:lang` and no
  `faultcode`. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"`. Expected: RED before, GREEN after.
- `SoapEndpointPublisherTest.contentTypeFallbackSelectsVersion`: setup an
  unreadable root with `application/soap+xml` and with `text/xml`; action
  render; assert 1.2 and 1.1 respectively. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"`. Expected: RED
  before, GREEN after.
- `SoapEndpointPublisherTest.versionDetectedBeforeWss`: setup a SOAP 1.2 root
  and a WSS processor recording the version it saw; action process; assert WSS
  saw 1.2 before any security failure. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"`. Expected: RED before,
  GREEN after.

**Acceptance:**
- Rendered SOAP 1.1 faults parse with the SOAP prefix bound to the SOAP 1.1
  envelope namespace.
- `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck` exits 0.

- [x] 2.1

#### Task 2.2: Fault matrix and per-profile `security.fault_status` override (WSS only)

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfileStore.java` (modified)
- `crates/components/camel-cxf/src/config.rs` (modified)
- `crates/services/camel-bridge/src/process.rs` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherTest.java` (modified)
- `crates/camel-test/tests/cxf_test.rs` (modified)

**Steps:**
1. In `SoapEndpointPublisher`, classify every failure as `Sender` or `Receiver`
   per the matrix: WSS failure `Sender`, malformed envelope `Sender`, missing
   `Body` `Sender`, signer-descriptor validation failure `Receiver`, route
   error `Receiver`, timeout `Receiver`, Rust `soap:Client` `Sender`, Rust
   `soap:Server` `Receiver`, any other SOAP 1.2 fault code HTTP 500.
2. Map status: SOAP 1.1 any fault 500; SOAP 1.2 `Sender` 400 and `Receiver`
   500; success 200; a fault never returns 200.
3. Add `fault_status: Option<u16>` to `CxfSecurityFields` (the derived
   `Default` and the manual `Debug` at `config.rs:83` both cover it; the two
   full `CxfSecurityFields { .. }` literals at `component.rs:271` and
   `config.rs:837` already use `..Default::default()`, so they need no edit).
   Add `pub fault_status: Option<u16>` to `CxfProfileEnvVars` in
   `crates/services/camel-bridge/src/process.rs` and emit
   `CXF_PROFILE_<NAME>_FAULT_STATUS` from `to_env_vars`; add the field to the
   same nine `CxfProfileEnvVars { .. }` literals listed in Task 1.5
   (`process.rs:676,696,798,837,1047`, `pool.rs:312,511`,
   `pool_env_test.rs:22`, `cxf_test.rs:337`). Validate at startup as an integer
   in 400 through 599 (101, 204, 399, 600 rejected) with an actionable error,
   and map `fault_status: p.security.fault_status` in **both** profile
   constructions in `pool.rs` (`:309-332` startup and `:509-549` restart) so it
   survives a health-monitor restart exactly like the listener TLS.
4. In `SecurityProfile`/`SecurityProfileStore`, carry `faultStatus` and apply
   it to **security (WSS) faults only**, on both SOAP versions; no global
   override; the signer-descriptor failure is a non-WSS `Receiver` fault and
   keeps 500.
5. Add integration cases covering security, malformed envelope, missing Body,
   route error, timeout, `soap:Client`, `soap:Server`, SOAP 1.1, and a `403`
   override proving non-WSS faults are untouched.
6. Document `security.fault_status` in `ws-soap.md` and the fault matrix in
   `crates/components/camel-cxf/CONTEXT.md`.

**Tests:**
- `config::tests::fault_status_out_of_range_rejected`: setup `fault_status =
  399`, `204`, and `600`; action `CxfPoolConfig::validate`; assert each fails.
  Command: `cargo test -p camel-component-cxf fault_status_out_of_range_rejected`.
  Expected: RED before, GREEN after.
- `SoapEndpointPublisherTest.senderFaultIs400AndReceiverFaultIs500`: setup SOAP
  1.2 security failure and route failure; action respond; assert 400 and 500.
  Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"`.
  Expected: RED before, GREEN after.
- `SoapEndpointPublisherTest.soap11AnyFaultIs500`: setup SOAP 1.1 Sender and
  Receiver failures; action respond; assert both 500. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"`.
  Expected: GREEN after.
- `cxf_consumer_override_403_applies_to_wss_only`: setup profile `esap` with
  `fault_status = 403` and profile `cyl` with defaults; action a SOAP 1.2 WSS
  fault on each, a route (`Receiver`) fault on `esap`, and a malformed-envelope
  (`Sender`) fault on `esap`; assert `esap` WSS is 403, `cyl` WSS is 400, and
  `esap` route is 500 and `esap` malformed is 400. Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_consumer_override_403_applies_to_wss_only`.
  Expected: RED before, GREEN after.
- `cxf_consumer_override_applies_on_soap11`: setup profile with
  `fault_status = 400`; action a SOAP 1.1 WSS fault; assert the override
  applies and the status is 400 (not the 500 default). Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_consumer_override_applies_on_soap11`.
  Expected: RED before, GREEN after.

**Acceptance:**
- `cargo test -p camel-component-cxf` exits 0 and
  `cargo test -p camel-test --features integration-tests --test cxf_test`
  exits 0.
- The `403` override returns 403 only for WSS faults; Receiver non-WSS stays
  500 and Sender non-WSS stays 400.

- [x] 2.2

#### Task 2.3: `SignerIdentity signer = 7` and `uint64 timeout_ms = 7` reconciliation

**Files:**
- `crates/components/camel-cxf/proto/cxf_bridge.proto` (modified)
- `bridges/cxf/src/main/proto/cxf_bridge.proto` (modified)
- `crates/components/camel-cxf/src/producer.rs` (modified)
- `crates/components/camel-cxf/src/bridge_client_test.rs` (modified)
- `crates/components/camel-cxf/tests/consumer_emission_test.rs` (modified)
- `crates/components/camel-cxf/tests/consumer_unit_test.rs` (modified)
- `crates/components/camel-cxf/tests/support/mock_bridge.rs` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfBridgeService.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/CxfBridgeServiceTest.java` (modified)

**Steps:**
1. Add `message SignerIdentity { string subject = 1; string issuer = 2; string serial = 3; string fingerprint_sha256 = 4; }`
   to both `cxf_bridge.proto` copies.
2. Add `optional SignerIdentity signer = 7;` to `ConsumerRequest` and change the
   `bridges/cxf` copy's `SoapRequest.timeout_ms` from `int32` to `uint64` so the
   two files are byte-identical (`diff` exits 0); the Rust copy already declares
   `uint64 timeout_ms = 7`.
3. Regenerate the Rust proto module via `crates/components/camel-cxf/build.rs`
   and the Java sources from `bridges/cxf/src/main/proto/cxf_bridge.proto`.
4. In `producer.rs`, before building the `SoapRequest`, reject
   `timeout_ms > 2147483647` with
   `CamelError::ProcessorError("cxf timeout_ms exceeds 2147483647")`; `0`
   continues to mean the configured default. A wire value whose unsigned
   representation exceeds the bound (for example `u64::MAX`, the decoded form
   of a legacy `-1`) is rejected by the same check.
5. In `CxfBridgeService.invoke` (the method the timeout tests exercise), read
   `long rawTimeout = request.getTimeoutMs();` and reject
   `rawTimeout < 0 || rawTimeout > 2147483647L` with `Status.INVALID_ARGUMENT`
   before `getDispatch`; otherwise keep
   `long normalizedTimeout = rawTimeout > 0 ? rawTimeout : bridgeConfig.connectionTimeoutMs();`.
   The timeout validation lives in `CxfBridgeService`, so its tests belong in
   `CxfBridgeServiceTest`, not `CxfClientManagerTest`.
6. Update every `ConsumerRequest` / `SoapRequest` struct literal so the workspace
   and the regenerated types compile: `crates/components/camel-cxf/src/bridge_client_test.rs`,
   `crates/components/camel-cxf/tests/consumer_emission_test.rs`,
   `crates/components/camel-cxf/tests/consumer_unit_test.rs`,
   `crates/components/camel-cxf/tests/support/mock_bridge.rs`, and the
   `consumer.rs` test module (Task 2.5 owns the field semantics).
7. Confirm `diff crates/components/camel-cxf/proto/cxf_bridge.proto bridges/cxf/src/main/proto/cxf_bridge.proto` exits 0.

**Tests:**
- `producer::tests::timeout_ms_zero_uses_default`: setup default timeout 5000;
  action normalize `timeout_ms = 0`; assert the request carries `0`. Command:
  `cargo test -p camel-component-cxf timeout_ms_zero_uses_default`. Expected:
  GREEN after.
- `producer::tests::timeout_ms_over_i32_max_is_rejected`: setup
  `timeout_ms = 2147483648`; action build the request; assert a
  `ProcessorError` naming 2147483647 and no dispatch. Command:
  `cargo test -p camel-component-cxf timeout_ms_over_i32_max_is_rejected`.
  Expected: RED before, GREEN after.
- `producer::tests::legacy_negative_timeout_is_rejected`: setup the unsigned
  value `u64::MAX` (the decode of a legacy `-1`); action build the request;
  assert a `ProcessorError`. Command:
  `cargo test -p camel-component-cxf legacy_negative_timeout_is_rejected`.
  Expected: RED before, GREEN after.
- `producer::tests::timeout_ms_in_range_is_applied`: setup `timeout_ms = 5000`;
  action build the request; assert the value reaches the `SoapRequest`. Command:
  `cargo test -p camel-component-cxf timeout_ms_in_range_is_applied`. Expected:
  RED before, GREEN after.
- `CxfBridgeServiceTest.legacyNegativeTimeoutIsRejected`: setup a `SoapRequest`
  whose decoded `timeout_ms` is negative; action `invoke`; assert
  `INVALID_ARGUMENT` before dispatch. Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.CxfBridgeServiceTest"`.
  Expected: GREEN after.
- `CxfBridgeServiceTest.timeoutAboveI32MaxIsRejected`: setup
  `timeout_ms = 2147483648`; action `invoke`; assert rejection before dispatch.
  Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.CxfBridgeServiceTest"`. Expected: GREEN after.
- `CxfBridgeServiceTest.inRangeTimeoutIsApplied`: setup `timeout_ms = 5000`;
  action `invoke`; assert the dispatch uses 5000 ms. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.CxfBridgeServiceTest"`. Expected: GREEN after.
- `CxfBridgeServiceTest.generatedSignerIdentityRoundTrips`: setup
  `SignerIdentity` with four strings; action serialize and parse; assert
  equality. Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.CxfBridgeServiceTest"`.
  Expected: GREEN after.

**Acceptance:**
- `diff crates/components/camel-cxf/proto/cxf_bridge.proto bridges/cxf/src/main/proto/cxf_bridge.proto` exits 0.
- `cargo build -p camel-component-cxf` and `bridges/cxf/gradlew -p bridges/cxf compileJava` exit 0.
- `timeout_ms` bounds `(< 0 || > 2147483647L)` are enforced before dispatch on
  both sides; `0` preserves the default; an in-range value is applied.

- [x] 2.3

#### Task 2.4: Export the signer descriptor with Body-coverage scoping

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/WssSecurityProcessor.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/WssSecurityProcessorTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/WssSecurityProcessorIntegrationTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherTest.java` (modified)
- `docs/src/components/ws-soap.md` (modified)
- `crates/components/camel-cxf/CONTEXT.md` (modified)

**Steps:**
1. Introduce `VerifiedInboundMessage` in `WssSecurityProcessor` — an exact
   result type carrying the verified XML and the optional signer descriptor:
   `record VerifiedInboundMessage(String xml, java.util.Optional<cxf_bridge.SignerIdentity> signer)`
   with accessors `xml()` and `signer()`. Change
   `public String processInbound(String soapXml)` (currently
   `WssSecurityProcessor.java:130`) to return `VerifiedInboundMessage`; the
   descriptor is RFC 2253 subject/issuer, lowercase-hex serial, and
   64-lowercase-hex-char SHA-256 of the DER certificate. When the processor
   cannot verify (`!canVerifyInbound()` or a blank message) return the input XML
   with `Optional.empty()` signer. `processOutbound` is unchanged.
2. When the profile's inbound actions include `Signature`, require a
   Body-covering signature; reject a request with no Body coverage. When
   `Signature` is not required, an unsigned request or a Body-excluded
   signature proceeds with no signer.
3. Examine at most 8 `SIGN` results. Certificates sharing one fingerprint count
   as one signer; more than one distinct Body-covering fingerprint rejects the
   request as ambiguous. Non-Body signatures never yield a signer.
4. Enforce a 64 KiB cap on the encoded certificate being hashed; reject, never
   truncate.
5. In `SoapEndpointPublisher.handleRequestBody` (the actual `ConsumerRequest`
   builder at `SoapEndpointPublisher.java:261-269`; `CxfBridgeService` is the
   producer-side `invoke` and does not build `ConsumerRequest`), replace the
   `requestXml = wssProcessor.processInbound(requestXml)` assignment at
   `:245-247` with a `VerifiedInboundMessage verified = wssProcessor.processInbound(requestXml)`,
   extract `verified.xml()` for the `extractBody` call, and call
   `consumerRequestBuilder.setSigner(verified.signer().orElse(null))` only when
   `verified.signer().isPresent()`; when no signature is present the field stays
   absent (unset). Update every `processInbound` caller — including
   `WssSecurityProcessorTest` and `WssSecurityProcessorIntegrationTest` — to the
   new return type.
6. Document the Body-coverage requirement as an intended breaking change for
   `Signature`-required profiles (timestamp-only/header-only clients are now
   rejected) in `ws-soap.md` and `crates/components/camel-cxf/CONTEXT.md`.

**Tests:**
- `WssSecurityProcessorTest.signedDescriptorHasFourLowercaseFields`: setup a
  valid Body signature; action `processInbound`; assert RFC 2253 DNs,
  lowercase-hex serial, and a 64-character lowercase fingerprint. Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.WssSecurityProcessorTest"`.
  Expected: RED before, GREEN after.
- `WssSecurityProcessorTest.distinctBodySignersRejected`: setup two
  Body-covering `SIGN` results with distinct fingerprints; action
  `processInbound`; assert rejection. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.WssSecurityProcessorTest"`. Expected: RED before,
  GREEN after.
- `WssSecurityProcessorTest.sameFingerprintIsOneSigner`: setup multiple results
  sharing one fingerprint; action process; assert accepted with one signer.
  Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.WssSecurityProcessorTest"`. Expected: GREEN after.
- `WssSecurityProcessorTest.mixedBodyAndNonBodySignaturesYieldBodySigner`:
  setup one header-only `SIGN` result plus one Body-covering `SIGN` result;
  action process; assert only the Body signer is exported and no ambiguity
  error. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.WssSecurityProcessorTest"`. Expected: RED before, GREEN after.
- `WssSecurityProcessorTest.signatureRequiredWithoutBodyCoverageRejected`:
  setup a `Signature`-required profile and a timestamp-only signature; action
  process; assert rejection. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.WssSecurityProcessorTest"`. Expected: RED before, GREEN
  after.
- `WssSecurityProcessorTest.unsignedProfileDispatchesWithoutSigner`: setup a
  profile without `Signature`; action process an unsigned request; assert
  success and no signer. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.WssSecurityProcessorTest"`. Expected: GREEN after.
- `WssSecurityProcessorTest.oversizedCertificateRejectedNotTruncated`: setup a
  certificate DER over 64 KiB; action export; assert rejection. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.WssSecurityProcessorTest"`. Expected: RED before, GREEN after.
- `SoapEndpointPublisherTest.emittedConsumerRequestCarriesSigner`: setup a
  body-covering signed request and a captured `ConsumerRequest`; action
  `handleRequestBody`; assert the emitted `ConsumerRequest` has `hasSigner()`
  true and carries the four descriptor strings, and that an unsigned request
  emits `hasSigner()` false. Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"`.
  Expected: RED before, GREEN after.

**Acceptance:**
- `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck` exits 0.
- At most 8 `SIGN` results are read; only a Body-covering certificate yields a
  signer; mixed Body/non-Body results are not ambiguous.
- The breaking change is documented.

- [x] 2.4

#### Task 2.5: Rust signer metadata as dedicated exchange properties only

**Files:**
- `crates/components/camel-cxf/src/signer.rs` (new)
- `crates/components/camel-cxf/src/lib.rs` (modified)
- `crates/components/camel-cxf/src/consumer.rs` (modified)
- `crates/components/camel-cxf/src/consumer.rs` tests module (modified)
- `crates/camel-test/tests/cxf_test.rs` (modified)

**Steps:**
1. Add `crates/components/camel-cxf/src/signer.rs` with
   `pub struct ValidatedSigner { pub subject: String, pub issuer: String, pub serial: String, pub fingerprint_sha256: String }`,
   constants `SIGNER_SUBJECT = "camel.cxf.signer.subject"`,
   `SIGNER_ISSUER = "camel.cxf.signer.issuer"`,
   `SIGNER_SERIAL = "camel.cxf.signer.serial"`,
   `SIGNER_FINGERPRINT = "camel.cxf.signer.fingerprint_sha256"`, and
   `pub fn validate_signer(signer: &crate::proto::SignerIdentity) -> Result<ValidatedSigner, CamelError>`.
2. Enforce bounds: `subject` and `issuer` each at most 4096 UTF-8 bytes;
   `serial` at most 128 bytes and lowercase hex only; `fingerprint_sha256`
   exactly 64 lowercase hex characters. On failure return `CamelError`
   classified as a **non-WSS `Receiver` fault** (never a WSS/security fault) and
   do not dispatch.
3. Change `consumer.rs::build_exchange` to return
   `Result<Exchange, CamelError>`; when `req.signer` is present, validate it,
   then write only the four `camel.cxf.signer.*` keys through
   `exchange.set_property`. On validation failure return the error before route
   dispatch, setting no signer property.
4. At the dispatch site (`consumer.rs:293`), on a `build_exchange` error send a
   correlated `ConsumerResponse` (same `request_id`) with `fault = true` and a
   `soap:Server` (`Receiver`) fault code so the Java publisher maps it to HTTP
   500, and do NOT dispatch the request to the route. The profile
   `fault_status` override SHALL NOT apply (it is not a WSS fault).
5. Update the `build_exchange` call sites and tests for the `Result` return
   (`consumer.rs` tests near lines 481, 537, 684).
6. Do not call `store_principal_properties`, do not construct a `Principal`,
   and do not write any `camel.auth.*` key. Do not set `claims`, `roles`, or
   `scopes`.
7. A client-supplied message header named `camel.cxf.signer.*` stays an
   ordinary header and never becomes an exchange property.

**Tests:**
- `signer::tests::valid_descriptor_yields_four_fields`: setup a valid
  descriptor; action `validate_signer`; assert the four values. Command:
  `cargo test -p camel-component-cxf valid_descriptor_yields_four_fields`.
  Expected: RED before, GREEN after.
- `signer::tests::oversized_subject_is_rejected`: setup `subject` of 4097
  bytes; action validate; assert error. Command:
  `cargo test -p camel-component-cxf oversized_subject_is_rejected`. Expected:
  RED before, GREEN after.
- `signer::tests::non_lowercase_serial_is_rejected` and
  `signer::tests::fingerprint_must_be_64_lowercase_hex`: setup malformed
  values; action validate; assert error. Command:
  `cargo test -p camel-component-cxf`. Expected: RED before, GREEN after.
- `consumer::tests::invalid_signer_descriptor_is_not_dispatched`: setup a
  `ConsumerRequest` with an invalid descriptor and a route that records
  dispatches; action run the consumer task; assert the route received no
  request. Command:
  `cargo test -p camel-component-cxf invalid_signer_descriptor_is_not_dispatched`.
  Expected: RED before, GREEN after.
- `consumer::tests::invalid_signer_descriptor_sets_no_properties`: setup an
  invalid descriptor; action run; assert no `camel.cxf.signer.*` property and
  no `camel.auth.*` property. Command:
  `cargo test -p camel-component-cxf invalid_signer_descriptor_sets_no_properties`.
  Expected: RED before, GREEN after.
- `consumer::tests::invalid_signer_descriptor_is_receiver_500_even_with_override`:
  setup a profile with `fault_status = 403` and an invalid descriptor; action
  run; assert the emitted `ConsumerResponse` is correlated by `request_id`,
  carries `fault = true`, and maps to `Receiver`/HTTP 500 (the override does
  not apply). Command:
  `cargo test -p camel-component-cxf invalid_signer_descriptor_is_receiver_500_even_with_override`.
  Expected: RED before, GREEN after.
- `consumer::tests::signer_is_stored_as_dedicated_properties_only`: setup a
  `ConsumerRequest` with a valid signer; action `build_exchange`; assert the
  four `camel.cxf.signer.*` properties exist and no `camel.auth.*` key and no
  principal. Command:
  `cargo test -p camel-component-cxf signer_is_stored_as_dedicated_properties_only`.
  Expected: RED before, GREEN after.
- `consumer::tests::spoofed_signer_header_stays_header`: setup a header
  `camel.cxf.signer.subject`; action `build_exchange`; assert it stays a header
  and sets no property. Command:
  `cargo test -p camel-component-cxf spoofed_signer_header_stays_header`.
  Expected: RED before, GREEN after.
- `cxf_consumer_invalid_signer_descriptor_is_receiver_500` (native
  integration-only): against the real native bridge, deliver a request whose
  exported descriptor fails Rust validation and assert the SOAP response is a
  `soap:Server` `Receiver` fault with HTTP 500 even when the profile sets a
  `fault_status` override, and that the route was not dispatched. This is the
  end-to-end HTTP 500 assertion for the descriptor fault; the Java tests cover
  WSS faults only. Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_consumer_invalid_signer_descriptor_is_receiver_500`.
  Expected: RED before, GREEN after. Requires the rebuilt native bridge.

**Acceptance:**
- `cargo test -p camel-component-cxf` exits 0.
- The production paths of `signer.rs` and `consumer.rs` contain no
  `store_principal_properties` call, no `Principal` construction, and no
  `camel.auth.*` write; a targeted `rg` over production code (excluding the
  `#[cfg(test)]` modules, whose assertions legitimately name `camel.auth.*` to
  prove absence) returns no hit. Legitimate test assertions that reference
  `camel.auth.*` are not excluded by this gate.
- `principal_from_exchange` reports no authenticated principal after assembly.
- An invalid descriptor is not dispatched and yields a correlated
  `Receiver`/500 response even under a `fault_status` override.

- [x] 2.5

#### Task 2.6: Pool-level listener TLS config and startup validation

**Files:**
- `crates/components/camel-cxf/src/config.rs` (modified)
- `crates/services/camel-bridge/src/process.rs` (modified)
- `crates/components/camel-cxf/src/pool.rs` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/BridgeConfig.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java` (modified)

**Steps:**
1. Add `soap_tls_cert_path`, `soap_tls_key_path`, `soap_tls_client_ca_path`,
   `soap_tls_pkcs12_path`, `soap_tls_pkcs12_password` to `CxfPoolConfig` (and
   `CxfPoolConfigRaw`) beside `bind_address`. Model the pair as a single
   `#[derive(Clone)] pub struct CxfListenerTls` with a manual `Debug` that
   redacts `pkcs12_password` (mirror `CxfSecurityFields`), and store
   `Option<CxfListenerTls>` on `CxfPoolConfig`, `CxfBridgePool`, and
   `BridgeSlot` so the shape is one cloneable record carried into the slot.
2. Validate in `CxfPoolConfig::validate`: exactly one key-material form
   (`cert_path` plus `key_path`, or `pkcs12_path` plus `pkcs12_password`);
   never both and never neither; `soap_tls_client_ca_path` always required when
   TLS material is present; any `soap_tls_*` with an `http` bind fails; `https`
   with no material fails.
3. Add `pub expect_soap_ready: bool` to `BridgeProcessConfig` (definition in
   `crates/services/camel-bridge/src/process.rs`, currently around line 187) and
   redact it consistently in its manual `Debug` if needed. Set
   `expect_soap_ready: false` in `BridgeProcessConfig::jms` (`:218`), `::xml`
   (`:252`), and `::cxf_profiles` (`:267`). Add the field to every direct
   struct-literal construction site: the `process.rs` test literals at the
   blocks starting near line 988 and line 1018. All constructor-based call sites
   (`crates/camel-bench/benches/xml_bridge_decompose.rs:100`,
   `crates/components/camel-jms/src/component.rs:601`,
   `crates/components/camel-xj/src/component.rs:332`,
   `crates/components/camel-xslt/src/component.rs:54`,
   `crates/components/camel-validator/src/xsd_bridge.rs:299`,
   `crates/components/camel-cxf/src/pool_env_test.rs:48`,
   `crates/camel-test/tests/cxf_test.rs:358`,
   `crates/camel-test/tests/mtls_bridge_test.rs:16,41`,
   `crates/camel-test/tests/support/jms_bridge_procs.rs:71`) remain unchanged
   and must stay compiling. This task owns the `expect_soap_ready` field, its
   defaults, and its struct literals so it compiles without Task 2.8; Task 2.8
   only adds the `soap-ready` parsing.
4. Forward the listener TLS through a **single builder used by both startup and
   restart**: change `bridge_config_with_env_trace` (currently only pushes
   `CXF_ADDRESS`, `pool.rs:39-59`) to take the `Option<&CxfListenerTls>`,
   set `expect_soap_ready = bind_address.is_some()` on the returned
   `BridgeProcessConfig`, and emit `CXF_SOAP_TLS_CERT_PATH`,
   `CXF_SOAP_TLS_KEY_PATH`, `CXF_SOAP_TLS_CLIENT_CA_PATH`,
   `CXF_SOAP_TLS_PKCS12_PATH`, `CXF_SOAP_TLS_PKCS12_PASSWORD` (password
   redacted), never `CXF_PROFILE_<NAME>_*`. Then make the health-monitor
   restart path at `pool.rs:551-561` call `bridge_config_with_env_trace` from
   the slot's persisted `listener_tls`/`bind_address` instead of duplicating
   `BridgeProcessConfig::cxf_profiles` plus a lone `CXF_ADDRESS` push; today
   that path drops all five TLS vars and the SOAP-ready expectation, so a
   restarted mutual-TLS listener silently becomes a WSS-store-less plaintext or
   unauthenticated listener.
5. Update `CxfPoolConfig::validate` bind-address handling to accept `https`
   only with material.
6. In `BridgeConfig.java` add getters for the five `CXF_SOAP_TLS_*` variables;
   re-run the same validation in `SoapEndpointPublisher.publish` before any
   bind so Java fails closed too.
7. **Literal inventory (every `CxfPoolConfig {` construction site in the
   workspace gains the new field/fields; grep-verified, not a bare claim):**
   `crates/components/camel-cxf/src/config.rs:227,735,748`;
   `crates/components/camel-cxf/src/component.rs:158,264`;
   `crates/components/camel-cxf/src/consumer.rs:458`;
   `crates/components/camel-cxf/src/pool.rs:719`;
   `crates/components/camel-cxf/src/producer.rs:142,385`;
   `crates/components/camel-cxf/tests/concurrency_test.rs:61`;
   `crates/components/camel-cxf/tests/consumer_emission_test.rs:63`;
   `crates/components/camel-cxf/tests/consumer_unit_test.rs:510`;
   `crates/components/camel-cxf/tests/pool_lifecycle_test.rs:76,101,119`;
   `crates/components/camel-cxf/tests/ws_security_test.rs:40,96`;
   `crates/camel-test/tests/cxf_test.rs:50,162,757`.
   Keep `CxfPoolConfig`'s `Default` impl (`config.rs:184`) and the
   `TryFrom<CxfPoolConfigRaw>` mapping (`config.rs:223-234`) in sync. This task
   also adds no `CxfSecurityFields` field; the `fault_status` field belongs to
   Task 2.2 and its `CxfSecurityFields { .. }` update sites are listed there.

**Tests:**
- `config::tests::listener_tls_both_forms_rejected`: setup cert/key plus
  pkcs12 pair; action validate; assert `CamelError::Config`. Command:
  `cargo test -p camel-component-cxf listener_tls_both_forms_rejected`.
  Expected: RED before, GREEN after.
- `config::tests::listener_tls_neither_form_rejected_for_https`: setup
  `bind_address = https://...` and no material; action validate; assert
  `CamelError::Config`. Command:
  `cargo test -p camel-component-cxf listener_tls_neither_form_rejected_for_https`.
  Expected: RED before, GREEN after.
- `config::tests::listener_tls_missing_client_ca_rejected`,
  `listener_tls_http_bind_rejected`, `listener_tls_pkcs12_form_accepted`,
  `listener_tls_cert_key_form_accepted`: setup each shape; action validate;
  assert the named outcome. Command:
  `cargo test -p camel-component-cxf`. Expected: RED before, GREEN after.
- `pool_env_test::listener_tls_uses_pool_env_vars_only`: setup a TLS pool;
  action build the process config; assert the five `CXF_SOAP_TLS_*` keys exist
  and no `CXF_PROFILE_*_TLS_*` is emitted. Command:
  `cargo test -p camel-component-cxf listener_tls_uses_pool_env_vars_only`.
  Expected: RED before, GREEN after.
- `pool_env_test::listener_tls_survives_restart`: setup a TLS pool, build the
  startup `BridgeProcessConfig`, then build the health-monitor restart
  `BridgeProcessConfig` through the same builder; action compare their env-var
  key sets; assert both contain the same five `CXF_SOAP_TLS_*` keys with the
  same password value (still redacted in `Debug`) and the same
  `expect_soap_ready`, proving restart does not drop listener TLS. Command:
  `cargo test -p camel-component-cxf listener_tls_survives_restart`.
  Expected: RED before, GREEN after.
- `process::tests::default_expect_soap_ready_is_false`: setup
  `BridgeProcessConfig::xml` and `::jms`; action read the field; assert false.
  This task owns the field and its defaults, so the test ships with it. Command:
  `cargo test -p camel-bridge default_expect_soap_ready_is_false`.
  Expected: RED before, GREEN after.

**Acceptance:**
- `cargo test -p camel-component-cxf` and `cargo test -p camel-bridge` exit 0.
- `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck` exits 0.
- Listener TLS validation runs both on Rust load and on Java start before any
  socket binds.
- Listener TLS and `expect_soap_ready` are persisted in the pool slot and
  re-emitted on health-monitor restart through the same builder as startup.
- `expect_soap_ready` defaults to false; only a CXF pool with a SOAP bind sets
  it true.
- Every listed `CxfPoolConfig {` literal compiles with the new field(s).

- [x] 2.6

#### Task 2.7: Mutual-TLS listener with one identity, plus `soap-ready` announcement

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfServerManager.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/PortAnnouncer.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/PortAnnouncerPortHandoffTest.java` (modified)
- `crates/camel-test/tests/cxf_test.rs` (modified)

**Steps:**
1. In `SoapEndpointPublisher.publish`, build `HttpServerOptions` with
   `setSsl(true)` and one identity serving all profiles: PEM form uses
   `PemKeyCertOptions` plus `PemTrustOptions`; PKCS12 form uses `PfxOptions`.
   Always `setClientAuth(ClientAuth.REQUIRED)` validated against
   `soap_tls_client_ca_path`. Never reuse WSS message-layer stores. Read the
   actual bound port from the `HttpServer` listen result (`actualPort()`) and
   expose it via `OptionalInt soapPort()`; `publish()` is idempotent.
2. Keep plaintext `http` binding available (no TLS) as the interim
   ingress-termination path.
3. **Move publication to startup, before readiness — no observer dependency.**
   Today `publish()` runs only from `CxfServerManager.setConsumerRequestObserver`
   (`CxfServerManager.java:106-109`), i.e. on the consumer gRPC stream that
   `BridgeProcess::start_and_connect` opens only AFTER `BridgeProcess::start`
   returns. Because `BridgeProcess::start` must now observe `soap-ready`
   (Task 2.8), publication-after-connect is a circular wait. Change
   `CxfServerManager.setConsumerRequestObserver` to only store the observer
   (do not call `endpointPublisher.publish()`), and publish the configured
   listener from a Quarkus startup path: add
   `void onStart(@Observes io.quarkus.runtime.StartupEvent ev)` to
   `SoapEndpointPublisher` (or call `publish()` from `CxfServerManager`'s own
   startup observer) that publishes synchronously when
   `bridgeConfig.address()` is present, before the ready/soap-ready
   announcement. The CDI graph keeps `CxfServerManager <-> SoapEndpointPublisher`
   as-is (field injection), so no new constructor cycle and no new gRPC RPC is
   introduced; requests arriving before the consumer stream take the non-fault
   `503` transport-availability response from step 4, so routes are unavailable
   — not hung — until `OpenConsumerStream` is called.
4. Add `boolean hasConsumerObserver()` to `CxfServerManager` (true only when a
   consumer-request observer/stream is connected) and make
   `SoapEndpointPublisher.handleRequestBody` check `hasConsumerObserver()` **
   FIRST — BEFORE any WS-Security processing and BEFORE parsing or classifying
   the SOAP body**. When it is false, the publisher SHALL return **HTTP 503**
   with a `Retry-After` header and a plaintext body and emit **no SOAP fault
   envelope**: this is a transport-availability response, not a SOAP fault, so
   it does not enter the fault-class matrix (which maps `Sender` to 400 and
   `Receiver` to 500). The gate SHALL be unconditional and version-independent:
   a `Signature`-required profile whose signature is missing or invalid SHALL
   still return the pre-stream 503, never a 400/500 security fault, and the
   configured body byte/transport limits SHALL remain enforced. Do NOT defer the
   check to a late `CxfServerManager.handleSoapRequest` exception and do NOT map
   it through the generic exception path to 500. A stream that becomes
   unavailable after the check SHALL yield the same HTTP 503
   transport-availability response.
5. In `PortAnnouncer`, after the SOAP listener binds (i.e. after the startup
   `publish()` completes), print exactly one JSON line
   `{"status":"soap-ready","port":<N>}` on stdout and flush, where `<N>` is the
   actual bound port (`SoapEndpointPublisher.soapPort()`). Because the two
   `StartupEvent` observers have no guaranteed order, the announcer SHALL wait
   (bounded, same as the existing `announceWhenBound` poll) for
   `SoapEndpointPublisher.soapPort()` to be present before printing, and SHALL
   fail loudly rather than print an unverified port. Emit the SOAP line only
   when a SOAP bind is configured; when no bind is configured, skip `publish()`
   and the line entirely. Keep the gRPC `{"status":"ready","port":<N>}` line
   unchanged and distinct; the two lines are emitted at startup without any
   `OpenConsumerStream` RPC.

**Tests:**
- `SoapEndpointPublisherTest.certKeyFormBindsWithClientAuth`: setup PEM
  cert/key/client-CA and `https`; action publish; assert client authentication
  is required. Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"`.
  Expected: RED before, GREEN after.
- `SoapEndpointPublisherTest.pkcs12FormBinds`: setup PKCS12 store plus client
  CA; action publish; assert the listener binds. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"`. Expected:
  RED before, GREEN after.
- `SoapEndpointPublisherTest.listenerTlsIndependentOfWssStores`: setup a
  WSS-only profile and no pool TLS with `https`; action publish; assert
  fail-closed and no WSS-store reuse. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"`. Expected: RED before,
  GREEN after.
- `PortAnnouncerPortHandoffTest.emitsSoapReadyLine`: setup a bound SOAP
  listener published at startup, with **no** consumer observer/stream set;
  action read stdout; assert `ANNOUNCEMENTS` contains
  `{"status":"soap-ready","port":<N>}` with the real port, proving the line is
  emitted without an `OpenConsumerStream` RPC. Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.PortAnnouncerPortHandoffTest"`.
  Expected: RED before, GREEN after.
- `SoapEndpointPublisherTest.publishesAtStartupWithoutConsumerStream`: setup a
  configured `https` listener and no consumer observer; action fire the Quarkus
  `StartupEvent`; assert `publish()` bound the listener, `soapPort()` is the
  real bound port, and `CxfServerManager.handleSoapRequest` before the stream
  returns the non-fault `503` transport-availability response rather than
  hanging. Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"`.
  Expected: RED before, GREEN after.
- `SoapEndpointPublisherTest.preStream503IsTransportAvailabilityNotFault`: setup
  a configured listener with no consumer stream and issue an actual POST before
  any consumer RPC; action `handleSoapRequest`/publish; assert the response is
  HTTP 503 with a `Retry-After` header and a plaintext body containing no
  `Envelope` or SOAP fault element, and NOT a generic 500. Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"`.
  Expected: RED before, GREEN after.
- `SoapEndpointPublisherTest.availabilityCheckedBeforeWssForSignatureProfile`:
  setup a `Signature`-required profile, no consumer stream, and a POST whose
  signature is missing or invalid; action `handleRequestBody`; assert the
  response is the pre-stream HTTP 503 with `Retry-After` and a plaintext body,
  NOT a 400/500 security fault, that the WSS processor was never invoked, and
  that the configured body byte/transport limits were applied. Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"`.
  Expected: RED before, GREEN after.
- `SoapEndpointPublisherTest.lateStreamLossYieldsSame503`: setup a request that
  passes the availability check, then drop the consumer stream before dispatch;
  action respond; assert the response is the same HTTP 503 transport-availability
  response with `Retry-After` and no fault envelope, not a generic 500. Command:
  `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"`.
  Expected: RED before, GREEN after.
- `cxf_consumer_mutual_tls_with_no_client_cert_is_rejected`: setup a mutual-TLS
  listener; action connect without a client certificate; assert the handshake
  is rejected before any SOAP processing. Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_consumer_mutual_tls_with_no_client_cert_is_rejected`.
  Expected: RED before, GREEN after. Requires the rebuilt native bridge.

**Acceptance:**
- `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck` exits 0.
- A `http` bind still starts without TLS.
- The announced SOAP port is the real bound port; tests connect using it
  without a bind/drop probe.
- The configured listener is published at startup with no
  `OpenConsumerStream` RPC; pre-stream SOAP requests take the non-fault
  `503` transport-availability response (`Retry-After`, no SOAP fault envelope,
  not a generic 500), so `soap-ready` is observable from `BridgeProcess::start`
  without connecting.

- [x] 2.7

#### Task 2.8: Dual-ready `BridgeProcess` parsing and optional SOAP port

**Files:**
- `crates/services/camel-bridge/src/process.rs` (modified)
- `crates/camel-test/tests/cxf_test.rs` (modified)

**Steps:**
1. In `BridgeProcess::start`, parse both `{"status":"soap-ready","port":N}` and
   `{"status":"ready","port":N}` in either order within the single
   `bridge_start_timeout_ms`; a malformed `soap-ready` line returns
   `BridgeError::BadReadyMessage`; when `expect_soap_ready` is false, do not
   require a `soap-ready` line. Readiness SHALL be bounded by the same
   timeout for both lines (no second timeout). This is now deadlock-free
   because Task 2.7 publishes the configured SOAP listener from the Quarkus
   `StartupEvent` at process startup — before the ready/soap-ready lines and
   with no dependence on the consumer stream — so `start` observes
   `soap-ready` without calling `connect()`/`OpenConsumerStream`. Requests that
   arrive before the consumer stream open take the non-fault `503`
   transport-availability response from Task 2.7 (`routes unavailable`, never a
   hang). The `expect_soap_ready` field, its defaults, and its struct literals
   are owned by Task 2.6; this task only consumes the field. No `Command:`-only
   shortcut: every case below keeps its own explicit GIVEN/WHEN/THEN and
   command.
2. Add `soap_port: Option<u16>` to `BridgeProcess` and
   `pub fn soap_port(&self) -> Option<u16>`.
3. Add integration coverage for reordered, partial-then-complete, malformed,
   and unbound cases, a native startup-lifecycle case proving an
   `expect_soap_ready = true` CXF pool starts and consumes `soap-ready` before
   any `OpenConsumerStream` RPC, and a mutual-TLS consumer case that connects
   using the announced SOAP port without a bind/drop probe, including a
   health-monitor restart that must keep the client CA and announce the new
   actual port.

**Tests:**
- `process::tests::dual_ready_reordered_is_accepted`: setup a fake bridge that
  prints `soap-ready` before `ready`; action `BridgeProcess::start`; assert both
  ports are stored. Command:
  `cargo test -p camel-bridge dual_ready_reordered_is_accepted`. Expected: RED
  before, GREEN after.
- `process::tests::partial_ready_is_bounded_and_completes_in_one_timeout`:
  setup a fake bridge that prints `soap-ready` then delays `ready`; action
  start; assert both lines are consumed within one `bridge_start_timeout_ms`
  and no second timeout is armed. Command:
  `cargo test -p camel-bridge partial_ready_is_bounded_and_completes_in_one_timeout`.
  Expected: RED before, GREEN after.
- `process::tests::malformed_soap_ready_fails_startup`: setup a `soap-ready`
  line with a non-numeric port; action start; assert
  `BridgeError::BadReadyMessage`. Command:
  `cargo test -p camel-bridge malformed_soap_ready_fails_startup`. Expected:
  RED before, GREEN after.
- `process::tests::no_soap_wait_when_unbound`: setup `expect_soap_ready = false`;
  action start; assert it awaits only `ready`. Command:
  `cargo test -p camel-bridge no_soap_wait_when_unbound`. Expected: RED before,
  GREEN after.
- `cxf_consumer_mutual_tls_uses_announced_port`: setup a mutual-TLS listener
  with `expect_soap_ready = true`; action connect with a CA-signed client
  certificate; assert the handshake succeeds and the request is processed with
  no bind/drop probe. Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_consumer_mutual_tls_uses_announced_port`.
  Expected: RED before, GREEN after.
- `cxf_consumer_expect_soap_ready_starts_without_consumer_rpc`: setup a native
  CXF bridge with `expect_soap_ready = true`; action `BridgeProcess::start`
  only (no `connect()`/`OpenConsumerStream`); assert startup returns and
  `soap_port()` is the announced actual port, proving the startup publication
  from Task 2.7 makes `expect_soap_ready` work without RPC. Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_consumer_expect_soap_ready_starts_without_consumer_rpc`.
  Expected: RED before, GREEN after.
- `cxf_consumer_mutual_tls_restart_preserves_ca_and_port`: setup a mutual-TLS
  listener with pool listener TLS, complete one request, trigger a
  health-monitor restart, then reconnect with the same CA-signed client; assert
  the restarted listener still requires the client CA (a wrong-CA or
  cert-less client is rejected) and announces the new actual
  `soap-ready` port that the test connects to with no bind/drop probe.
  Command:
  `cargo test -p camel-test --features integration-tests --test cxf_test cxf_consumer_mutual_tls_restart_preserves_ca_and_port`.
  Expected: RED before, GREEN after.
- `mtls_bridge_test` existing cases: setup the existing mTLS bridge tests that
  use `BridgeProcessConfig::xml`; action compile and run them with the new
  field present; assert the constructor-based callers still build and pass.
  Command:
  `cargo test -p camel-test --features integration-tests --test mtls_bridge_test`.
  Expected: GREEN baseline; must stay green.

**Acceptance:**
- `cargo test -p camel-bridge` and
  `cargo test -p camel-test --features integration-tests --test cxf_test`
  exit 0.
- Both ready lines are accepted in either order within
  `bridge_start_timeout_ms`; a partial sequence still completes inside the one
  timeout.
- `BridgeProcess::start` observes `soap-ready` with no consumer RPC (startup
  publication), and a health-monitor restart preserves the listener client CA
  and announces the new actual port.

- [x] 2.8

## Phase 3: Schema set (F)

**Subdeliverables (ordered):** confined discovery newtypes, schema-set proto
and registration wiring, aliases and cache and replay, Java resolver and caps,
import/replay integration, clean native builds and docs, release gate.

### camel-component-validator + bridges/xml + camel-component-xslt + camel-test

#### Task 3.1: Confined virtual-root discovery with `quick-xml`

**Files:**
- `crates/components/camel-validator/Cargo.toml` (modified)
- `crates/components/camel-validator/src/schema_set.rs` (new)
- `crates/components/camel-validator/src/lib.rs` (modified)

**Steps:**
1. Add `quick-xml = { workspace = true }` to
   `crates/components/camel-validator/Cargo.toml` (the workspace already
   declares `quick-xml = "0.42"` in `/Cargo.toml`).
2. Add `crates/components/camel-validator/src/schema_set.rs` with the precise
   discovery result type and a stable cache key:
   ```rust
   #[derive(Debug, Clone)]
   pub struct SchemaSet {
       /// Root file's normalized relative basename; the entry schema's systemId.
       pub root_system_id: String,
       /// Transmitted keys to schema bytes, sorted for determinism: every
       /// distinct root-relative virtual lookup key, plus a redundant entry
       /// keyed by each supplied alias URI carrying the same bytes (wire
       /// compatibility). Two keys MAY share one canonical file (for example a
       /// symlink loop back to the root), and each key counts toward the caps.
       pub resources: BTreeMap<String, Vec<u8>>,
       /// Supplied absolute-URI alias -> normalized root-relative virtual path.
       /// Transmitted as the authoritative aliased-schema virtual base (the
       /// redundant resources alias-URI entry supplies bytes only); retained so
       /// replay re-supplies the exact map and the cache key covers it.
       pub aliases: BTreeMap<String, String>,
   }

   impl SchemaSet {
       /// Cache key: root_system_id + sorted resource keys + per-entry bytes
       /// digest + sorted alias entries. Any changed child or alias rotates it.
       pub fn cache_key(&self) -> String { /* sha256 hex, "xsd-<hex>" */ }
       pub fn root_bytes(&self) -> &[u8] { /* resources[&self.root_system_id] */ }
   }
   ```
   plus constants `MAX_SCHEMA_RESOURCES = 256`, `MAX_SCHEMA_BYTES = 8 * 1024 * 1024`,
   `MAX_SCHEMA_ALIASES = 128`, `MAX_IMPORT_DEPTH = 32`, and
   `pub fn discover_schema_set(root: &Path, aliases: &BTreeMap<String, String>) -> Result<SchemaSet, ValidatorError>`.
   `discover_schema_set` materializes each supplied alias into a redundant
   `resources` entry keyed by its absolute URI (bytes for wire compatibility)
   AND retains the `aliases` map (absolute URI -> normalized relative virtual
   path); the alias map, not the URI key, supplies the aliased schema's virtual
   base.
3. Compute `canonical_root = canonicalize(root.parent())` and canonicalize
   every candidate: allow files whose canonical target is inside
   `canonical_root` (symlinks included) and reject files that escape it. Keep
   two distinct structures: a byte/reference cache keyed by canonical target
   (so a canonical file's bytes and parsed refs are read once) and an `expanded`
   set keyed by the pair `(canonical target, normalized virtual path)` that
   decides expansion. A canonical file reached through two virtual keys with
   different base directories SHALL be expanded once per key, because each
   key's relative `xs:import`/`xs:include` targets resolve against its own base:
   symlinks `a/shared.xsd` and `b/shared.xsd` over one canonical file that
   includes `child.xsd` SHALL expand both `a/child.xsd` and `b/child.xsd`.
   Retain the transmitted `resources` map keyed by every distinct root-relative
   virtual lookup key that reaches a file, because the Java resolver serves
   supplied keys only; dropping a virtual key would make the importing schema
   unresolvable. Revisiting an already-expanded `(canonical target, virtual
   path)` pair SHALL skip re-expansion, so a symlink cycle terminates while
   still transmitting every distinct key.
4. Start from the root file and follow only reachable `xs:import`/`xs:include`
   targets with a `quick_xml::Reader`; never walk the directory tree. Depth
   SHALL count new `(canonical target, virtual path)` expansion states on one
   traversal path (maximum 32). A canonical file's bytes/refs are cached once
   but expanded per distinct virtual path/base; every distinct reachable virtual
   lookup key is transmitted. Because symlinks can produce unbounded
   virtual-key growth, the 256-key/128-alias/8 MiB caps SHALL be enforced
   incrementally during expansion and SHALL fail closed with a bounded error.
5. Set `root_system_id` to the root file's normalized relative basename. A
   single root file with no imports yields a one-entry set. Enforce the
   256-entry and 8 MiB caps over **all** transmitted keys and their byte values,
   with bounded errors naming the cap. A supplied alias and a relative path that
   denote the same file are two entries, each counted; likewise every distinct
   virtual key that reaches one canonical file is its own entry and is counted.
6. Reject conflicting provided definitions: the same normalized virtual path
   supplied with different bytes is an error; repeated reach of the same path
   is not.
7. Register the module in `lib.rs`.

**Tests:**
- `schema_set::tests::reachable_imports_are_included`: setup root importing
  child including grandchild; action discover; assert three normalized relative
  entries. Command:
  `cargo test -p camel-component-validator reachable_imports_are_included`.
  Expected: RED before, GREEN after.
- `schema_set::tests::unrelated_files_are_excluded`: setup an unreferenced
  sibling; action discover; assert it is absent. Command: `cargo test -p camel-component-validator unrelated_files_are_excluded`. Expected:
  RED before, GREEN after.
- `schema_set::tests::diamond_included_once_without_duplicate_error`: setup
  root importing B and C each importing D; action discover; assert D appears
  once. Command: `cargo test -p camel-component-validator diamond_included_once_without_duplicate_error`. Expected: RED before, GREEN after.
- `schema_set::tests::cyclic_include_terminates`: setup A includes B and B
  includes A; action discover; assert termination with no error. Command: `cargo test -p camel-component-validator cyclic_include_terminates`. Expected: RED before, GREEN after.
- `schema_set::tests::per_virtual_base_expands_distinct_children`: setup a root
  that `xs:include`s BOTH links `a/shared.xsd` and `b/shared.xsd` (both links,
  not a second same-namespace `xs:import`, which Xerces silently ignores), where
  both resolve to one canonical `shared.xsd` that declares no global component
  and includes an EMPTY `child.xsd` relative to its own directory (empty so no
  duplicate component/namespace declaration when both bases are included);
  action discover; assert the canonical shared file's bytes are read once and
  `resources` contains both `a/shared.xsd`/`b/shared.xsd` AND both
  `a/child.xsd`/`b/child.xsd` (a canonical-only visited key would drop one
  base's child). Command:
  `cargo test -p camel-component-validator per_virtual_base_expands_distinct_children`.
  Expected: RED before, GREEN after.
- `schema_set::tests::unbounded_symlink_alias_growth_fails_closed`: setup a
  symlink-alias chain whose distinct virtual keys would exceed the 256-entry
  cap; action discover; assert a bounded error naming the cap (fail closed),
  not unbounded growth or a hang. Command:
  `cargo test -p camel-component-validator unbounded_symlink_alias_growth_fails_closed`.
  Expected: RED before, GREEN after.
- `schema_set::tests::symlinked_cycle_preserves_loop_key`: setup a root schema
  that declares exactly one global element and `xs:include`s an EMPTY `loop.xsd`;
  `loop.xsd` `xs:include`s `loop-link.xsd`, a symlink back to the empty
  `loop.xsd` (NOT to the root, so the root's global element declaration is not
  duplicated), using `xs:include` (NOT a same-namespace `xs:import`, which is
  invalid for a same-namespace self target); action discover; assert termination
  with no depth error and no duplicate-global error, the canonical empty file's
  bytes read once, expansion keyed by the `(canonical target, virtual path)`
  pair, and BOTH loop virtual keys present in `resources` so the
  supplied-keys-only resolver can serve the include. Command:
  `cargo test -p camel-component-validator symlinked_cycle_preserves_loop_key`.
  Expected: RED before, GREEN after.
- `schema_set::tests::depth_limit_is_enforced`: setup a chain deeper than 32;
  action discover; assert a bounded error. Command: `cargo test -p camel-component-validator depth_limit_is_enforced`. Expected: RED
  before, GREEN after.
- `schema_set::tests::in_root_symlink_is_accepted` and
  `schema_set::tests::escaping_symlink_is_rejected`: setup symlinks whose
  canonical target is inside and outside `canonical_root`; action discover;
  assert include and reject. Command: `cargo test -p camel-component-validator in_root_symlink_is_accepted`. Expected: RED before, GREEN
  after.
- `schema_set::tests::traversal_outside_root_is_rejected` and
  `schema_set::tests::single_file_without_imports_is_one_entry`: setup each
  case; action discover; assert rejection and the one-entry set. Command: `cargo test -p camel-component-validator traversal_outside_root_is_rejected`. Expected: RED before, GREEN after.
- `schema_set::tests::more_than_256_entries_is_rejected`,
  `schema_set::tests::more_than_8_mib_is_rejected`, and
  `schema_set::tests::conflicting_definitions_are_rejected`: setup each; action
  build; assert bounded errors naming the cap. Command: `cargo test -p camel-component-validator more_than_256_entries_is_rejected`. Expected: RED
  before, GREEN after.

**Acceptance:**
- `cargo test -p camel-component-validator` exits 0.
- `crates/components/camel-validator/Cargo.toml` declares
  `quick-xml = { workspace = true }`.
- Discovery performs no whole-directory walk; each canonical file's bytes are
  cached once and expanded once per `(canonical target, normalized virtual
  path)` pair while every distinct reachable virtual lookup key is transmitted;
  a symlink-aliased cycle terminates and preserves every loop key.

- [x] 3.1

#### Task 3.2: `resources` + `aliases` + root systemId proto, alias config, and registration wiring

**Files:**
- `crates/components/camel-validator/proto/xml_bridge.proto` (modified)
- `crates/components/camel-xslt/proto/xml_bridge.proto` (modified)
- `bridges/xml/src/main/proto/xml_bridge.proto` (modified)
- `crates/components/camel-validator/src/config.rs` (modified)
- `crates/components/camel-validator/src/xsd_bridge.rs` (modified)
- `crates/components/camel-validator/src/compiled.rs` (modified)
- `crates/components/camel-validator/src/metadata.rs` (modified)
- `crates/components/camel-validator/src/component.rs` (modified)
- `crates/components/camel-validator/src/bridge_client_test.rs` (modified)
- `crates/components/camel-xslt/src/bridge_client_test.rs` (modified)
- `crates/components/camel-xslt/src/client.rs` (modified)

**Steps:**
1. In all three `xml_bridge.proto` files (camel-validator, camel-xslt,
   bridges/xml) replace `RegisterSchemaRequest` with: `string schema_id = 1;`
   `bytes schema = 2;` `string root_system_id = 3;` `map<string, bytes>
   resources = 4;` `map<string, string> aliases = 5;` — where `aliases` maps each
   supplied absolute-URI alias to its normalized root-relative virtual path —
   keeping the three files byte-identical.
2. Regenerate the Rust proto modules for `camel-component-validator` and
   `camel-xslt` (build scripts `crates/components/camel-validator/build.rs`,
   `crates/components/camel-xslt/build.rs`) and the Java sources for
   `bridges/xml`.
3. Add `pub schema_aliases: BTreeMap<String, String>` to `ValidatorConfig`
   (absolute URI to root-relative virtual path; default empty) and define its
   ONLY operator surface in `ValidatorConfig::from_uri` (`config.rs:44-145`).
   Accept one optional `schemaAliases` query parameter whose value is a
   percent-encoded JSON object, for example
   `validator:schemas/order.xsd?schemaAliases=%7B%22http%3A%2F%2Fexample.com%2Fp.xsd%22%3A%22a%2Fparent.xsd%22%7D`
   (decodes once to `{"http://example.com/p.xsd":"a/parent.xsd"}`). While
   iterating the `&`-split query pairs, reject a second `schemaAliases=` pair
   with `CamelError::InvalidUri` naming the option. Reuse
   `validate_percent_encoding` on the raw option value before decoding; report
   `CamelError::InvalidUri` naming `schemaAliases`, not the schema path.
   Then call `percent_decode_str(value).decode_utf8()` EXACTLY ONCE and
   `serde_json::from_str::<BTreeMap<String, String>>(decoded)`. Reject malformed
   percent-encoding, a non-object JSON body, a non-string value, and more
   than `MAX_SCHEMA_ALIASES` (128) entries, each with a bounded error naming the
   option. Do NOT decode twice. These errors SHALL surface during
   `create_endpoint` (no bridge startup), never deferred to first `validate()`.
   Precedent for the JSON-object encoding: `queueArguments` in
   `crates/components/camel-rabbitmq/src/config.rs:424-431`; `serde_json` is
   already a `camel-validator` dependency, so add none.
   Extend `ValidatorMetadataDescriptor` (`metadata.rs:16-34`) with
   `#[uri_param(name = "schemaAliases")] pub _schema_aliases: Option<String>`
   and add `"schemaAliases"` to the `validator_metadata_uri_options_parity`
   expected list, so parser keys, metadata, and docs agree.
4. **Discovery owner is `compiled.rs`, before backend registration — the backend
   never rediscovers and never reads the filesystem.** `ValidatorConfig` carries
   `schema_path` and the new `schema_aliases`; `CompiledValidator::compile`
   (which owns the config) calls
   `discover_schema_set(&config.schema_path, &config.schema_aliases)` once at
   compile time and retains the result. The `XsdBridge` trait
   (`xsd_bridge.rs:81-84`) becomes
   `async fn register(&self, schema_id: &str, set: &crate::schema_set::SchemaSet) -> Result<SchemaId, ValidatorError>`
   (`validate` is unchanged), where `schema_id` is `set.cache_key()` computed by
   the caller. Add ONE conversion helper in `xsd_bridge.rs`; the generated prost
   map fields are `HashMap` while `SchemaSet` keeps `BTreeMap`, so the explicit
   iteration is the only conversion:
   ```rust
   impl RegisterSchemaRequest {
       pub(crate) fn new(schema_id: SchemaId, set: &crate::schema_set::SchemaSet) -> Self {
           Self {
               schema_id,
               schema: set.root_bytes().to_vec(),
               root_system_id: set.root_system_id.clone(),
               resources: set.resources.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
               aliases: set.aliases.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
           }
       }
   }
   ```
   `XsdBridgeBackend::register` (`:412`) builds the request as
   `RegisterSchemaRequest::new(schema_id, set)` from the passed set — it does NOT
   call `discover_schema_set`, does NOT read any path, and never clones a
   `BTreeMap` straight into a prost field. Do NOT change the build scripts (no
   prost `btree_map` option): `SchemaSet` keeps the internal `BTreeMap`
   (deterministically sorted) and the helper converts only at the wire boundary.
   The backend has no root path today,
   and passing one in would turn the trait into a filesystem API and re-introduce
   the "backend rediscovers unknown roots" contradiction; discovery stays in
   `compiled.rs`. `schema` stays populated with the root entry bytes for wire
   compatibility.
5. Update the full surface that the signature change touches, in one pass:
   - `crates/components/camel-validator/src/compiled.rs`: `compile_xsd`
     (`:84-97`) computes `Arc<SchemaSet>` once via `discover_schema_set` and
     stores `schema_set: Arc<SchemaSet>` in `CompiledValidator::Xml` (replacing
     `xsd_bytes`); `validate_xml` (`:194-220`) calls
     `backend.register(set.cache_key().as_str(), set).await`; the test mock at
     `:298` implements the new signature.
   - `crates/components/camel-validator/src/component.rs`: the `XsdBridge` mock
     at `:244` implements the new signature.
   - `crates/components/camel-validator/src/xsd_bridge.rs`:
     `XsdBridgeBackend.schemas` (`:134`) becomes
     `Arc<DashMap<SchemaId, Arc<SchemaSet>>>` (retain the whole set, not raw
     root bytes); `register_with_channel` (`:332`) takes `&SchemaSet` and builds
     the request from it; the reconnect replay loop in `on_reconnect` (`:354`)
      re-registers each retained `Arc<SchemaSet>` in full — every virtual path,
      the root systemId, and the `aliases` map — never rediscovering; `for_test`
      (`:180`) and the `MockRpc` (`:496`) track the new fields/type.
   - `crates/components/camel-validator/src/bridge_client_test.rs` and
     `crates/components/camel-xslt/src/bridge_client_test.rs`: mocks round-trip
     the new fields; the XSLT initializer in
     `crates/components/camel-xslt/src/client.rs` stays on the stylesheet RPCs
     and compiles against the regenerated module.
6. Confirm the three proto copies are byte-identical.
7. Update the only other `ValidatorConfig` struct literals so the crate
   compiles: the `Ok(ValidatorConfig { .. })` in `config.rs:136` gains
   `schema_aliases`, and the literal in the
   `compiled.rs::tests::xsd_bridge_register_error_propagates_on_validate` test
   (`compiled.rs:320`) gains `schema_aliases: BTreeMap::new()`. No other crate
   constructs `ValidatorConfig`.

**Tests:**
- `config::tests::schema_aliases_json_parsed`: setup the URI
  `validator:schemas/order.xsd?schemaAliases=%7B%22http%3A%2F%2Fexample.com%2Fp.xsd%22%3A%22a%2Fparent.xsd%22%7D`;
  action `ValidatorConfig::from_uri`; assert `schema_aliases` equals the one
  entry `"http://example.com/p.xsd" -> "a/parent.xsd"`. Command:
  `cargo test -p camel-component-validator schema_aliases_json_parsed`.
  Expected: RED before, GREEN after.
- `config::tests::schema_aliases_percent_decoded_exactly_once`: setup a
  double-encoded value (`%257B%2522urn%253Aa%2522%253A%2522a.xsd%2522%257D`);
  action `from_uri`; assert `Err(CamelError::InvalidUri)` because one decode
  yields `%7B%22urn%3Aa%22%3A%22a.xsd%22%7D`, not JSON.
  Command:
  `cargo test -p camel-component-validator schema_aliases_percent_decoded_exactly_once`.
  Expected: RED before, GREEN after.
- `config::tests::schema_aliases_duplicate_param_errors`: setup a query with two
  `schemaAliases=` pairs; action `from_uri`; assert `Err(InvalidUri)`. Command:
  `cargo test -p camel-component-validator schema_aliases_duplicate_param_errors`.
  Expected: RED before, GREEN after.
- `config::tests::schema_aliases_malformed_json_errors`: setup
  `schemaAliases=not-json`, `schemaAliases=%5B%5D` (an array), and the encoded object
  `schemaAliases=%7B%22urn%3Aa%22%3A1%7D` (non-string value);
  action `from_uri`; assert `Err(InvalidUri)` for all three. Command:
  `cargo test -p camel-component-validator schema_aliases_malformed_json_errors`.
  Expected: RED before, GREEN after.
- `config::tests::schema_aliases_malformed_percent_errors`: setup
  `validator:schemas/order.xsd?schemaAliases=%7B%22urn%3Aa%22%3A%22%ZZ%22%7D`;
  action `ValidatorConfig::from_uri`; assert `Err(CamelError::InvalidUri)` naming
  `schemaAliases` before decoding or bridge startup. Command:
  `cargo test -p camel-component-validator schema_aliases_malformed_percent_errors`.
  Expected: RED before, GREEN after.
- `config::tests::schema_aliases_cap_enforced_at_parse`: setup a JSON object with
  129 entries; action `from_uri`; assert `Err(InvalidUri)` naming the 128-entry
  cap. Command:
  `cargo test -p camel-component-validator schema_aliases_cap_enforced_at_parse`.
  Expected: RED before, GREEN after.
- `component::tests::create_endpoint_rejects_malformed_alias_uri`: setup
  a temp directory with a valid root XSD and URI
  `validator:<temp-root-path>?schemaAliases=%7Bbad`;
  action `ValidatorComponent::new().create_endpoint(uri, &NoOpComponentContext)`;
  assert `matches!(err, CamelError::InvalidUri(m) if m.contains("schemaAliases"))`
  and that no bridge process started (failure is at URI parse, not file access).
  Command:
  `cargo test -p camel-component-validator create_endpoint_rejects_malformed_alias_uri`.
  Expected: RED before, GREEN after.
- `component::tests::create_endpoint_accepts_alias_uri`: setup a temp root XSD
  file plus a valid `a/parent.xsd` under its parent directory, and URI
  `validator:<path>?schemaAliases=%7B%22urn%3Aalias%22%3A%22a%2Fparent.xsd%22%7D`;
  action `ValidatorComponent::new().create_endpoint`; assert `Ok`. Command:
  `cargo test -p camel-component-validator create_endpoint_accepts_alias_uri`.
  Expected: GREEN before and after (regression guard); alias preservation is
  separately asserted by `schema_aliases_json_parsed` and the native URI test.
- `metadata::tests::validator_metadata_uri_options_parity`: setup the descriptor;
  action read `uri_options`; assert the names include `schemaAliases` alongside
  the existing six. Command:
  `cargo test -p camel-component-validator validator_metadata_uri_options_parity`.
  Expected: RED before, GREEN after.
- `xsd_bridge::tests::register_request_carries_root_system_id_and_resources`:
  setup a two-entry `SchemaSet`; action `register(set.cache_key().as_str(), &set)`;
  assert the captured `RegisterSchemaRequest` carries `schema_id`,
  `root_system_id`, and the `resources` map contents. Command:
  `cargo test -p camel-component-validator register_request_carries_root_system_id_and_resources`.
  Expected: RED before, GREEN after.
- `xsd_bridge::tests::register_request_carries_alias_map`: setup a `SchemaSet`
  with one supplied alias (absolute URI → `a/parent.xsd`) materialized into a
  redundant `resources` entry; action `register(set.cache_key().as_str(), &set)`;
  assert the captured `RegisterSchemaRequest.aliases` equals the set's alias map
  AND the redundant `resources` alias-URI entry is still present. Command:
  `cargo test -p camel-component-validator register_request_carries_alias_map`.
  Expected: RED before, GREEN after.
- `xsd_bridge::tests::register_preserves_schema_aliases`: setup a set with a
  supplied alias materialized into `resources`; action register then reconnect;
  assert the replayed request still carries the `aliases` map and the alias
  entry (and the full set). Command:
  `cargo test -p camel-component-validator register_preserves_schema_aliases`.
  Expected: RED before, GREEN after.
- `xsd_bridge::tests::register_does_not_read_filesystem`: setup a backend whose
  `register` is handed a fully-discovered `SchemaSet` (no `schema_path` is
  available to it); action `register`; assert it builds the request purely from
  the passed set and performs no discovery/filesystem access (the discovery
  owner is `compiled.rs`). Command:
  `cargo test -p camel-component-validator register_does_not_read_filesystem`.
  Expected: RED before, GREEN after.
- `component::tests::xsd_mock_compiles_against_new_trait_signature`: setup the
  `XsdBridge` mock in `component.rs`; action build the crate test target; assert
  the mock implements the new `register(&self, &str, &SchemaSet)` signature.
  Command: `cargo test -p camel-component-validator`. Expected: GREEN after.
- `bridge_client_test` in `camel-xslt`: setup a stylesheet compile request;
  action round-trip; assert the regenerated types compile and match. Command:
  `cargo test -p camel-xslt`. Expected: GREEN after.

**Acceptance:**
- `diff crates/components/camel-validator/proto/xml_bridge.proto crates/components/camel-xslt/proto/xml_bridge.proto`
  exits 0 and the same diff against
  `bridges/xml/src/main/proto/xml_bridge.proto` exits 0.
- `cargo build -p camel-component-validator -p camel-xslt` exits 0.
- Discovery owns the filesystem read and runs in `compiled.rs` before
  registration; the backend consumes only the passed `SchemaSet` and reads
  nothing.
- Every `register` caller, impl, mock, and replay path in the inventory above
  compiles against `register(&self, &str, &SchemaSet)`; the alias map is
  preserved through storage and replay.
- `ValidatorConfig::from_uri` is the sole operator surface for `schema_aliases`;
  a malformed, duplicated, or over-128 `schemaAliases` URI parameter fails
  `create_endpoint` before any bridge startup, and `uri_options` lists
  `schemaAliases`.
- No prost map field is assigned from a `BTreeMap` directly: every
  `RegisterSchemaRequest` is built by `RegisterSchemaRequest::new`.

- [x] 3.2

#### Task 3.3: Alias cap, schema-set cache key, and full-set replay

**Files:**
- `crates/components/camel-validator/src/config.rs` (modified)
- `crates/components/camel-validator/src/xsd_bridge.rs` (modified)

**Steps:**
1. Enforce that `schema_aliases` has at most 128 entries (`MAX_SCHEMA_ALIASES`)
   with a bounded error naming the cap. The primary enforcement is in the
   `from_uri` parse owned by Task 3.2 (so a 129-entry `schemaAliases` URI fails
   `create_endpoint`); keep a `ValidatorConfig`-level check as defense so a
   programmatic constructor cannot bypass the cap.
2. Replace `XsdBridgeBackend::schema_id_for(xsd_bytes)` (`xsd_bridge.rs:209-213`)
   with `SchemaSet::cache_key()` computed by the discovery owner
   (`compiled.rs`) and passed to `register`; the key is derived from the root
   systemId, the sorted virtual/alias keys, each entry's bytes digest, and the
   alias map. A changed child or alias yields a new key.
3. On reconnect, re-register the full retained set from the backend's
   `Arc<DashMap<SchemaId, Arc<SchemaSet>>>` storage (every virtual path,
   including children and aliases, plus the root systemId) rather than the
   single root blob — no rediscovery. Each replayed request is built by
   `RegisterSchemaRequest::new(schema_id, set)` (the same conversion helper as
   first registration), never by an inline struct literal.
4. Keep `RegisterSchemaRequest.schema` populated with the root entry bytes for
   wire compatibility while `resources` carries the whole set.

**Tests:**
- `component::tests::create_endpoint_rejects_alias_cap`: setup a `validator:`
  URI whose percent-encoded `schemaAliases` JSON object has 129 entries; action
  `ValidatorComponent::new().create_endpoint`; assert `Err` naming the 128-alias
  cap before any bridge startup (never a direct-config-only construction).
  Command:
  `cargo test -p camel-component-validator create_endpoint_rejects_alias_cap`.
  Expected: GREEN before and after Task 3.3 (regression guard for the parse-time
  cap implemented by Task 3.2).
- `xsd_bridge::tests::changed_child_yields_new_cache_key`: setup a
  `SchemaSet`, discover, then change a child's bytes; action compute
  `SchemaSet::cache_key()`; assert it differs and `register` uses the new id.
  Command:
  `cargo test -p camel-component-validator changed_child_yields_new_cache_key`.
  Expected: RED before, GREEN after.
- `xsd_bridge::tests::cache_key_is_stable_for_unchanged_set`: setup a
  `SchemaSet`; action compute `SchemaSet::cache_key()` twice; assert it is
  identical. Command:
  `cargo test -p camel-component-validator cache_key_is_stable_for_unchanged_set`.
  Expected: RED before, GREEN after.
- `xsd_bridge::tests::reconnect_reseeds_every_virtual_path`: setup a two-entry
  `SchemaSet` with an alias; action trigger reconnect; assert the backend
  re-registers the retained `Arc<SchemaSet>` with both resources and the alias
  (no rediscovery). Command:
  `cargo test -p camel-component-validator reconnect_reseeds_every_virtual_path`.
  Expected: RED before, GREEN after.
- `xsd_bridge::tests::root_without_imports_registers_one_entry`: setup a root
  `SchemaSet` with no imports; action register; assert one resource entry and
  the root systemId. Command: `cargo test -p camel-component-validator root_without_imports_registers_one_entry`. Expected: RED before, GREEN after.

**Acceptance:**
- `cargo test -p camel-component-validator` exits 0.
- The cache key changes when any child's bytes change and is stable otherwise.
- Replay re-registers every virtual path plus the alias map.
- Both first registration and replay build through `RegisterSchemaRequest::new`;
  the alias cap is exercised through the URI surface, not a direct literal.

- [x] 3.3

#### Task 3.4: Java resolver serves supplied keys only and enforces decoded caps

**Files:**
- `bridges/xml/src/main/java/org/rustcamel/xmlbridge/XsdValidatorService.java` (modified)
- `bridges/xml/src/main/java/org/rustcamel/xmlbridge/SchemaCache.java` (modified)
- `bridges/xml/src/test/java/org/rustcamel/xmlbridge/XsdValidationIntegrationTest.java` (modified)

**Steps:**
1. In `XsdValidatorService.registerSchema`, read `root_system_id`, the decoded
   `resources` map, and the decoded `aliases` map. Enforce before compiling:
   decoded `resources` count at most 256, the sum of decoded byte values at most
   8 MiB, decoded `aliases` entries at most 128, and `resources` entries whose
   key is an alias URI at most 128 (two aligned caps, enforced independently);
   reject with a bounded error naming the cap and compile nothing. Validate every
   alias key is an absolute URI and every alias value is a normalized
   root-relative virtual path present in `resources`; reject an alias whose value
   is absolute, missing from `resources`, or whose normalized form escapes the
   virtual root. A `..` segment that normalizes still in-root is accepted.
2. Do not attempt duplicate-key detection: proto map semantics overwrite
   duplicates before decode, so a duplicate key cannot be represented on the
   wire and post-decode detection is impossible. Decoded absolute-URI entries in
   `resources` count toward the resources cap and are limited to 128; decoded
   `aliases` entries are limited to 128 by their own aligned cap. An alias and a
   relative path for the same file are separate entries, each counting toward 256
   and 8 MiB. Count and byte caps are enforced independently.
3. Add a `LSResourceResolver` that resolves only from the supplied decoded
   keys, in this precedence order: (a) when the importing schema has a virtual
   base path, base-relative normalization runs first — the relative systemId is
   resolved against that base to a normalized root-relative virtual path, and
   that key wins over an exact raw-systemId match (nested `a/parent.xsd`
   targeting `child.xsd` resolves `a/child.xsd`, never a root-level `child.xsd`);
   (b) an absolute URI (an explicit `http`/`https`/`file` scheme) resolves by
   exact match on the `aliases` map key only when supplied; set the resolved
   schema's `LSInput` `systemId` to the mapped normalized root-relative virtual
   path and leave `baseURI` UNSET (setting both applies the base twice, for
   example `a/a/child.xsd`), so its own relative targets re-expand from the
   process anchor with a single base; (c) a relative systemId resolves by exact
   raw key only when there is no importing-schema virtual base — the
   distinguished root resolved reference, which SHALL NOT shadow a base-relative
   key.
   Pass the root schema as a source whose `systemId` is the transmitted root
   systemId and whose `baseURI` is UNSET; capture the expanded root URI Xerces
   derives through Java relative-to-absolute normalization and compute the anchor
   root directory from it. On each resolver call, take the importing schema's
   expanded `file:` base: validate it is under the computed anchor root (a base
   outside it is rejected), strip the anchor prefix to the importer's normalized
   root-relative virtual key, then normalize the relative systemId against that
   key's directory. Normalize `..` before the key lookup so an in-root `..`
   resolves and only an escaping `..` is rejected. Treat a scheme-carrying
   systemId as an absolute alias lookup, never as a relative path. Keep the
   mapping deterministic and isolated per registration (no process-global anchor
   leaks between registrations); an explicit virtual anchor URI may replace the
   process anchor only if it yields the identical mapping.
4. Reject a missing key, an alias key that is not absolute, an alias value whose
   normalized form escapes the root, an unsupplied network URL, a base outside
   the anchor, a `..` that escapes the anchor root, and any DTD reference.
   Perform no network and no filesystem read. Keep external access disabled and
   the XXE posture unchanged.
5. Key `SchemaCache` by the schema-set digest so a changed child rotates the
   entry.

**Tests:**
- `XsdValidationIntegrationTest.moreThan256DecodedEntriesRejected`: setup a
  decoded request with 257 resources; action register; assert bounded failure
  and no compile. Command:
  `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`.
  Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.moreThan8MiBDecodedRejected`: setup decoded
  bytes summing over 8 MiB; action register; assert bounded failure. Command: `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`. Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.resourceAliasUrisOver128Rejected`: setup a
  decoded request with 129 `resources` entries keyed by absolute alias URIs;
  action register; assert bounded failure naming the resource-alias cap.
  Command: `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`.
  Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.aliasMapOver128Rejected`: setup a decoded request
  with 129 `aliases` map entries; action register; assert bounded failure naming
  the alias-map cap. Command: `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`.
  Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.aliasKeyNotAbsoluteUriRejected`: setup a decoded
  alias map whose key is a relative path; action register; assert bounded failure
  and no fetch. Command: `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`.
  Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.aliasValueAbsoluteRejected`: setup a decoded
  alias map whose value is an absolute URI; action register; assert bounded
  failure and no fetch. Command: `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`.
  Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.aliasValueMissingResourceRejected`: setup a
  decoded alias map whose value is a normalized relative path absent from
  `resources`; action register; assert bounded failure and no fetch. Command: `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`.
  Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.aliasValueTraversalRejected`: setup a decoded
  alias map whose value contains a `..` segment that normalizes outside the root;
  action register; assert bounded failure and no fetch. Command:
  `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`.
  Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.aliasValueInRootTraversalResolves`: setup a
  decoded alias map whose value contains a `..` segment that normalizes to an
  in-root key present in `resources`; action register then compile; assert the
  alias resolves from the normalized key and no fetch occurs. Command:
  `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`.
  Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.aliasAndRelativeSameFileCountSeparately`: setup
  an alias plus a relative path to the same file; action register; assert both
  count toward the 256-entry and 8 MiB caps. Command: `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`. Expected: RED
  before, GREEN after.
- `XsdValidationIntegrationTest.countCapAndByteCapAreIndependent`: setup a
  request under the byte cap but over the count cap and one over the byte cap
  but under the count cap; action register; assert each is rejected by its own
  cap. Command: `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`. Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.duplicateMapKeyIsLastWinsNoError`: setup a wire
  request whose duplicate key is overwritten before decode; action register;
  assert no duplicate-key error is raised and the decoded map is capped.
  Command: `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`. Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.relativeImportResolvesFromSuppliedSet`: setup a
  supplied relative child; action compile; assert resolution from in-memory
  bytes. Command: `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`. Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.baseRelativeBeatsExactRootRawKey`: setup a
  decoded request that registers a root-level `child.xsd` and a nested
  `a/parent.xsd` whose relative `child.xsd` normalizes to `a/child.xsd`, both
  natively valid with observably different constraints and no target-namespace
  component collision; action register then compile `a/parent.xsd`; assert
  resolution uses `a/child.xsd` (an exact-raw-key-first resolver picks the root
  `child.xsd` instead). Command: `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`. Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.aliasResolvesWithVirtualBase`: setup a supplied
  absolute-URI alias mapped to `a/parent.xsd`, where `a/parent.xsd` contains a
  relative `xs:include` of `child.xsd`, and a conflicting root-level `child.xsd`,
  both natively valid with observably different constraints and no
  target-namespace component collision; action register then compile the alias;
  assert the nested `a/child.xsd` resolves from the mapped virtual base (a
  URL-based `LSInput` base resolves against the URI and diverges) and no fetch
  occurs. Command: `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`.
  Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.nestedImporterAndAliasDoNotDoubleBase`: setup a
  root schema importing a nested `nested/child.xsd` plus a supplied absolute-URI
  alias mapped to `a/parent.xsd`, where `a/parent.xsd` imports `a/child.xsd`, with
  a conflicting root-level `child.xsd`, all natively valid; action register then
  compile the alias; assert the nested `a/child.xsd` resolves from the single
  mapped base and never `a/a/child.xsd`, `nested/a/child.xsd`, or the root child,
  proving the root source and the returned `LSInput` set only `systemId` with
  `baseURI` unset. Command:
  `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`.
  Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.suppliedAliasResolvesWithoutFetch`: setup a
  supplied absolute-URI alias; action compile; assert resolution and no fetch.
  Command: `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`. Expected: RED before, GREEN after.
- `XsdValidationIntegrationTest.unsuppliedUrlRefused`,
  `escapingTraversalAndDtdRefused`, and `inRootTraversalNormalizesResolves`:
  setup each; action compile; assert an unsupplied URL, an escaping `..`, and a
  DTD are refused with no fetch, while an in-root `..` that normalizes to a
  supplied key resolves. Command:
  `bridges/xml/gradlew -p bridges/xml test --tests "org.rustcamel.xmlbridge.XsdValidationIntegrationTest"`. Expected: RED before, GREEN after.

**Acceptance:**
- `bridges/xml/gradlew -p bridges/xml test spotlessCheck` exits 0.
- No filesystem or network access occurs during resolution.
- No duplicate-key detection is attempted; the decoded count and byte caps are
  enforced independently.

- [x] 3.4

#### Task 3.5: Schema-set integration tests (imports, replay, aliases)

**Files:**
- `crates/camel-test/tests/xml_bridge_validator_test.rs` (modified)
- `crates/camel-test/tests/support/xml_bridge.rs` (modified)

**Steps:**
1. Add fixture schemas under the test temp directory: root importing a child
   that includes a grandchild; a diamond; a cycle; a deep chain; an in-root
   symlink and an escaping symlink; an unrelated sibling; and a **two-base
   shared-target** fixture — one physical `shared.xsd` (NO global component
   declarations) mounted as two in-root symlinks `a/shared.xsd` and `b/shared.xsd`,
   where the root `xs:include`s BOTH links (both links, not a second
   same-namespace `xs:import`, which Xerces silently ignores) and `shared.xsd`
   includes an **empty** `child.xsd` (empty so the native XSD stays free of
   duplicate component/namespace declarations when both `a/child.xsd` and
   `b/child.xsd` are included).
2. Add integration tests that register through the real `xml-bridge` and
   validate a document, exercising import/include resolution end to end.
3. Add alias cases that supply the alias through the real operator surface:
   build the `validator:<root>` endpoint URI with a percent-encoded
   `schemaAliases` JSON parameter (for example
   `?schemaAliases=%7B%22http%3A%2F%2Fexample.com%2Fp.xsd%22%3A%22a%2Fparent.xsd%22%7D`).
   A supplied alias resolves without a fetch; an unsupplied `http`/`https`/`file`
   systemId is refused. Do NOT configure the alias by a direct
   `ValidatorConfig`/`schema_aliases` literal.
4. Add a changed-child case asserting rotation to a new cache key and
   validation against the new set.
5. Add a restart-replay case asserting the full set is re-registered, including
   children and aliases.
6. Add a **natively valid** symlink-cycle fixture: the root declares exactly one
   global element and `xs:include`s an EMPTY `loop.xsd`; `loop.xsd` `xs:include`s
   `loop-link.xsd`, a symlink back to the empty `loop.xsd` (NOT to the root, so
   the root's global element declaration is not duplicated). Use `xs:include`
   (NOT `xs:import`, which is invalid for a same-namespace self target). Register
   it through the real `xml-bridge` and validate a document; assert the compile
   succeeds with no duplicate-global error, every loop virtual key resolves from
   the transmitted set with no fetch, and the entry caps count every transmitted
   virtual key. Do not assert native rejection of a self-`xs:import` fixture;
   that variant is discovery-only and out of scope here.
7. Add a two-base native regression: register the two-base shared-target fixture
   through the real `xml-bridge` and validate; assert the compile succeeds and
   both `a/child.xsd` and `b/child.xsd` are present in the transmitted
   `resources` (a canonical-only visited key would drop one base's child).
8. Add a base-relative precedence native regression: a root-level `child.xsd`
   and a nested `a/parent.xsd` whose relative `child.xsd` normalizes to
   `a/child.xsd`. The two `child.xsd` schemas SHALL be natively valid and impose
   observably different constraints, with distinct component names in the target
   namespace so the fixture has no duplicate component/namespace collision. The
   unrelated root-level `child.xsd` is transmitted in `resources` (registered)
   but SHALL NOT be the schema compiled for `a/parent.xsd`. Register through the
   real `xml-bridge` and validate a document that satisfies only the nested
   constraint; assert the compile resolves `a/child.xsd` (an
   exact-raw-key-first resolver would pick the root child and diverge).
9. Add an absolute-URI alias virtual-base native regression: the endpoint URI's
   percent-encoded `schemaAliases` JSON parameter maps an absolute URI to
   `a/parent.xsd`, which contains a relative `xs:include` of `child.xsd`; a
   conflicting root-level `child.xsd` is also transmitted in `resources`. Both children are natively valid with observably
   different constraints and no target-namespace component collision. Register
   through the real `xml-bridge` and validate a document that satisfies only the
   nested constraint; assert the alias resolves `a/child.xsd` via its mapped
   virtual base (a URL-based `LSInput` base resolves against the absolute URI and
   diverges).
10. Add a nested-importer + alias single-base native regression: a root schema
    imports a nested `nested/child.xsd` and the endpoint URI's `schemaAliases`
    parameter supplies an absolute-URI alias mapped to `a/parent.xsd`, where
    `a/parent.xsd` imports `a/child.xsd`, plus a conflicting root-level
    `child.xsd`. Register through the real `xml-bridge` and
    validate; assert the alias resolves `a/child.xsd` from the single mapped base,
    never `a/a/child.xsd`, `nested/a/child.xsd`, or the root child (proving the
    root source and returned `LSInput` set only `systemId` with `baseURI` unset).
11. Add an in-root traversal native regression and an escaping-traversal/Dtd
    refusal: an `xs:import` systemId containing a `..` that normalizes to a
    supplied in-root key resolves; an import whose `..` escapes the anchor root or
    a DTD reference is refused, with no fetch.

**Tests:**
- `xml_bridge_validator_imports_include_chain_resolves`: setup root, child,
  grandchild; action register and validate; assert success. Command:
  `cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test xml_bridge_validator_imports_include_chain_resolves`.
  Expected: RED before, GREEN after.
- `xml_bridge_validator_diamond_and_cycle_terminate`: setup diamond and cycle
  fixtures; action register; assert success with no duplicate error and no
  depth loop. Command: `cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test xml_bridge_validator_diamond_and_cycle_terminate`. Expected: RED before, GREEN after.
- `xml_bridge_validator_symlink_and_traversal_rejected`: setup in-root and
  escaping symlink fixtures; action register; assert in-root accepted and
  escaping refused. Command: `cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test xml_bridge_validator_symlink_and_traversal_rejected`. Expected: RED before, GREEN after.
- `xml_bridge_validator_supplied_alias_resolves_and_unsupplied_url_refused`:
  setup an alias map; action register; assert resolution and refusal. Command: `cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test xml_bridge_validator_supplied_alias_resolves_and_unsupplied_url_refused`. Expected: RED before, GREEN after.
- `xml_bridge_validator_changed_child_rotates_and_replays`: setup a set; action
  change a child then restart; assert a new key and full replay re-registration
  including aliases. Command: `cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test xml_bridge_validator_changed_child_rotates_and_replays`. Expected: RED before, GREEN after.
- `xml_bridge_validator_symlink_cycle_compiles_all_keys`: setup a natively valid
  cycle — the root declares one global element and `xs:include`s an EMPTY
  `loop.xsd`, which `xs:include`s `loop-link.xsd`, a symlink back to the empty
  `loop.xsd` (never to the root, so the root global element is not duplicated);
  action register through the native xml bridge and validate; assert the compile
  succeeds with no duplicate-global error, every loop key is served from the
  transmitted map with no fetch, and the entry count includes every virtual key.
  Command:
  `cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test xml_bridge_validator_symlink_cycle_compiles_all_keys`.
  Expected: RED before, GREEN after.
- `xml_bridge_validator_two_bases_expand_distinct_children`: setup one physical
  `shared.xsd` with no global component, mounted as `a/shared.xsd` and
  `b/shared.xsd` symlinks, included by the root via `xs:include` on BOTH links
  (not a same-namespace `xs:import`) and including an empty `child.xsd`; action
  register through the native xml bridge and validate; assert the compile
  succeeds and both `a/child.xsd` and `b/child.xsd` are present in the
  transmitted `resources`. Command:
  `cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test xml_bridge_validator_two_bases_expand_distinct_children`.
  Expected: RED before, GREEN after.
- `xml_bridge_validator_base_relative_beats_exact_root_key`: setup a root-level
  `child.xsd` and a nested `a/parent.xsd` whose relative `child.xsd` normalizes
  to `a/child.xsd`; both children are natively valid with observably different
  constraints and no target-namespace component collision, and the unrelated
  root `child.xsd` is registered but is not the schema compiled for
  `a/parent.xsd`. Action register through the native xml bridge and validate a
  document that satisfies only the nested constraint; assert the compile and
  validation reflect `a/child.xsd` (a raw-exact-first resolver picks the root
  child and diverges). Command:
  `cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test xml_bridge_validator_base_relative_beats_exact_root_key`.
  Expected: RED before, GREEN after.
- `xml_bridge_validator_alias_uri_end_to_end`: setup a `validator:<root>` URI
  whose percent-encoded `schemaAliases` JSON parameter maps an absolute URI to
  `a/parent.xsd` (containing a relative `xs:include` of `child.xsd`) plus a
  conflicting root-level `child.xsd`, both natively valid with observably
  different constraints. Action create the endpoint through
  `ValidatorComponent::create_endpoint` and register through the native xml
  bridge, then validate a document satisfying only the nested constraint; assert
  the alias was parsed from the URI and `a/child.xsd` resolves from the mapped
  virtual base (never the root-level `child.xsd`) with no fetch. Command:
  `cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test xml_bridge_validator_alias_uri_end_to_end`.
  Expected: RED before, GREEN after.
- `xml_bridge_validator_alias_resolves_virtual_base`: setup the alias through the
  endpoint URI's percent-encoded `schemaAliases` parameter (not a direct
  config literal), mapped to `a/parent.xsd` (containing a relative `xs:include`
  of `child.xsd`) plus a conflicting root-level `child.xsd`; both natively valid
  with observably different constraints. Action register through the native xml
  bridge and validate a document satisfying only the nested constraint; assert
  `a/child.xsd` resolves from the alias's mapped virtual base (not the absolute
  URI) and no fetch occurs. Command:
  `cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test xml_bridge_validator_alias_resolves_virtual_base`.
  Expected: RED before, GREEN after.
- `xml_bridge_validator_nested_importer_alias_single_base`: setup a root importing
  a nested `nested/child.xsd` and a supplied absolute-URI alias mapped to
  `a/parent.xsd` (importing `a/child.xsd`), plus a conflicting root-level
  `child.xsd`, all natively valid. Action register through the native xml bridge
  and validate; assert the alias resolves `a/child.xsd` from the single mapped
  base and never `a/a/child.xsd`, `nested/a/child.xsd`, or the root child (the
  root source and returned `LSInput` set only `systemId`, with `baseURI` unset).
  Command:
  `cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test xml_bridge_validator_nested_importer_alias_single_base`.
  Expected: RED before, GREEN after.
- `xml_bridge_validator_in_root_traversal_normalizes`: setup an `xs:import`
  systemId containing a `..` that normalizes to a supplied in-root key, plus an
  import whose `..` escapes the anchor root and a DTD reference. Action register
  through the native xml bridge and validate; assert the in-root `..` resolves
  with no fetch while the escaping `..` and the DTD are refused. Command:
  `cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test xml_bridge_validator_in_root_traversal_normalizes`.
  Expected: RED before, GREEN after.

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test`
  exits 0 against the native xml bridge binary built by
  `cargo xtask build-xml-bridge`.
- The symlink-cycle case compiles end to end through the real xml bridge and
  the transmitted set preserves the loop virtual key.
- The base-relative precedence regression compiles end to end through the real
  xml bridge and `a/parent.xsd` resolves the nested `a/child.xsd`, not the
  root-level `child.xsd`.
- The alias end-to-end test supplies the alias only through the operator
  `schemaAliases` URI parameter and resolves the nested `a/child.xsd`, never the
  conflicting root-level `child.xsd`.

- [x] 3.5

#### Task 3.6: Clean native builds, schema check, and schema-set docs

**Files:**
- `docs/src/components/validator.md` (modified)
- `crates/components/camel-validator/README.md` (modified)
- `crates/components/camel-validator/CONTEXT.md` (modified)

**Steps:**
1. Run `cargo xtask build-xml-bridge` and `cargo xtask build-cxf-bridge` to
   confirm clean native builds after the proto and Java changes; resolve any
   native reflection registration gaps in
   `bridges/*/src/main/java/**/NativeImageReflectionRegistrations.java`.
2. Run `cargo xtask schema --check`. Adding a `uri_param` does not change the
   `ComponentMetadata` shape, so `schemas/component-metadata.json` stays
   byte-identical; regenerate with `cargo xtask schema` only if the check reports
   drift, and leave no regenerated file uncommitted.
3. Run `cargo xtask lint-non-exhaustive`; if this change introduces a public
   contract enum it carries `#[non_exhaustive]` per ADR-0049, otherwise no new
   enum is added.
4. Document the exact operator syntax in `validator.md` and
   `crates/components/camel-validator/README.md`, with the URI grammar and the
   option table gaining a `schemaAliases` row:
   ```text
   validator:<schema-path>[?...&schemaAliases=<percent-encoded JSON object>]
   ```
   and the worked example
   `schemaAliases=%7B%22http%3A%2F%2Fexample.com%2Fp.xsd%22%3A%22a%2Fparent.xsd%22%7D`
   (decodes once to `{"http://example.com/p.xsd":"a/parent.xsd"}`), stating that
   the value is a JSON object percent-encoded once, that malformed, duplicated,
   or over-128 `schemaAliases` values fail endpoint creation before the bridge
   starts, and that paths are root-relative virtual paths. Record the
   reachable-import discovery and the 256-entry, 8 MiB, 128-alias, and depth-32
   bounds and the confined-root rule.
5. Record the discovery/resolver split (Rust reads confined; Java never
   fetches) in `crates/components/camel-validator/CONTEXT.md`.

**Tests:**
- No new unit test; this task is a build and docs gate. Command:
  `cargo xtask build-xml-bridge`, `cargo xtask build-cxf-bridge`,
  `cargo xtask schema --check`, `cargo xtask lint-non-exhaustive`. Expected:
  all exit 0.

**Acceptance:**
- `cargo xtask build-xml-bridge` and `cargo xtask build-cxf-bridge` exit 0.
- `cargo xtask schema --check` exits 0 with no regenerated files left
  uncommitted.
- `cargo xtask lint-non-exhaustive` exits 0.
- `validator.md`, `crates/components/camel-validator/README.md`, and
  `crates/components/camel-validator/CONTEXT.md` state the caps, the
  confined-root rule, the no-fetch Java resolver, and the exact `schemaAliases`
  URI syntax with a decodable example.
- `crates/components/camel-validator/src/metadata.rs`'s `uri_options` and the
  docs table list the same seven options (`schemaAliases` included), and
  `schemas/component-metadata.json` is unchanged.

- [x] 3.6

#### Task 3.7: Release pin/tag gate (blocked on the master plan-bless ruling)

**Files:**
- `bridges/cxf/build.gradle.kts` (modified; only under an explicit ruling)
- `bridges/xml/build.gradle.kts` (modified; only under an explicit ruling)
- `crates/components/camel-cxf/src/lib.rs` (modified; `BRIDGE_VERSION`, only under an explicit ruling)
- `crates/components/camel-validator/src/lib.rs` (modified; `BRIDGE_VERSION`, only under an explicit ruling)
- `crates/components/camel-xslt/src/lib.rs` (modified; `BRIDGE_VERSION`, only under an explicit ruling)

**Steps:**
1. Prerequisite: the master plan-bless must rule on the final cxf and xml
   bridge release versions and tags (the pending item in
   `design.md ## Open decisions`). Do not invent a version and do not edit any
   `version`/`BRIDGE_VERSION` pin before that ruling exists. The current value is
   `0.7.0` in all three Rust crates.
2. If no ruling exists at apply time: leave both `bridges/cxf/build.gradle.kts`
   and `bridges/xml/build.gradle.kts` `version` values and all three Rust
   `BRIDGE_VERSION` constants unchanged and keep the two bridges independent
   (no JMS linkage). This is the expected outcome until the ruling lands.
3. If the ruling names exact cxf and xml versions: set only those exact values
   in the two `build.gradle.kts` files and in the matching Rust
   `BRIDGE_VERSION` constants (`crates/components/camel-cxf/src/lib.rs` for cxf,
   `crates/components/camel-validator/src/lib.rs` and
   `crates/components/camel-xslt/src/lib.rs` for xml), independently, so each
   bridge's Rust version equals its Gradle version. Cite the ruling in the issue
   tracker. Never invent a version.
4. Do not edit `design.md`, `specs/`, or `proposal.md`; those are blessed
   artifacts and their hash must not drift.
5. Do not create a git tag. Tag creation is a human action.

**Tests:**
- No new unit test. Verification commands:
  `git diff -- bridges/cxf/build.gradle.kts bridges/xml/build.gradle.kts crates/components/camel-cxf/src/lib.rs crates/components/camel-validator/src/lib.rs crates/components/camel-xslt/src/lib.rs`
  (inspect the `version`/`BRIDGE_VERSION` lines), and `git status --short`.
  Expected: without a ruling, no version line changes; with a ruling, exactly the
  named values change and each Rust `BRIDGE_VERSION` matches its Gradle
  `version`.

**Acceptance:**
- No bridge `version`/`BRIDGE_VERSION` pin changes without a master plan-bless
  ruling naming the exact version.
- Each bridge's Rust `BRIDGE_VERSION` equals its Gradle `version`.
- No git tag is created and no `git push`/`git fetch` runs.
- The blessed artifacts are byte-unchanged by this task.

- [x] 3.7

## Requirement coverage matrix

Every blessed requirement and scenario maps to an owning task with at least one
named executable test. Spec keys: A `cxf-producer-transport`,
B `cxf-soap-envelope`, C `cxf-fault-status`, D `cxf-signer-identity`,
E `cxf-listener-tls`, F `xsd-schema-set`.

### A `cxf-producer-transport`

| Requirement | Scenario owners |
| --- | --- |
| Producer transport TLS is a nested, all-or-nothing profile config | Configured client certificate is presented: 1.6, 1.7. Store type defaults to PKCS12: 1.5. Absent tls block uses JVM defaults: 1.5, 1.6. Truststore-only enables server authentication without a client cert: 1.5, 1.7. Incomplete store pair fails startup: 1.5. Wrong store password fails Java startup: 1.5, 1.6. Cross-profile isolation: 1.5, 1.7. |
| Hostname verification is retained on the producer conduit | Wrong-SAN host is rejected: 1.6, 1.7. |

### B `cxf-soap-envelope`

| Requirement | Scenario owners |
| --- | --- |
| SOAP version derives from the selected WSDL port binding | SOAP 1.1 binding emits a SOAP 1.1 request: 1.2, 1.3. SOAP 1.2 binding emits a SOAP 1.2 request: 1.3. Both versions complete a success roundtrip and capture the wire: 1.1, 1.3. soap-version header is ignored: 1.2, 1.4. Unrecognized binding fails explicitly: 1.2, 1.3, 1.4. |
| PAYLOAD dispatch emits exactly one SOAP envelope | Wire request has a single envelope and a direct operation child: 1.1, 1.2, 1.4. A payload element is not nested in a duplicate envelope: 1.1, 1.2. |
| A full SOAP Envelope payload is rejected before network | Pre-wrapped payload fails fast: 1.2, 1.4. |
| Producer timeout is a reconciled nonnegative uint64 | Zero preserves the existing default: 2.3. Oversized or legacy-negative timeout is rejected: 2.3. In-range timeout is honored: 2.3. |

### C `cxf-fault-status`

| Requirement | Scenario owners |
| --- | --- |
| SOAP version is detected before WS-Security processing | SOAP 1.2 root detected before WSS: 2.1. SOAP 1.1 root detected before WSS: 2.1. application/soap+xml without a readable root selects 1.2: 2.1. text/xml or no content type without a readable root defaults to 1.1: 2.1. |
| Fault class and HTTP status follow a fixed matrix | Security failure is a Sender fault: 2.2. Malformed envelope is a Sender fault: 2.2. Missing Body is a Sender fault: 2.2. Invalid signer descriptor is a Receiver fault: 2.2, 2.5. Route error and timeout are Receiver faults: 2.2. Rust soap:Client maps to Sender and soap:Server to Receiver: 2.2. SOAP 1.1 always returns 500: 2.2. |
| Fault envelopes are version-matched and namespace-correct | SOAP 1.1 fault prefix is declared: 2.1. SOAP 1.2 fault uses the 1.2 structure: 2.1. |
| Per-profile fault-status override applies to security faults only | Override applies to its profile's WSS faults on SOAP 1.2: 2.2. Override applies to its profile's WSS faults on SOAP 1.1: 2.2. Override does not affect non-security Receiver faults: 2.2. Override does not affect non-WSS Sender faults: 2.2. Out-of-range override is rejected at startup: 2.2. |
| Consumer-stream availability is gated before security and body processing | Request before the consumer stream returns 503 without a fault: 2.7. Availability is checked before WS-Security (`Signature` profile): 2.7. Late stream loss yields the same 503: 2.7. |

### D `cxf-signer-identity`

| Requirement | Scenario owners |
| --- | --- |
| Signer identity is exchanged as bounded string fields only | Signed request carries the four string fields: 2.4. Oversized certificate is rejected, not truncated: 2.4. Unsigned request yields unknown signer: 2.4. Mirrored proto copies are byte-identical: 2.3. |
| An invalid signer descriptor is a non-WSS Receiver fault and is not dispatched | Oversized subject is not dispatched: 2.5. Malformed or non-lowercase fingerprint is not dispatched: 2.5. Non-lowercase serial is not dispatched: 2.5. End-to-end soap:Server HTTP 500 native assertion: 2.5. |
| Signer metadata is written to dedicated exchange properties only | Signer is stored as dedicated properties: 2.5. No Principal is constructed: 2.5. Spoofed signer header stays a header: 2.5. |
| Signer extraction is scoped to profiles requiring a Signature | Profile without Signature action allows an unsigned request: 2.4. Signature-required profile rejects no Body coverage: 2.4. Same certificate in multiple results is one signer: 2.4. Distinct Body signers are rejected: 2.4. Non-Body signature cannot yield a signer: 2.4. |
| Body signature coverage is a breaking change for Signature-required profiles | Previously passing timestamp-only client is rejected: 2.4. |

### E `cxf-listener-tls`

| Requirement | Scenario owners |
| --- | --- |
| Listener TLS is a pool-level, validated shape | Cert/key form is accepted: 2.6, 2.7. PKCS12 form is accepted: 2.6, 2.7. Implicit plaintext is rejected: 2.6. Both forms or neither is rejected: 2.6. Missing client CA is rejected: 2.6. TLS with http bind address is rejected: 2.6. TLS keys travel as pool env vars: 2.6. One listener identity serves all profiles: 2.7. |
| Listener requires client authentication and fails closed | Mutual TLS request succeeds: 2.7, 2.8. Missing or wrong client certificate is rejected: 2.7. Listener TLS material is independent of WSS stores: 2.7. Plaintext http remains available: 2.7. |
| The actual bound SOAP port is announced and consumed | Bound port is announced: 2.7. Either ready order is accepted: 2.8. Malformed soap-ready line fails startup: 2.8. No soap wait absent a bound listener: 2.8. |
| SOAP-ready expectation is an explicit BridgeProcessConfig opt-in | Default false waits only for ready: 2.6, 2.8. CXF with a bind opts in: 2.6, 2.8. Other bridges are unchanged: 2.6, 2.8. |

### F `xsd-schema-set`

| Requirement | Scenario owners |
| --- | --- |
| Schema set is transmitted as virtual paths plus a root systemId | Parent and child schemas register together: 3.2, 3.5. Mirrored proto copies stay synchronized: 3.2. Alias map is transmitted beside resources: 3.2, 3.5. |
| Supplied aliases reach the component through the validator URI | Alias query is parsed at endpoint creation: 3.2. Percent-encoded alias value is decoded exactly once: 3.2. Malformed alias query fails endpoint creation: 3.2. Duplicate alias query parameter is rejected: 3.2. Alias cap is enforced at URI parse: 3.2, 3.3. |
| Schema-set numeric limits are enforced in Rust and Java | Rust discovery rejects more than 256 entries: 3.1. Java registration rejects more than 256 decoded entries: 3.4. Rust discovery rejects more than 8 MiB total: 3.1. Java registration rejects more than 8 MiB decoded total: 3.4. Alias cap is enforced in Rust config: 3.2, 3.3. Resource alias-URI count cap is enforced in Java: 3.4. Alias mapping cap is enforced in Java: 3.4. Rust rejects conflicting resource definitions: 3.1. Java does not reject repeated map keys: 3.4. |
| Discovery reads confined from Rust; the Java resolver never fetches | Rust discovery reads within the root only: 3.1. Java resolver reads no filesystem or network: 3.4. |
| Discovery follows imports reachable from the root file only | Reachable imports are included: 3.1, 3.5. Unrelated files are excluded: 3.1. Diamond import is included once without a duplicate error: 3.1. Cyclic include terminates without a depth loop: 3.1. Distinct virtual bases expand their own relative children: 3.1, 3.5. Symlinked include cycle terminates and keeps every virtual lookup key: 3.1, 3.5. Depth limit is enforced: 3.1. In-root symlink is accepted: 3.1. Escaping symlink is rejected: 3.1. Traversal outside the virtual root is rejected: 3.1. Single file without imports is a one-entry set: 3.1, 3.5. |
| Supplied absolute-URI aliases resolve; only unsupplied URLs are refused | Supplied alias resolves without a fetch: 3.4, 3.5. Unsupplied URL is refused: 3.4, 3.5. Alias and relative path for the same file count separately: 3.4. Alias key must be an absolute URI: 3.4. Alias value must be normalized relative and present in resources: 3.4. In-root traversal in an alias value normalizes and resolves: 3.4. |
| Resolver serves supplied keys only with defined semantics | Relative import resolves from the supplied set: 3.4. Base-relative resolution beats an exact root raw key: 3.4, 3.5. Alias resolution uses the mapped virtual base: 3.4, 3.5. Nested importer and alias do not double the base: 3.4, 3.5. Escaping traversal and DTD are rejected: 3.4, 3.5. In-root traversal normalizes before key lookup: 3.4, 3.5. |
| Schema cache key includes child digests and replay re-registers the set | Changed child yields a new cache key: 3.3, 3.5. Restart replay re-registers the whole set: 3.3, 3.5. |
