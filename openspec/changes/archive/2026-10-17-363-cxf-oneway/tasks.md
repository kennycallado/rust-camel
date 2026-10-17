# Tasks: 363-cxf-oneway

## Execution contract

This is the complete plan for both delivery phases. Do not implement before fresh plan blessing and master approval. Execute tasks in order. Each task has a coding budget of 30 minutes; if it cannot finish safely in that budget, stop and report the remaining work instead of expanding its scope. Native builds and repository gates are separately monitored operational runs, not additional coding work.

Use only `/home/shared/rust-camel-worktrees/363-cxf-oneway` for project changes. Set `TMPDIR=/home/shared/tmp`, `JAVA_TOOL_OPTIONS=-Djava.io.tmpdir=/home/shared/tmp`, `CARGO_BUILD_JOBS=4`, and `CARGO_TARGET_DIR=/home/shared/rust-camel-worktrees/363-cxf-oneway/target`. The Java setting must reach Gradle daemons and forked test JVMs; verify their reported `java.io.tmpdir` before tests. Run one cargo command at a time. Prefix host build/test commands with `systemd-run --user --scope --quiet -p MemoryMax=12G -p MemorySwapMax=0 --` after verifying user cgroup-limit support; if unavailable, stop for an approved equivalent rather than run uncapped. Container-native builds also need the Docker memory limit in Task 2.8. Check disk before each build; park at 76% usage, never approach the 80% ceiling. No push, fetch, tag, merge, archive, bd closure, or extra worktrees. Never stage `.fleet/`, `.review.json`, or `.bless.json`. Run bd only from `/home/kenny/dev/rust-camel`.

Default worker: `workers/w_low`. Cross-file metadata/invocation/consumer/native integration tasks may use `workers/w_balanced`; record the reason in the task verdict. Fresh `reviewers/r_gpt` reviews each task, both multi-task phases, and the final complete implementation. After Phase 1, commit reviewed progress, write master inbox evidence, and stop for approval. After Phase 2 and Stage 4, park for master; do not land or archive.

Write exactly the new tests specified below before production changes and observe the stated failures. Existing regression tests must remain enabled. No additional test design by workers: report `test-design-gap` when this plan is insufficient. Java commands run from the worktree root with `bridges/cxf/gradlew -p bridges/cxf`; do not create a second build tree. For any modified Rust crate, finish with `cargo fmt --check --all` and affected-crate clippy with `-- -D warnings`.

### Scope-check and symbols

- Phase 1 is one producer-delivery slice: operation metadata, immutable dispatch selection, invocation contract, and replay safety. Four tasks, no independent subsystem.
- Phase 2 is one listener-ACK slice and its end-to-end verification: startup metadata, ACK/fault rendering, Rust marshalling evidence, native tests and signed-fixture preparation, documentation, and final verification. Eight tasks, all support the same deliverable. Both boundaries match blessed `design.md`.
- Task 1.1 introduces package-private `WsdlOperationCatalog`, nested immutable `Operation(QName name, Set<QName> inputQNames, String soapAction, boolean oneWay)`, nested `InvalidOperationException extends IllegalArgumentException`, `static WsdlOperationCatalog fromBinding(BindingInfo binding)`, `Operation resolveProducer(String operation, QName inputQName)`, `Operation resolveConsumer(QName inputQName, String soapAction)`, and `static String normalizeSoapAction(String raw)` (null stays null).
- Task 1.2 introduces `CxfClientManager.ResolvedDispatch(Dispatch<Source> dispatch, WsdlOperationCatalog.Operation operation)` and `ResolvedDispatch getResolvedDispatch(String wsdl, String address, String service, String port, String operation, String profileName, long timeoutMs, QName inputQName) throws Exception`. Keep the existing `getDispatch` signature as an explicit-operation compatibility delegate; unnamed production calls must use the QName-aware method. No other production path may use the compatibility delegate for unnamed dispatch.
- Task 2.1 introduces `static WsdlOperationCatalog load(String wsdl, String service, String port) throws Exception` and package-private `void SoapEndpointPublisher.initializeOperationCatalogs()`. Catalog loading makes no HTTP invocation.
- Task 2.4 introduces `spawn_soap_tls_server_with_response(cert_pem: &[u8], key_pem: &[u8], client_auth: TestClientAuth, response_status: u16, response_body: Vec<u8>)` with the same return type as existing `spawn_soap_tls_server`. Existing callers retain their behavior through the old function delegating to the new one.
- Task 2.5 introduces the test-support JUnit entrypoint `void SignedRequestFixtureGeneratorTest.regenerateLongDnWithExistingKeystore() throws Exception`, gated by `CXF_LONGDN_REUSE_FIXTURE_DIR`, for re-signing XML without regenerating its certificate.

### Coverage map

- Operation-shape requirement: explicit/name-free/unknown/ambiguous producer scenarios, 1.1–1.3; listener conflicting-action scenario, 2.1–2.2.
- One-way producer/no-response requirement: 1.2–1.3 and exact-name native mTLS acceptance, 2.4.
- Non-robust/no-replay requirement: local failure, 1.3; lost bridge response and later explicit send, 1.4.
- Strict empty request-reply requirement: null/empty/parser failures and one wire POST, 1.3; native coverage, 2.4.
- Correlated empty-202 consumer requirement: held completion/nonempty serialized output, 2.2 and 2.6; unsupported stream output, 2.3; fault/timeout/503/request-reply regressions, 2.2 and 2.6.
- Modified fault-status requirement: every existing Sender/Receiver/signature/timeout/SOAP-version scenario is retained in 2.2 and rerun in 2.6/2.8; input-only success and stream-marshalling scenarios are owned by 2.2–2.3.

## Phase 1: Producer one-way delivery

### Java operation metadata

#### Task 1.1: Resolve immutable operation shape from selected binding

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/WsdlOperationCatalog.java` (new)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/WsdlOperationCatalogTest.java` (new)
- `bridges/cxf/src/test/resources/soap-oneway.wsdl` (new)

**Steps:**
1. Add a SOAP 1.1 document/literal fixture with namespace `urn:oneway:363`, `ReceiveMessageOp` input `{urn:oneway:363}receiveMessage` and action `urn:receive`, no output; `EchoOp` input `echoRequest`, output `echoResponse`, action `urn:echo`; and a second port binding containing an alias with the same input QName to exercise ambiguity. Keep the alias absent from the main binding.
2. Introduce the catalog, operation record, exception, and three methods listed in the symbol contract. Copy operation metadata from the supplied selected CXF `BindingInfo`; do not keep mutable CXF model collections. Use wrapped/document input element QNames from message parts, not operation local names or unwrapped scalar parameter names. Classify one-way from input presence and absent WSDL output.
3. Resolve explicit producer names against selected-binding operation local names; reject unknown or non-unique names. Resolve absent/blank producer names by exact namespace-qualified input QName; reject zero/multiple matches. Reject a missing QName when there is no explicit name.
4. Implement `normalizeSoapAction`: preserve null for absence; otherwise trim and remove one matching outer quote pair, never every quote. Resolve consumer input by unique body QName, then compare normalized supplied SOAPAction. A present empty action must match a declared empty action. Normalize a missing declared action to the empty string. Throw `InvalidOperationException` for unknown, ambiguous, or conflicting selection.

**Tests:**
- `explicitInputOnlyOperationUsesSelectedBinding`: setup the main fixture binding; action resolve `ReceiveMessageOp` with a valid input QName; assert name, `urn:receive`, and `oneWay=true`, while `EchoOp` is false. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests '*WsdlOperationCatalogTest'`. Expected: fail before catalog exists; pass after implementation.
- `unnamedInputUsesQualifiedWrappedElement`: setup main binding; action resolve unnamed `receiveMessage`, then an otherwise identical QName in a different namespace; assert the first selects `ReceiveMessageOp` and the second throws. Same command. Expected: fail before implementation; pass afterward.
- `unknownAndAmbiguousSelectionsFail`: setup main and alias bindings; action resolve missing explicit name, missing input, unmatched input, and the alias binding's shared input; assert `InvalidOperationException` for each and alias absence in the main binding. Same command. Expected: fail before implementation; pass afterward.
- `consumerActionMustAgreeWithUniqueInput`: setup main binding; action resolve receive input with absent, quoted `urn:receive`, conflicting `urn:echo`, and present-empty actions; assert success only for absent/matching action. Include an empty-action mocked binding and assert quoted-empty agrees there. Same command. Expected: fail before implementation; pass afterward.
- `catalogSnapshotCannotBeMutated`: setup catalog from mutable mocked CXF metadata; action mutate original collections and attempt to mutate returned input-QName set; assert catalog selection is unchanged and returned set rejects mutation. Same command. Expected: fail before implementation; pass afterward.

**Acceptance:**
- All five catalog tests pass; `bridges/cxf/gradlew -p bridges/cxf spotlessCheck` exits 0.
- Catalog creation/selection performs no dispatch invocation and no network request; the fixture's main port remains independent of alias-port operations.

- [x] 1.1

### Java dispatch and invocation

#### Task 1.2: Cache resolved dispatches without per-call context mutation

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfClientManager.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/CxfClientManagerTest.java` (modified)

**Steps:**
1. Add `ResolvedDispatch` and `getResolvedDispatch` with the exact signatures in the symbol contract. Build the catalog from the CXF dispatch's selected binding and resolve the operation before publishing the dispatch into the cache. Return metadata and dispatch together.
2. Extend the existing bounded LRU key/value as needed so explicit names and inferred input QNames cannot reuse a dispatch configured for another operation. Preserve address/profile/timeout isolation, eviction closure, startup TLS attachment, and creation locking. Do not add an unbounded parallel metadata cache.
3. Configure SOAPAction from resolved metadata at creation. For input-only dispatches set `org.apache.cxf.oneway.robust` to `Boolean.FALSE` before caching. Never change those context entries after lookup.
4. Keep the existing explicit-operation `getDispatch` method delegating to `getResolvedDispatch` with a null input QName. Update existing cache/TLS test setup to supply a known operation where it previously used a blank name. Close rejected temporary dispatches, propagate the original argument exception, and never cache a failed selection.

**Tests:**
- `resolvedDispatchCarriesShapeAndFixedNonRobustMode`: setup main fixture binding with mocked `DispatchImpl`; action look up named receive and echo operations; assert metadata shape and action, plus robust property false for receive. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests '*CxfClientManagerTest'`. Expected: new assertions fail before implementation; pass afterward.
- `inferredOperationsDoNotCrossContaminateCachedContext`: setup receive/echo input QNames; action preseed dispatches, perform alternating lookups and concurrent lookups with existing thread-safe harness; assert each dispatch retains its own SOAPAction/shape and no post-creation context writes occur. Preseed before concurrent work because Mockito static Service mocks are thread-confined. Same command. Expected: fail before implementation; pass afterward.
- `invalidResolutionClosesDispatchWithoutCaching`: setup unknown explicit, unmatched inferred, and ambiguous inferred selections; action request each; assert argument-class exception, close called once per created rejected dispatch, cache size unchanged, and zero invokes. Same command. Expected: fail before implementation; pass afterward.
- Existing `soapActionSetAtCreationNotAfterLookup`, `concurrentDistinctOperationsDoNotCrossContaminate`, `operationParticipatesInCacheKey`, and all TLS/cache eviction tests: setup their existing fixtures; action run the full manager class; assert existing isolation, TLS, and closure assertions still pass. Same command. Expected: remain passing after mechanical fixture adaptation; do not weaken assertions.

**Acceptance:**
- Manager test class and `spotlessCheck` exit 0. Cache size remains bounded by the existing configured limit.
- Only creation sets operation-specific request context. Rejected resolution causes no wire invocation and no cached resource leak.

- [x] 1.2

#### Task 1.3: Invoke input-only operations once and reject empty request-reply

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfBridgeService.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/CxfBridgeServiceTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/CxfBridgeServiceBoundaryTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/CxfClientManagerWireTest.java` (modified)

**Steps:**
1. Parse the bounded bare request payload with existing hardened `SoapEnvelopeHelper.parseResponse`, obtain its root QName, and preserve the existing prewrapped-Envelope rejection before invocation. Call `getResolvedDispatch` with that QName and map `InvalidOperationException` to gRPC `INVALID_ARGUMENT`, preserving existing unknown-operation context.
2. Branch on resolved `operation.oneWay()`. Call `invokeOneWay` once, return one default non-fault `SoapResponse` with empty bytes and complete the observer. Do not serialize or inspect a response Source on this branch. Propagate local invocation exceptions through existing error handling.
3. Retain one `invoke` for request-reply. Reject null Source before transformation and zero-length serialized payload afterward as gRPC `INTERNAL` protocol failures with a static empty-SOAP-response description. Preserve CXF parser exceptions as errors, existing response size cap, structured SOAP faults, and transport/WSS/TLS behavior. Never retry or switch dispatch method after an error.
4. Update mocks in both service test classes to return resolved metadata. Add a parameterized wire test over operation shape and HTTP 200/202-empty using the real CXF fixture and existing Vert.x capture server. Keep all 361 single-envelope, SOAP-version, declared-action, and prewrapped-payload tests enabled.

**Tests:**
- `oneWayInvokesOnceAndReturnsEmptySuccess`: setup resolved receive metadata and mocked dispatch; action invoke the gRPC service; assert `invokeOneWay` once, `invoke` never, exactly one empty non-fault response and one observer completion. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests '*CxfBridgeServiceTest'`. Expected: fail before implementation; pass afterward.
- `oneWayLocalFailureDoesNotFallbackOrSucceed`: setup `invokeOneWay` throwing `WebServiceException`; action invoke service; assert observer error, zero successful responses, one one-way attempt and no request-reply attempt. Same command. Expected: fail before implementation; pass afterward.
- `requestReplyMissingContentIsAnError`: setup echo metadata; parameterize null Source, zero-content StreamSource, and CXF-style parser exception; action invoke service; assert non-OK observer result, one `invoke`, zero `invokeOneWay`, no fabricated payload. Same command. Expected: null/empty guards fail before implementation; existing exception propagation remains passing.
- `emptyAcknowledgmentsFollowWsdlShapeAndSendOnce`: setup a real main-fixture dispatch and recording HTTP server for each receive/echo and 200/202 combination; action send a namespace-qualified payload; assert receive returns empty success, echo returns error, each records exactly one POST, one SOAP 1.1 Envelope, bare operation as direct Body child, and declared action. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests '*CxfClientManagerWireTest'`. Expected: one-way success variants fail before implementation; strict echo errors remain errors; all variants pass afterward.
- Existing boundary and 361 wire tests: setup existing size/version/action/envelope fixtures; action run all Java tests; assert no reduced caps, double envelope, missing TLS attachment, or changed SOAP fault mapping. Command: `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck`. Expected: pass after mock adaptation and implementation.

**Acceptance:**
- Full Java test suite and `spotlessCheck` exit 0. All four empty-response wire variants record exactly one POST.
- The one-way branch contains no Source serialization and the request-reply branch contains no fallback invocation.

- [x] 1.3

### Rust producer

#### Task 1.4: Preserve empty bytes and eliminate invocation replay

**Files:**
- `crates/components/camel-cxf/src/producer.rs` (modified)
- `crates/components/camel-cxf/tests/producer_unit_test.rs` (modified)
- `crates/components/camel-cxf/tests/support/mock_bridge.rs` (modified)

**Steps:**
1. Extend existing tonic mock state with an invocation counter and a FIFO of invocation outcomes. Count receipt before responding so a transport error can represent a possibly transmitted SOAP send. Keep existing default-success behavior for all other tests.
2. Remove the branch that refreshes and invokes the same request again after gRPC transport failure. Keep invalidation/recovery state needed to acquire a fresh client on a later explicit call; do not convert the failed call to success.
3. Preserve the successful empty-payload mapping to `Body::Bytes` of length zero, structured SOAP fault handling, operation-header selection, and security headers. Update existing retry-expecting tests to assert no replay rather than deleting their error-path coverage.

**Tests:**
- `cxf_empty_success_preserves_empty_bytes`: setup mock returning successful empty `SoapResponse`; action call producer once with nonempty input; assert output is `Body::Bytes` length zero, not original input, and invocation count is one. Command: `cargo test -p camel-component-cxf --test producer_unit_test`. Expected: existing behavior may already pass; must remain passing.
- `cxf_transport_failure_never_replays_invocation`: setup scripted `Unavailable` and transport-class `Internal` outcomes as separate cases, followed by success; action perform one producer call per isolated case; assert error and count one, with success response unconsumed. Same command. Expected: fail against the retrying producer; pass after removal.
- `cxf_later_explicit_send_recovers_after_transport_failure`: setup first recorded call fails and second succeeds; action issue first call, then a new explicit call through the recovery path; assert first error, second success, total count two, and second payload belongs to the second call. Same command. Expected: fail before implementation because the first call replays; pass afterward.
- Existing `test_invoke_success_sets_response_body` and SOAP-fault/operation-header cases: setup existing mocks; action run component suite; assert behavior unchanged. Command: `cargo test -p camel-component-cxf`. Expected: remain passing except retry assertions intentionally replaced above.

**Acceptance:**
- `cargo test -p camel-component-cxf`, `cargo fmt --check --all`, and `cargo clippy -p camel-component-cxf --all-targets --all-features -- -D warnings` exit 0.
- Fresh Phase-1 `r_gpt` review covers the diff from Phase-1 start commit through all four task commits and approves cross-task action/shape/replay behavior. Record start/end SHAs, gates, verdict, and renamed spawned sessions in the absolute master inbox; stop before Phase 2.

- [x] 1.4

## Phase 2: Consumer ACK and native verification

### Java listener metadata and ACK

#### Task 2.1: Load the selected listener operation catalog before binding

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/WsdlOperationCatalog.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/WsdlOperationCatalogTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherTest.java` (modified)

**Steps:**
1. Add catalog `load` using existing `Service.create` and a temporary PAYLOAD CXF Dispatch for the configured WSDL/service/port; copy selected `BindingInfo` through `fromBinding`, then close the temporary dispatch on success or failure. Do not call either invocation method or create a remote connection. Loading uses configured paths only, not exchange data.
2. Add `initializeOperationCatalogs()` to construct an immutable per-profile catalog snapshot from `profileStore.profiles()` and each profile's `wsdlPath()`, `serviceName()`, and `portName()`. Invoke it before the HTTP listener binds. Keep no-bind producer-only startup behavior unchanged. Configuration/load failure must fail listener startup before a ready port is announced.
3. Prepare package-visible catalog injection through the existing publisher test setup, not a production bypass. Existing unrelated WSS/fault harness cases may inject a fixed request-reply catalog; real operation-selection cases must use the actual new fixture. Update listener startup tests to provide valid fixture profile metadata.

**Tests:**
- `catalogLoadUsesSelectedPortWithoutInvoking`: setup the new fixture plus a recording temporary CXF dispatch; action load the main and alias ports; assert port-isolated metadata, zero invokes, and temporary dispatch closure. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests '*WsdlOperationCatalogTest'`. Expected: fail before load exists; pass afterward.
- `listenerCatalogFailurePreventsBind`: setup profile with missing WSDL or absent port; action publish; assert startup exception, no listener bind and no announced SOAP port. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests '*SoapEndpointPublisherTest'`. Expected: fail before startup metadata validation; pass afterward.
- `listenerCatalogSnapshotUsesStartupProfile`: setup real main-port profile and initialize its catalog; action change backing mocked profile metadata after initialization; assert the initialized catalog still resolves only the original selected binding and no request-time reload occurs. Same command. Expected: fail before catalog snapshot exists; pass afterward.
- Existing producer-only no-bind and listener TLS startup tests: setup their current configurations; action run publisher tests; assert no unintended listener in producer-only mode and all TLS shape guards remain. Same command. Expected: remain passing after fixture adaptation.

**Acceptance:**
- Catalog and publisher test classes plus `spotlessCheck` exit 0. No consumer payload can select a different WSDL/service/port.
- Metadata storage is bounded by configured profiles and temporary metadata dispatches are closed.

- [x] 2.1

#### Task 2.2: Render empty 202 only for correlated non-fault one-way completion

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherTest.java` (modified)

**Steps:**
1. Preserve availability-first 503, body cap, SOAP-version detection, envelope/Body validation, inbound WSS verification, and signer export. Change existing `extractSoapAction(Map<String, String> headers)` to return the raw header value when the `soapaction` key is present, even if empty. If that header is absent, extract the exact case-insensitive `action` parameter from SOAP 1.2 `application/soap+xml` content type, respecting quoted values and parameter boundaries; return null when neither source is present. Do not use the existing blank-to-empty collapse or remove-all-quotes behavior. Header presence takes precedence over content type, including explicit-empty headers.
2. After verified-body extraction, resolve its QName and raw nullable action using the startup catalog. Use resolved operation name for `ConsumerRequest.operation`; set protobuf `soap_action` to `normalizeSoapAction(rawAction)` or the empty string only after resolution when the raw action was absent. Never pass the protobuf fallback value back into operation selection.
3. Map catalog `InvalidOperationException` to Sender using the existing version/status renderer, never the WSS-only `fault_status` override, and do not dispatch rejected inputs to Rust.
4. Keep `handleSoapRequest(consumerRequest).get(bridgeConfig.consumerTimeoutMs(), TimeUnit.MILLISECONDS)` correlation and all fault/error branches. Only after a non-fault `ConsumerResponse`, branch on input-only metadata and return `SoapHttpResult(202, detectedVersion, "")` before payload wrapping and outbound signing. Non-fault request-reply still wraps/signs one SOAP response with HTTP 200.
5. Extend `driveRequestOutcome` with an asynchronous, latch-driven execution mode so a held future cannot block the test thread. Wait for request-dispatch acknowledgment, assert no response ended yet, then complete the correlated future. Use bounded waits, no sleeps or unbounded loops.

**Tests:**
- `oneWayActionPresenceSurvivesHttpExtraction`: setup real receive catalog and completed successful response; parameterize SOAP 1.1 absent header, matching quoted header, present-empty header, quoted-empty header, and SOAP 1.2 content types with absent action, matching quoted/unquoted action, present-empty/quoted-empty action, mixed-case parameter, and unrelated `notaction` parameter. Action send each through publisher; assert absent or matching action permits empty 202, explicit-empty conflicts with `urn:receive` and faults before route dispatch, unrelated parameter counts as absence, and empty SOAPAction header overrides a matching content-type action. Add a declared-empty-action catalog case proving present-empty succeeds there. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests '*SoapEndpointPublisherTest'`. Expected: fail against the lossy existing extraction; pass after implementation.
- `oneWayAckWaitsForCorrelatedNonFaultCompletion`: setup real receive catalog, held completion future, and signing-capable WSS mock; action dispatch, await entered latch, inspect no response, then complete with nonempty serialized payload and matching request id; assert operation name `ReceiveMessageOp`, empty HTTP 202, zero outbound signing calls, and response end exactly once after completion. Command: `bridges/cxf/gradlew -p bridges/cxf test --tests '*SoapEndpointPublisherTest'`. Expected: fail before ACK branch; pass afterward.
- `oneWayUnknownAmbiguousAndConflictingInputsNeverDispatch`: setup main/alias catalogs and supplied actions; action send unknown QName, alias-ambiguous input, and conflicting/empty action for receive; assert zero route calls, SOAP 1.1 Sender HTTP 500 or SOAP 1.2 Sender HTTP 400 according to envelope version, even with configured WSS `fault_status`. Same command. Expected: fail before operation validation; pass afterward.
- `oneWayFailuresPreserveAvailabilitySecurityAndFaultMatrix`: setup receive input and parameterized missing/invalid WSS, route `soap:Client`, route `soap:Server`, unknown fault code, signer-descriptor Receiver fault, timeout, and early/late unavailable-stream cases for SOAP 1.1/1.2; action drive each; assert existing matrix and security override only for WSS, Receiver 500 for route/descriptor/timeout, unavailable 503 with Retry-After/plain body, never 202. Same command. Expected: new operation-aware setup exercises unchanged failure behavior; pass after implementation without relaxing existing assertions.
- `requestReplySuccessStillWrapsAndSignsOnce`: setup real echo catalog and non-fault serialized payload; action complete request; assert HTTP 200, one version-matched Envelope, payload retained and outbound signer called once if configured. Same command. Expected: remain passing after operation-aware setup.
- Existing `versionDetectedBeforeWss`, `senderFaultIs400AndReceiverFaultIs500`, `missingBodyIsSenderFault`, `malformedEnvelopeFails400`, `signedRouteFaultRemainsReceiver`, `preStream503IsTransportAvailabilityNotFault`, `availabilityCheckedBeforeWssForSignatureProfile`, and `lateStreamLossYieldsSame503`: setup existing inputs; action full Java suite; assert all existing security/namespace/version assertions remain passing. Command: `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck`. Expected: pass; catalog test injection must not bypass production validation in the new operation-aware cases.

**Acceptance:**
- Full Java tests and `spotlessCheck` exit 0. No 202 is emitted for fault responses or before correlated completion.
- No protobuf change, Rust marshalling bypass, outbound ACK envelope, or ACK signature is introduced.

- [x] 2.2

### Rust marshalling and native wire evidence

#### Task 2.3: Prove route success with stream output stays a correlated fault

**Files:**
- `crates/components/camel-cxf/tests/consumer_emission_test.rs` (modified)

**Steps:**
1. Reuse `Behavior::ReplyStreamBody` and the existing consumer-emission harness to exercise successful route execution with unsupported streamed output. Add explicit assertions on the emitted response request id, `fault=true`, `fault_code=soap:Server`, and absence of a normal success payload.
2. Leave production `consumer.rs`, `build_response_body`, and error marshalling unchanged. Pair this evidence with Task 2.2's input-only Receiver-response case; Java must not suppress the resulting fault.

**Tests:**
- `cxf_stream_output_emits_correlated_receiver_fault`: setup input-only operation metadata on a consumer request with a known request id and a route that returns `Body::Stream` successfully; action wait for emitted response using the existing bounded harness; assert the same request id, `fault=true`, `soap:Server`, and no non-fault response. Command: `cargo test -p camel-component-cxf --test consumer_emission_test cxf_stream_output_emits_correlated_receiver_fault`. Expected: pass against current marshalling implementation; this is explicit regression evidence, not a reason to change production behavior.
- Existing `build_response_body_stream_returns_error` and `cxf_marshalling_failure_retains_b_prime_and_double_counts`: setup existing stream fixtures; action run component suite; assert stream rejection and existing accounting stay intact. Command: `cargo test -p camel-component-cxf`. Expected: remain passing.

**Acceptance:**
- Component tests, `cargo fmt --check --all`, and `cargo clippy -p camel-component-cxf --all-targets --all-features -- -D warnings` exit 0.
- Diff contains test assertions only; no successful-ACK shortcut in Rust.

- [x] 2.3

#### Task 2.4: Add native empty-ACK mTLS wire acceptance

**Files:**
- `crates/camel-test/tests/support/cxf.rs` (modified)
- `crates/camel-test/tests/cxf_test/tls.rs` (modified)
- `crates/camel-test/tests/cxf_test/wire.rs` (modified)

**Steps:**
1. Add `spawn_soap_tls_server_with_response` from the symbol contract and retain old helper behavior by delegation. Read complete HTTP headers and request bytes up to bounded Content-Length, reject malformed/oversized lengths, record full body and POST count in existing `TestTlsServerState`, and close each test connection deterministically. Use the current CXF client's framing; if a different transfer encoding appears, stop with `test-design-gap` rather than claiming envelope coverage over unread bytes.
2. Add the exact-name acceptance test using the single Java `soap-oneway.wsdl` fixture via workspace-relative path. Use fresh server/state per status 200 and 202, valid server trust, and required client certificate from `TestTlsCa`/`TestTlsStores`. Invoke `ReceiveMessageOp` once per variant with a bare receive payload.
3. Give existing `wire::CapturedSoapStructure`, the fields read by sibling tests, and `wire::parse_soap_structure` the narrow `pub(super)` visibility needed by `tls.rs`. Reuse that parser, do not duplicate it. Add request-reply empty-response variants using `EchoOp` in the same fixture; assert namespace-qualified Envelope count and direct Body operation element, not substring counts.
4. Keep existing mTLS trust/missing-client/wrong-client/hostname rejection tests intact. Compile the native test target before native rebuilding; a downloaded bridge is not valid acceptance evidence.

**Tests:**
- `cxf_tls_capture_rejects_invalid_content_lengths`: setup the fake TLS server and a raw trusted TLS client without launching a bridge; action send nonnumeric, negative, and over-cap Content-Length headers in isolated connections using bounded waits; assert rejection without unbounded allocation or a recorded complete POST, and clean server shutdown. Also send a valid body split across writes and assert the full body is recorded once. Command: `cargo test -p camel-test --features integration-tests --test cxf_test cxf_tls_capture_rejects_invalid_content_lengths -- --test-threads=1`. Expected: fails before full-body bounded capture exists; passes afterward. Fix the fake-server cap at 1 MiB, above all fixture payloads; do not inherit untrusted lengths as allocation sizes.
- `cxf_oneway_empty_200_and_202_complete_ok_single_envelope_over_mtls`: setup isolated required-client-auth fake servers returning zero-body 200 and 202; action one producer send per variant through rebuilt bridge; assert `Ok`, output `Body::Bytes` length zero, exactly one POST, exactly one SOAP 1.1 Envelope with direct Body child `{urn:oneway:363}receiveMessage`, `SOAPAction=urn:receive`, and accepted client authentication. Command after Task 2.8 rebuild: `cargo test -p camel-test --features integration-tests --test cxf_test cxf_oneway_empty_200_and_202_complete_ok_single_envelope_over_mtls -- --exact --test-threads=1`. Expected: fail against pre-363 bridge; pass after rebuild. If the runner registers a module-qualified test name, use its fully qualified name obtained from `--list` with `--exact`; never accept zero tests run.
- `cxf_request_reply_empty_200_and_202_fail_without_resend_over_mtls`: setup the same isolated TLS cases with `EchoOp`; action one explicit send; assert error, exactly one authenticated POST, single SOAP 1.1 Envelope and direct `echoRequest` Body child. Command: `cargo test -p camel-test --features integration-tests --test cxf_test cxf_request_reply_empty_200_and_202_fail_without_resend_over_mtls -- --test-threads=1`. Expected: existing bridge may already reject empty content; must pass with exactly-one-send assertion after implementation.
- Existing producer TLS rejection tests: setup their existing CA and host mismatch cases; action run the full native CXF target after rebuild; assert no weakened trust validation or empty-success conversion of detected TLS errors. Command: `cargo test -p camel-test --features integration-tests --test cxf_test -- --test-threads=1`. Expected: remain passing.

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test cxf_test --no-run`, `cargo fmt --check --all`, and `cargo clippy -p camel-test --all-targets --features integration-tests -- -D warnings` exit 0.
- Native execution is mandatory in Task 2.8; compile-only or zero-test success cannot satisfy the exact-name acceptance.

- [x] 2.4

#### Task 2.5: Align signed and unsigned native fixtures with WSDL inputs

**Files:**
- `crates/camel-test/tests/cxf_test.rs` (modified)
- `crates/camel-test/tests/cxf_test/listener.rs` (modified)
- `crates/camel-test/tests/cxf_test/signer.rs` (modified)
- `crates/camel-test/tests/cxf_test/wire.rs` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SignedRequestFixtureGeneratorTest.java` (modified)
- `crates/camel-test/tests/fixtures/cxf-wss-signed-request.xml` (modified)
- `crates/camel-test/tests/fixtures/cxf-wss-longdn-signed-request.xml` (modified)

**Steps:**
1. Correct well-formed existing listener request fixtures to use the selected WSDL's declared input element `sayHelloRequest`, not the operation local name `sayHello`; preserve intentionally malformed/decoy/missing-Body/WSS-invalid fixtures and keep declared SOAPAction unchanged.
2. Change the real SOAP Body input element in generator `SOAP12_REQUEST` to `hel:sayHelloRequest`, retaining the decoy Header `s11:Body`, malicious operation QName, no-Timestamp signature, algorithms, and signer identity. Do not manually modify signed XML or regenerate JKS certificates.
3. Add `regenerateLongDnWithExistingKeystore()` gated by `CXF_LONGDN_REUSE_FIXTURE_DIR`. Load its existing `cxf-wss-longdn-keystore.jks` or fail if absent, use existing `TestKeystoreHelper` alias/password constants, re-sign the corrected static request with the existing WSS processor, and write only `cxf-wss-longdn-signed-request.xml`. Assert JKS bytes are identical before/after. Do not call the certificate-generating `generateLongDnFixture` entrypoint for this mission.
4. With the global host memory/JVM scratch settings, regenerate standard XML using `CXF_SIGNED_FIXTURE_JKS=/home/shared/rust-camel-worktrees/363-cxf-oneway/crates/camel-test/tests/fixtures/cxf-wss-test-keystore.jks CXF_SIGNED_FIXTURE_OUT=/home/shared/rust-camel-worktrees/363-cxf-oneway/crates/camel-test/tests/fixtures/cxf-wss-signed-request.xml bridges/cxf/gradlew -p bridges/cxf test --tests 'org.rustcamel.cxf.SignedRequestFixtureGeneratorTest.generate' --rerun-tasks`. Regenerate long-DN XML using `CXF_LONGDN_REUSE_FIXTURE_DIR=/home/shared/rust-camel-worktrees/363-cxf-oneway/crates/camel-test/tests/fixtures bridges/cxf/gradlew -p bridges/cxf test --tests 'org.rustcamel.cxf.SignedRequestFixtureGeneratorTest.regenerateLongDnWithExistingKeystore' --rerun-tasks`. Record both keystore SHA-256 values before and after; both must match exactly.

**Tests:**
- `regenerateLongDnWithExistingKeystore`: setup existing long-DN JKS and opt-in output directory; action run the exact regeneration command above; assert unchanged keystore bytes, regenerated XML verifies against that certificate, direct SOAP Body child is `sayHelloRequest`, decoy Header Body remains, and no Timestamp is introduced. Expected: fails because the new entrypoint does not exist before implementation; passes afterward. Standard `generate` must retain the same direct-Body/decoy/no-Timestamp assertions and unchanged standard keystore identity.
- Existing native signer/request-reply regression cases: setup corrected XML and unchanged standard/long-DN keystores; action compile `cargo test -p camel-test --features integration-tests --test cxf_test --no-run`, then rerun full native target in Task 2.8; assert standard signer descriptor values stay identical, long-DN still fails the Rust descriptor bound with Receiver HTTP 500 and zero route dispatch, and decoy Body cannot select another operation. Expected: compile passes and the rebuilt native regressions remain passing.

**Acceptance:**
- Both explicit generator commands report one executed test, XML signature verification passes, Java `spotlessCheck` and native target compilation exit 0.
- No JKS/certificate file changes in `git diff --name-only`; no hardcoded signer expectation changes; fmt and camel-test clippy from Task 2.4 exit 0.

- [x] 2.5

#### Task 2.6: Verify listener ACK and preserve 361 native regressions

**Files:**
- `crates/camel-test/tests/cxf_test/listener.rs` (modified)

**Steps:**
1. Add a native listener route configured with the new input-only fixture and mTLS listener material. Hold route completion with an explicit async notification pair, send a valid receive request, await route entry, and assert the HTTP task is unfinished. Release with supported nonempty output and check zero-length 202 response. Do not use sleeps.
2. Add a second isolated route configured to complete successfully with `Body::Stream`; assert Receiver HTTP 500 and a version-correct fault. Add a wrong SOAPAction variant and assert Sender fault and no route entry.
3. Retain existing request-reply listener/route-fault/timeout/TLS/restart/readiness/signer suites with Task 2.5's corrected fixtures. Do not change WSDL exchange shape to make tests pass.

**Tests:**
- `cxf_consumer_oneway_mtls_ack_waits_for_completion`: setup one-way route with held completion and mutual-TLS client; action dispatch, await entry, check unfinished HTTP future, release supported serialized output; assert operation metadata `ReceiveMessageOp`, empty 202, no Envelope, no early ACK, and one route execution. Command after rebuild: `cargo test -p camel-test --features integration-tests --test cxf_test cxf_consumer_oneway_mtls_ack_waits_for_completion -- --test-threads=1`. Expected: fail before listener ACK implementation; pass afterward.
- `cxf_consumer_oneway_stream_output_is_receiver_fault`: setup successful route with stream output and valid request; action send and await correlated response; assert HTTP 500, SOAP 1.1 `Server` fault, never 202. Command: `cargo test -p camel-test --features integration-tests --test cxf_test cxf_consumer_oneway_stream_output_is_receiver_fault -- --test-threads=1`. Expected: remain an error; new input-only fixture proves it is not suppressed.
- `cxf_consumer_oneway_conflicting_action_never_dispatches`: setup receive QName and `urn:echo` action with route-entry counter; action send through listener; assert Sender HTTP 500, valid fault XML and route count zero. Command: `cargo test -p camel-test --features integration-tests --test cxf_test cxf_consumer_oneway_conflicting_action_never_dispatches -- --test-threads=1`. Expected: fail before operation validation; pass afterward.
- Existing `cxf_consumer_fault_code_and_timeout_matrix`, `cxf_consumer_mutual_tls_uses_announced_port`, `cxf_consumer_mutual_tls_restart_preserves_ca_and_port`, `cxf_consumer_expect_soap_ready_starts_without_consumer_rpc`, signer identity, and SOAP-envelope wire tests: setup their current security/runtime scenarios with corrected valid body inputs; action full native CXF suite; assert unchanged 361 guarantees. Command: `cargo test -p camel-test --features integration-tests --test cxf_test -- --test-threads=1`. Expected: remain passing.

**Acceptance:**
- Native target compiles; fmt and camel-test clippy commands from Task 2.4 exit 0. Task 2.8 runs every new native listener test against the rebuilt binary with a nonzero test count.
- No test is deleted, ignored, or loosened to evade operation resolution, signer identity, TLS, fault-matrix, or readiness checks.

- [x] 2.6

### Documentation and delivery verification

#### Task 2.7: Document one-way completion and strict request-reply behavior

**Files:**
- `crates/components/camel-cxf/src/lib.rs` (modified)
- `crates/components/camel-cxf/CONTEXT.md` (modified)
- `docs/src/components/ws-soap.md` (modified)
- `docs/src/concepts/glossary.md` (modified)
- `CONTEXT-MAP.md` (modified)
- `bridges/cxf/README.md` (modified)

**Steps:**
1. Remove current one-way exclusions in crate/guide prose. Define WSDL input-only selection, exactly-one-send behavior, empty producer bytes for ACKs, and strict empty request-reply errors. Explain that local transport failure remains error and subsequent explicit sends are separate actions.
2. Document fixed non-robust producer semantics: no public `oneway.robust` option and no guarantee of remote business/fault observation. Document listener empty 202 only after correlated non-fault completion including Rust marshalling, and Receiver HTTP 500 for unsupported stream output.
3. Add the term **One-way SOAP operation** consistently to CXF context, `CONTEXT-MAP.md`, and the guide glossary before OpenAPI. Preserve ADR-0001/0032/0036 boundaries and all 361 guarantees. Update the guide's unconditional success-200 statement to distinguish request-reply from input-only.
4. Document the pinned-0.8.0 rebuilt-native acceptance command and explicit binary override without changing any release version, generated schema, or component metadata. Scan existing examples for contradictory claims; do not redesign examples or their 361 security configuration.

**Tests:**
- Documentation consistency check: setup updated files; action search changed prose for one-way exclusions and unconditional success-200 claims, compare all six files against blessed spec; assert no stale exclusion, no robust-mode option advertised, and ACK eligibility includes marshalling. Command: `git diff --check` followed by the fresh task reviewer inspecting the six-file diff. Expected: stale claims present before edits; absent afterward.
- Existing documentation/citation checks: setup unchanged API with updated crate prose; action run `cargo xtask lint-context-citations` and `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core -p camel-builder -p camel-dsl -p camel-endpoint -p camel-component-cxf --no-deps`; assert exit 0. Expected: pass after edits; no prose-induced broken links or citations.

**Acceptance:**
- Both documentation commands, `cargo fmt --check --all`, and `git diff --check` exit 0. CXF version pins still equal `0.8.0`.
- Fresh task review explicitly checks guide, crate context, glossary, and examples alignment; no unsupported behavior is documented.

- [x] 2.7

#### Task 2.8: Rebuild pinned native bridge and record complete gate evidence

**Files:**
- `.fleet/native-build-tools/docker` (new; untracked scratch, never staged)
- `.fleet/native-build-tools/record-docker` (new; untracked recording executable, never staged)
- `.fleet/native-build-tools/build-native.sh` (new; untracked scratch-path overlay, never staged)
- `.fleet/native-build-tools/native-run-evidence.json` (new; untracked evidence, never staged)

**Steps:**
1. Verify Docker availability, Java/Gradle prerequisites, 12 GiB cgroup-limit support, worktree-local target, and disk below 76%. Obtain master's explicit approval of this scratch-overlay strategy before executing a native build; plan blessing alone is not execution approval. If approval or any prerequisite is missing, stop and park a concrete blocker; do not substitute a downloaded bridge or silently omit native verification.
2. Create the untracked overlay by copying current `bridges/cxf/build-native.sh` and replacing every literal default-system-temp-root occurrence with `/home/shared/tmp/363-cxf-oneway-native`. This includes musl toolchain extraction, musl-bin links/PATH, fallback cache, zlib source and tar extraction; comments may be updated in the copy. Verify the copy has zero original temp-root references, preserves every checksum/URL/build argument/version expression, and differs only in scratch paths. Do not modify the tracked script or xtask.
3. Create the untracked Docker launcher. Resolve real Docker into absolute `REAL_DOCKER` before prefixing PATH, pass through non-build commands, and intercept only the CXF build `run`. Add `--memory=12g --memory-swap=12g --cpus=4`, bind dedicated scratch at `/home/shared/tmp/363-cxf-oneway-native`, bind the overlay read-only over `/project/build-native.sh`, set `TMPDIR` and `JAVA_TOOL_OPTIONS=-Djava.io.tmpdir=/home/shared/tmp/363-cxf-oneway-native`, and remove xtask's default temporary-filesystem argument. Use argument arrays, not shell string evaluation. The mount preserves the script's `/project` relative resource paths.
4. Verify recorded launcher argv, overlay contents, and actual Java temporary-directory properties before building; container `java -XshowSettings:properties -version` must report the mission scratch path. Check host Java likewise. Then run `cargo xtask build-cxf-bridge --version 0.8.0` through the launcher and verified host memory scope. Record Docker inspect memory/swap values and enforce disk monitoring. If runtime tooling still selects the default system temporary directory, stop and park rather than continuing. Record source commit, overlay SHA-256, version argument, binary path/SHA-256, start/end times, and exit code. No publication or tag.
5. Set `CAMEL_CXF_BRIDGE_BINARY_PATH=/home/shared/rust-camel-worktrees/363-cxf-oneway/bridges/cxf/build/native/cxf-bridge`. Run the exact-name producer acceptance first, verify at least one test actually ran, then the complete `cargo test -p camel-test --features integration-tests --test cxf_test -- --test-threads=1`. Keep the explicit binary path for every invocation; record nonzero test counts and status for all 361 regressions.
6. Run Stage-4 quality gates from the current `AGENTS.md` registry, each command separately with an exit code. Add `cargo build --workspace` before tests, `cargo test --workspace --lib`, and `cargo test -p camel-core --test hexagonal_architecture_boundaries_test`. Do not run full `cargo test --workspace`. Skip `lint-commits` because it fetches; never execute it. Run Java `test spotlessCheck` again. Enumerate each registry command, the four clippy variants separately, the three conductor additions, and any documented N/A or verified-main exemptions in evidence. No blanket all-green claim with missing gates.
7. Resolve real in-scope findings before advancing. A pre-existing gate exemption requires verification on main without cargo in the main checkout, plus a linked bd issue created from root; without safe verification, report blocked rather than exempt. Infra/ignored test changes require `integration-verification-deferred-to-CI` in final handoff even when native CXF checks ran locally.
8. After task review, obtain fresh Phase-2 `r_gpt` review against the recorded start-of-phase commit. Then perform Stage-4 fresh holistic `r_gpt` review of the entire implementation and all blessed artifacts, including docs/examples alignment. Record local untracked review receipt, commit only intended source/docs/task progress, rerun change validation/hash checks, and write the final absolute master inbox report with commit SHA, each gate result, native binary hash, verdicts, and `spawned_sessions [{sid,title,renamed}]`. Stop without merge, archive, or bd closure.

**Tests:**
- `native_build_limits_and_scratch_are_enforced` (operational check, not a new project test): setup real Docker path, copied overlay, and launcher with recording executable replacing Docker; action feed representative xtask `info` and CXF `run` argv, inspect overlay diff, then probe host/container JVM properties; assert info pass-through, exact version preserved, Docker limits, overlay mount, zero default-temp-root references in overlaid script, unchanged toolchain integrity checks, and actual JVM scratch under `/home/shared/tmp`. Command: run launcher with recorder, compare argv and overlay, run host/container `java -XshowSettings:properties -version` with configured JVM property, then actual `cargo xtask build-cxf-bridge --version 0.8.0`. Expected: unmodified xtask/script fail checks; approved launcher/overlay/property combination passes before native build.
- Native acceptance and regression execution: setup fresh rebuilt binary and explicit override; action run Task 2.4 acceptance, Task 2.5 signed-fixture regressions, all Task 2.6 tests, and complete native target; assert required exact-name test count is one, all isolated HTTP status variants pass, and full target reports no failures or newly ignored native tests. Commands: exact commands in Tasks 2.4–2.6 plus complete target above. Expected: pass; any failure blocks final delivery. Opt-in fixture-generator skips in ordinary Java runs remain intentional and are not native coverage.
- Gate-coverage self-check: setup authoritative registry and recorded separate command exits; action compare gate names/commands with evidence plus conductor additions and fetch exclusion; assert every applicable gate has exit 0 or a justified recorded exemption, never an unrecorded omission. Command: `TMPDIR=/home/shared/tmp openspec status --change 363-cxf-oneway --json` and `TMPDIR=/home/shared/tmp openspec validate 363-cxf-oneway --type change --json`, followed by reviewer verification of the enumerated gate evidence. Expected: artifacts complete, all implementation checkboxes complete only after reviewed work, and delta validation passes.

**Acceptance:**
- Native producer exact-name acceptance and full CXF target pass against the newly built pinned-0.8.0 binary; binary SHA and source commit are recorded.
- Every required gate is explicitly accounted for. Phase and holistic reviews have no unresolved legitimate findings. Master inbox evidence uses `/home/kenny/dev/rust-camel/.opencode/fleet/inbox/` and all spawned session titles are renamed.
- Final tracked diff contains no `.fleet/`, `.review.json`, `.bless.json`, release version change, or unauthorized external operation. Master owns landing, archive, and issue closure.

- [x] 2.8
