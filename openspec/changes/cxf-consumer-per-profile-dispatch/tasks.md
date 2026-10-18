# Tasks: cxf-consumer-per-profile-dispatch

Single phase. Order matters: each task compiles and its tests pass on top of
the previous tasks. Commit subjects are owned by caveman-commit and are not
specified here. Run every cargo and Gradle command from the feature worktree,
never from the main checkout (AGENTS.md).

NEW symbols introduced by this change (names are binding for every task):

| Symbol | Kind | Introduced in |
| --- | --- | --- |
| `org.rustcamel.cxf.ConsumerProfileInterceptor` | Java class, `@ApplicationScoped`, implements `io.grpc.ServerInterceptor` | 1.1 |
| `ConsumerProfileInterceptor.PROFILE_METADATA_KEY` | `static final Metadata.Key<String>` = `Metadata.Key.of("x-cxf-profile", Metadata.ASCII_STRING_MARSHALLER)` | 1.1 |
| `ConsumerProfileInterceptor.PROFILE_CONTEXT_KEY` | `static final io.grpc.Context.Key<String>` = `Context.key("cxf-consumer-profile")` | 1.1 |
| `CxfServerManager.consumerRequestObservers` | `Map<String, StreamObserver<ConsumerRequest>>` (a `HashMap`, guarded by `lock`) | 1.2 |
| `CxfServerManager.setConsumerRequestObserver(String profile, StreamObserver<ConsumerRequest> observer)` | replaces the 1-arg method | 1.2 |
| `CxfServerManager.hasConsumerObserver(String profile)` | replaces the 0-arg method | 1.2 |
| `CxfServerManager.onConsumerStreamClosed(String profile, StreamObserver<ConsumerRequest> observer)` | replaces the 1-arg method | 1.2 |
| `CxfBridgeService.REJECTED_STREAM_OBSERVER` | `private static final StreamObserver<ConsumerResponse>` no-op | 1.4 |
| `ConsumerProfileGrpcTest` | Java `@QuarkusTest` class | 1.5 |
| `camel_component_cxf::consumer::CXF_PROFILE_METADATA_KEY` | `pub const &str = "x-cxf-profile"` | 2.1 |
| `MockState.open_stream_profiles` / `MockState.open_stream_recorded` / `wait_for_open_count` | test support | 2.1 |
| `MockState.open_stream_rejections` | test support, `Arc<Mutex<VecDeque<tonic::Code>>>` | 2.2 |
| `crates/components/camel-cxf/tests/consumer_profile_test.rs` | new integration test target | 2.1 |
| `RegistrationToken` | `pub(crate)` newtype `struct RegistrationToken(u64)` in `pool.rs` | 2.4 |
| `ConsumerProfileRegistry` | `pub(crate)` struct in `pool.rs` | 2.4 |
| `ConsumerProfileGuard` | `pub(crate)` RAII struct in `pool.rs` | 2.4 |
| `CxfBridgePool.consumer_profiles` | `pub(crate) Arc<ConsumerProfileRegistry>` field | 2.4 |
| `CxfBridgePool::register_consumer_profile` | `pub(crate) fn register_consumer_profile(&self, profile: &str) -> Result<ConsumerProfileGuard, CamelError>` | 2.4 |
| `CxfBridgePool::consumer_profile_active_for_test` | `#[cfg(any(test, feature = "test-util"))] pub fn consumer_profile_active_for_test(&self, profile: &str) -> bool` | 2.4 |
| `CxfConsumer.profile_guard` | `Option<ConsumerProfileGuard>` field | 2.5 |

Java test command prefix used below (repo convention):
`bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.ClassName"`.

Rust test rules for every new test: no `tokio::time::sleep` / `std::thread::sleep`
in a test body (`lint-test-sleep`); every `.await` on a JoinHandle, channel
`recv`, `Notify`, `Mutex::lock`, `stop()` or open-count wait is wrapped in
`tokio::time::timeout(..)` or goes through a deadline-bounded helper
(`wait_for_open_count`, `acquire_deadline`, `support::wait::wait_until`), so
`cargo xtask lint-unbounded-wait` stays at or below the 296 ratchet.

## Java bridge (`bridges/cxf`)

### Task 1.1: Add `ConsumerProfileInterceptor` copying `x-cxf-profile` into a gRPC Context key

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/ConsumerProfileInterceptor.java` (new)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/ConsumerProfileInterceptorTest.java` (new)

**Steps:**
1. Create `ConsumerProfileInterceptor` annotated `@jakarta.enterprise.context.ApplicationScoped`, implementing `io.grpc.ServerInterceptor`. Declare `static final Metadata.Key<String> PROFILE_METADATA_KEY = Metadata.Key.of("x-cxf-profile", Metadata.ASCII_STRING_MARSHALLER);` and `static final Context.Key<String> PROFILE_CONTEXT_KEY = Context.key("cxf-consumer-profile");`.
2. Implement `interceptCall(ServerCall<ReqT, RespT> call, Metadata headers, ServerCallHandler<ReqT, RespT> next)`: when `call.getMethodDescriptor().getFullMethodName()` does not equal `CxfBridgeGrpc.getOpenConsumerStreamMethod().getFullMethodName()`, return `next.startCall(call, headers)` unchanged. Otherwise read `headers.get(PROFILE_METADATA_KEY)` (may be `null`), build `Context.current().withValue(PROFILE_CONTEXT_KEY, value)` and return `Contexts.interceptCall(ctx, call, headers, next)`. No validation here: the handler (Task 1.4) owns the decision.
3. Javadoc: the interceptor only transports the announced profile; matching is exact and case-sensitive in `CxfBridgeService.openConsumerStream`. Do not register it on the service yet (Task 1.4 does).

**Tests:** (JUnit 5 + Mockito, no Quarkus)
- `copiesProfileHeaderIntoContext`: setup: Mockito `ServerCall` whose `getMethodDescriptor()` returns `CxfBridgeGrpc.getOpenConsumerStreamMethod()`; `Metadata` with `x-cxf-profile: v11`; a `ServerCallHandler` lambda that stores `ConsumerProfileInterceptor.PROFILE_CONTEXT_KEY.get()` into an `AtomicReference` and returns `new ServerCall.Listener<>() {}` → action: `interceptCall` → assert: stored value equals `"v11"` and the handler ran exactly once. expected: fails to compile before the class exists, passes after.
- `missingHeaderYieldsNullProfile`: same setup with empty `Metadata` → assert stored value is `null`.
- `otherMethodsPassThroughWithoutContext`: method descriptor `CxfBridgeGrpc.getHealthMethod()`, header `x-cxf-profile: v11` → assert handler ran once and stored value is `null`.
- command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.ConsumerProfileInterceptorTest"`

**Acceptance:**
- The command above passes.
- `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck` exits 0.

- [x] 1.1

### Task 1.2: Key `CxfServerManager` consumer observers by profile

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfServerManager.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfBridgeService.java` (modified, call sites only)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java` (modified, gate call site only)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/CxfServerManagerTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherTest.java` (modified, mock stubs and `lateStreamLossYieldsSame503`)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherBodyCapTest.java` (modified, mock stub only)

**Steps:**
1. In `CxfServerManager`, replace the field `private StreamObserver<ConsumerRequest> consumerRequestObserver;` (line 89) with `private final Map<String, StreamObserver<ConsumerRequest>> consumerRequestObservers = new HashMap<>();` guarded by the existing `lock`. Rewrite its Javadoc (lines 84-88): one live observer per profile; replaced per key; cleared on close only when the key still maps to the closing observer.
2. Replace `setConsumerRequestObserver(StreamObserver)` (line 201) with `setConsumerRequestObserver(String profile, StreamObserver<ConsumerRequest> observer)`: throw `IllegalArgumentException("consumer profile is required")` when `profile` is `null` or empty; otherwise `consumerRequestObservers.put(profile, observer)` under `lock`. Pending registrations stay untouched (same contract as today).
3. Replace `hasConsumerObserver()` (line 208) with `hasConsumerObserver(String profile)`: under `lock`, `profile != null && consumerRequestObservers.containsKey(profile)`.
4. Replace `onConsumerStreamClosed(StreamObserver)` (line 223) with `onConsumerStreamClosed(String profile, StreamObserver<ConsumerRequest> observer)`: under `lock`, `consumerRequestObservers.remove(profile, observer)` (conditional remove; replaces the `==` check at line 226); the victim selection loop (`pending.owner == observer`) and the out-of-lock completion with `StreamUnavailableException("consumer stream closed")` stay as they are.
5. In `handleSoapRequest` (line 152), replace `observer = consumerRequestObserver;` with `observer = consumerRequestObservers.get(request.getSecurityProfile());` and the rejection message at line 154 with `"no consumer stream for profile " + request.getSecurityProfile()`. The capacity counter, the duplicate `request_id` check and `PendingRequest.owner` stay as they are. No fallback to any other key.
6. In `cleanup()` (line 264), replace `consumerRequestObserver = null;` with `consumerRequestObservers.clear();`. Add `import java.util.HashMap;`.
7. Interim call sites in `CxfBridgeService.openConsumerStream` (lines 305, 316, 321): read `String profile = ConsumerProfileInterceptor.PROFILE_CONTEXT_KEY.get();` as the first statement, pass `profile` to `setConsumerRequestObserver(profile, responseObserver)` and to both `onConsumerStreamClosed(profile, responseObserver)` calls. Validation and the `INVALID_ARGUMENT` answer come in Task 1.4.
8. In `SoapEndpointPublisher.handleRequestBody` line 605, replace `cxfServerManager.hasConsumerObserver()` with `cxfServerManager.hasConsumerObserver(profileName)` and update the comment at lines 599-604 to say the gate checks the consumer stream of the addressed profile.
9. Mechanical test stub updates: `SoapEndpointPublisherTest` line 116 and `SoapEndpointPublisherBodyCapTest` line 94 become `when(cxfServerManager.hasConsumerObserver(anyString())).thenReturn(true);`; `SoapEndpointPublisherTest` lines 1437, 1478, 2276 become `hasConsumerObserver(TEST_PROFILE_NAME)).thenReturn(false)` and line 2282 `hasConsumerObserver(TEST_PROFILE_NAME)).thenReturn(true)`.
10. Rewrite `SoapEndpointPublisherTest.lateStreamLossYieldsSame503` per profile: lines 1535-1536 become `manager.setConsumerRequestObserver(TEST_PROFILE_NAME, observerB); manager.onConsumerStreamClosed(TEST_PROFILE_NAME, observerA);`, line 1542 `manager.setConsumerRequestObserver(TEST_PROFILE_NAME, observerA);`, line 1607 `assertTrue(manager.hasConsumerObserver(TEST_PROFILE_NAME), "replacement stream B must stay live");`.
11. In `CxfServerManagerTest`, add helper `private static ConsumerRequest request(String profile, String id)` that sets both `setRequestId(id)` and `setSecurityProfile(profile)`, remove the 1-arg `request(String id)`, and make `response(String id)` set `setSecurityProfile("v11")`.

**Tests:** (all in `CxfServerManagerTest`; setUp keeps `maxConcurrentRequests()` = 2)
- REWRITE `replacedStreamCloseFailsOnlyItsPendingRequests`: every `setConsumerRequestObserver` / `onConsumerStreamClosed` call gains profile `"v11"`, every request is `request("v11", id)`, every `hasConsumerObserver()` becomes `hasConsumerObserver("v11")`. All existing assertions stay (A fails typed, B live and pending, capacity exact at max=2, B completes only B).
- REWRITE `reconnectDuringPendingCleanupPreservesNewRequests` (scenario "Reconnect of one profile keeps new requests"): same profile rewrite on `"v11"`; add `verify(observerB).onNext(argThat(r -> "d".equals(r.getRequestId())))` and `verify(observerA, never()).onNext(argThat(r -> "d".equals(r.getRequestId())))`; keep `assertTrue(manager.hasConsumerObserver("v11"))`, `assertFalse(futureD.isDone())` and the `StreamUnavailableException` root-cause assertion on `futureA`.
- command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.CxfServerManagerTest" --tests "org.rustcamel.cxf.SoapEndpointPublisherTest" --tests "org.rustcamel.cxf.SoapEndpointPublisherBodyCapTest"`
- expected: the rewritten tests fail to compile before the change; all pass after.

**Acceptance:**
- The command above passes, including the existing `preStream503IsTransportAvailabilityNotFault`, `availabilityCheckedBeforeWssForSignatureProfile` and the rewritten `lateStreamLossYieldsSame503` (scenario "Late stream loss yields the same 503").
- `grep -n "consumerRequestObserver\b" bridges/cxf/src/main/java/org/rustcamel/cxf/CxfServerManager.java` returns no hit (only `consumerRequestObservers`).
- `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck` exits 0.

- [x] 1.2

### Task 1.3: Per-profile `CxfServerManager` regression tests

**Files:**
- `bridges/cxf/src/test/java/org/rustcamel/cxf/CxfServerManagerTest.java` (modified)

**Steps:**
1. Add the five tests below to `CxfServerManagerTest`, using the `request(String profile, String id)` and `response(String id)` helpers from Task 1.2 step 11. No production change.

**Tests:** (setUp keeps `maxConcurrentRequests()` = 2)
- NEW `secondProfileStreamDoesNotReplaceFirst` (scenario "Two profiles keep two streams"): setup: mocks A, B → action: `setConsumerRequestObserver("v11", A)`, then `setConsumerRequestObserver("v12", B)`, then `handleSoapRequest(request("v11", "x"))` → assert: `hasConsumerObserver("v11")` and `hasConsumerObserver("v12")` are true, `verify(A).onNext(any())` once, `verify(B, never()).onNext(any())`.
- NEW `requestDispatchesOnlyToItsProfileStream` (scenario "Request goes to its own route"): A on v11, B on v12 → `handleSoapRequest(request("v11", "r1"))` and `handleSoapRequest(request("v12", "r2"))` → assert with `ArgumentCaptor<ConsumerRequest>`: A received exactly `["r1"]`, B received exactly `["r2"]`.
- NEW `missingProfileStreamFailsWithoutFallback` (scenario "No cross-profile fallback"): only B on v12 → `handleSoapRequest(request("v11", "r"))` → assert: future completed exceptionally, root cause `StreamUnavailableException` whose message contains `"v11"`, `verify(B, never()).onNext(any())`; then `handleSoapRequest(request("v12", "s1"))` and `handleSoapRequest(request("v12", "s2"))` are both admitted (`assertFalse(isDone())`), proving the rejected v11 request released its capacity slot.
- NEW `closingOneProfileFailsOnlyItsRequests` (scenario "Closing one profile does not fail another"): A on v11 with pending `"a"`, B on v12 with pending `"b"` → `onConsumerStreamClosed("v11", A)` → assert: `"a"` fails with `StreamUnavailableException`, `"b"` is not done, `hasConsumerObserver("v11")` false, `hasConsumerObserver("v12")` true; `completeFromConsumer(response("b"))` completes `"b"`.
- NEW `cleanupClearsEveryProfile`: A on v11, B on v12 → `cleanup()` → both `hasConsumerObserver` calls return false.
- command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.CxfServerManagerTest"`
- expected: passes on top of Task 1.2 (each test exercises behaviour Task 1.2 introduced).

**Acceptance:**
- The command above passes.
- `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck` exits 0.

- [x] 1.3

### Task 1.4: Reject `OpenConsumerStream` with `INVALID_ARGUMENT` for a missing, empty or unconfigured profile

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfBridgeService.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/CxfBridgeServiceTest.java` (modified)

**Steps:**
1. Annotate `CxfBridgeService` (line 24-26) with `@io.quarkus.grpc.RegisterInterceptor(ConsumerProfileInterceptor.class)` next to `@GrpcService @Blocking`.
2. Add `private static final StreamObserver<ConsumerResponse> REJECTED_STREAM_OBSERVER` whose `onNext`, `onError`, `onCompleted` do nothing.
3. In `openConsumerStream` (line 303), after reading `profile` from `ConsumerProfileInterceptor.PROFILE_CONTEXT_KEY.get()`: if `profile == null`, or `profile.isEmpty()`, or `!profileStore.profiles().containsKey(profile)` (exact, case-sensitive map lookup), then (a) `LOG.log(Level.WARNING, "Rejected OpenConsumerStream: {0}", description)`, (b) `responseObserver.onError(Status.INVALID_ARGUMENT.withDescription(description).asException())`, (c) `return REJECTED_STREAM_OBSERVER;`. `description` is `"x-cxf-profile must name a configured consumer profile; got no value"` for `null`, `"; got an empty value"` suffix for `""`, and `"; got '" + profile + "'"` otherwise. The rejected path MUST NOT call `serverManager.setConsumerRequestObserver` or any other `serverManager` method.
4. The accepted path keeps Task 1.2 behaviour: `setConsumerRequestObserver(profile, responseObserver)` and the returned observer calls `completeFromConsumer` / `onConsumerStreamClosed(profile, responseObserver)`.

**Tests:** (in `CxfBridgeServiceTest`; setup: `CxfServerManager real = new CxfServerManager(); real.bridgeConfig = bridgeConfig; real.endpointPublisher = mock(SoapEndpointPublisher.class); CxfServerManager manager = spy(real); service.serverManager = manager;`, `when(bridgeConfig.maxConcurrentRequests()).thenReturn(4)`, `when(profileStore.profiles()).thenReturn(Map.of("v11", mock(SecurityProfile.class)))`; call `manager.cleanup()` at test end)
- `openConsumerStreamRejectsMissingEmptyMiscasedAndUnknownProfile` (scenario "Open without a profile is rejected"): setup: register `v11Observer` via `manager.setConsumerRequestObserver("v11", v11Observer)` then `clearInvocations(manager)` → action: for each case value `null` (call outside any Context), `""`, `"V11"`, `"v99"` run `Context.current().withValue(ConsumerProfileInterceptor.PROFILE_CONTEXT_KEY, value).call(() -> service.openConsumerStream(rejected))` with a fresh mock `rejected` → assert per case: `ArgumentCaptor<Throwable>` on `verify(rejected).onError(captor)` gives `Status.fromThrowable(t).getCode() == Status.Code.INVALID_ARGUMENT`; `verify(rejected, never()).onNext(any())`; the returned observer is non-null and calling its `onNext(ConsumerResponse.getDefaultInstance())` and `onCompleted()` does not invoke the manager; after all cases `verify(manager, never()).setConsumerRequestObserver(anyString(), any())`, `verify(manager, never()).onConsumerStreamClosed(anyString(), any())`, `manager.hasConsumerObserver("V11")` and `("v99")` and `("")` are false, `manager.hasConsumerObserver("v11")` is true, and `manager.handleSoapRequest(ConsumerRequest.newBuilder().setRequestId("k").setSecurityProfile("v11").build())` reaches `v11Observer` (`verify(v11Observer).onNext(any())`).
- `openConsumerStreamRegistersConfiguredProfile`: Context value `"v11"` → assert `manager.hasConsumerObserver("v11")` true and `verify(manager).setConsumerRequestObserver(eq("v11"), same(responseObserver))`; then the returned observer's `onCompleted()` → `hasConsumerObserver("v11")` false.
- `serviceRegistersConsumerProfileInterceptor`: assert `CxfBridgeService.class.getAnnotationsByType(RegisterInterceptor.class)` contains an entry whose `value()` is `ConsumerProfileInterceptor.class`.
- command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.CxfBridgeServiceTest"`
- expected: first and third test fail before the change, all pass after.

**Acceptance:**
- The command above passes.
- `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck` exits 0.

- [x] 1.4

### Task 1.5: REQUIRED real-gRPC `@QuarkusTest` proving the Context key crosses `@Blocking`

**Files:**
- `bridges/cxf/src/test/java/org/rustcamel/cxf/ConsumerProfileGrpcTest.java` (new)

**Steps:**
1. Create `@QuarkusTest class ConsumerProfileGrpcTest` (precedent: `OtelSpanFlowTest.grpcServerSpanContinuesPropagatedTraceparent`, `ManagedChannelBuilder.forAddress("localhost", 9001).usePlaintext()` plus `MetadataUtils.newAttachHeadersInterceptor`). Inject `@Inject CxfServerManager serverManager;`.
2. `@BeforeEach`: profiles come from environment variables, so install a profile store mock: `SecurityProfileStore store = Mockito.mock(SecurityProfileStore.class); when(store.profiles()).thenReturn(Map.of("v11", SecurityProfile.builder("v11").build())); QuarkusMock.installMockForType(store, SecurityProfileStore.class);`.
3. Open streams with the async stub `CxfBridgeGrpc.newStub(channel).withInterceptors(MetadataUtils.newAttachHeadersInterceptor(headers)).openConsumerStream(inbound)`, where `inbound` is a `StreamObserver<ConsumerRequest>` completing a `CompletableFuture<Throwable>` from `onError`.
4. Wait helper `awaitTrue(BooleanSupplier cond, Duration budget)`: loop until `cond` is true or `System.nanoTime()` passes the deadline, parking 20 ms between checks with `LockSupport.parkNanos`; returns the final value. No unbounded wait.
5. Every test closes its client stream with `onCompleted()` and calls `channel.shutdownNow().awaitTermination(5, TimeUnit.SECONDS)` in `finally`.
6. If `streamRegistersUnderAnnouncedProfile` fails because the handler sees a `null` profile on the worker thread, stop and file a bd issue (`discovered-from:rc-gsy1v`): the design assumption that a gRPC `Context` set by the interceptor reaches the `@Blocking` handler is then false and needs a design change, not a local workaround.

**Tests:**
- `streamRegistersUnderAnnouncedProfile` (scenario "Metadata reaches the handler over real gRPC"): setup: running Quarkus app, mocked store with `v11` → action: open a stream with `x-cxf-profile: v11` → assert: `awaitTrue(() -> serverManager.hasConsumerObserver("v11"), 5 s)` is true; after `requestObserver.onCompleted()`, `awaitTrue(() -> !serverManager.hasConsumerObserver("v11"), 5 s)` is true.
- `miscasedProfileIsRejectedOverRealGrpc` (scenario "Open without a profile is rejected", real transport): first open `v11` and await registration as above; then open a second stream with `x-cxf-profile: V11` → assert: its error future completes within 5 s with `Status.fromThrowable(t).getCode() == Status.Code.INVALID_ARGUMENT`, and `serverManager.hasConsumerObserver("v11")` is still true afterwards.
- command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.ConsumerProfileGrpcTest"`
- expected: passes after Tasks 1.1-1.4.

**Acceptance:**
- The command above passes.
- `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck` exits 0.

- [x] 1.5

### Task 1.6: Publisher availability gate is per addressed profile

**Files:**
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherTest.java` (modified)

**Steps:**
1. The production change landed in Task 1.2 step 8. This task adds the per-profile regression tests next to `preStream503IsTransportAvailabilityNotFault` (line 1430).
2. Extend `availabilityCheckedBeforeWssForSignatureProfile` with `verify(cxfServerManager).hasConsumerObserver(TEST_PROFILE_NAME);` after the first 503 assertion.

**Tests:**
- `gateUsesAddressedProfileNotAnyProfile` (MODIFIED scenario "Request before the consumer stream returns 503 without a fault"): setup: `when(cxfServerManager.hasConsumerObserver(TEST_PROFILE_NAME)).thenReturn(false); when(cxfServerManager.hasConsumerObserver("other_profile")).thenReturn(true);` → action: `driveRequestOutcome(envelope11("<test/>"), "text/xml", SecurityProfile.builder(TEST_PROFILE_NAME).build(), completedOk())` → assert: status 503, `verify(httpResponse).putHeader("Retry-After", "1")`, body contains `"routes unavailable"` and neither `"Envelope"` nor `"Fault"`, `verify(cxfServerManager).hasConsumerObserver(TEST_PROFILE_NAME)`, `verify(cxfServerManager, never()).handleSoapRequest(any())`.
- `noCrossProfileFallbackWithRealManager` (ADDED scenario "No cross-profile fallback", HTTP part): setup: `Vertx real = Vertx.vertx(); SoapEndpointPublisher pub = publisherWiredWith(real); CxfServerManager manager = new CxfServerManager(); manager.bridgeConfig = bridgeConfig; manager.endpointPublisher = pub; pub.cxfServerManager = manager;` `when(bridgeConfig.maxConcurrentRequests()).thenReturn(10); when(bridgeConfig.address()).thenReturn("http://127.0.0.1:0/soap");` and `manager.setConsumerRequestObserver("other_profile", otherObserver)` → action: `pub.publish()` then `postPlain(pub.soapPort().getAsInt(), envelope11("<test/>"))` (posts to `/cxf/test_profile`) → assert: status 503, `retry-after` header present, body has no `"Envelope"`, `verify(otherObserver, never()).onNext(any())`. `finally`: `manager.cleanup()` (stops the publisher) and `real.close().toCompletionStage().toCompletableFuture().get(5, TimeUnit.SECONDS)`.
- EXTENDED `availabilityCheckedBeforeWssForSignatureProfile` (MODIFIED scenario "Availability is checked before WS-Security"): existing assertions plus the profile-scoped `verify` from step 2.
- command: `bridges/cxf/gradlew -p bridges/cxf test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"`
- expected: passes.

**Acceptance:**
- The command above passes.
- `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck` exits 0.

- [x] 1.6

## Rust (`crates/components/camel-cxf`)

### Task 2.1: Consumer announces its profile as `x-cxf-profile` metadata

**Files:**
- `crates/components/camel-cxf/src/consumer.rs` (modified)
- `crates/components/camel-cxf/tests/support/mock_bridge.rs` (modified)
- `crates/components/camel-cxf/tests/consumer_profile_test.rs` (new)

**Steps:**
1. In `consumer.rs` add `pub const CXF_PROFILE_METADATA_KEY: &str = "x-cxf-profile";` with a doc comment naming the bridge handler `CxfBridgeService.openConsumerStream`.
2. Replace the doc comment and attribute on `profile_name` (lines 21-23): remove `#[allow(dead_code)]` and write: the profile this consumer serves; sent as `x-cxf-profile` metadata on every `OpenConsumerStream` so the bridge registers the stream under this profile and dispatches to it only requests addressed to this profile's path; also the key of the duplicate-consumer registration.
3. At the top of `start()` (line 179), build `let profile_header: tonic::metadata::AsciiMetadataValue = self.profile_name.parse().map_err(|e| CamelError::Config(format!("cxf consumer profile '{}' is not a valid gRPC metadata value: {e}", self.profile_name)))?;` and before `tokio::spawn` clone `let profile_name = self.profile_name.clone();` into the task.
4. Replace line 259 `client.open_consumer_stream(response_stream).await` with a `tonic::Request::new(response_stream)` whose `metadata_mut().insert(CXF_PROFILE_METADATA_KEY, profile_header.clone())` is set, then `client.open_consumer_stream(open_request).await`. Every reconnect iteration sends the header.
5. In `mock_bridge.rs` add to `MockState` (each with `#[allow(dead_code)]`, mirroring `response_recorded`): `pub open_stream_profiles: Arc<Mutex<Vec<Option<String>>>>` and `pub open_stream_recorded: Arc<Notify>`. In `open_consumer_stream` (line 156), before `request.into_inner()`, push `request.metadata().get("x-cxf-profile").and_then(|v| v.to_str().ok()).map(str::to_string)` and call `open_stream_recorded.notify_one()`.
6. In `mock_bridge.rs` add `#[allow(dead_code)] pub async fn wait_for_open_count(state: &MockState, n: usize, budget: Duration) -> Result<Vec<Option<String>>, String>`: compute a deadline; loop: read a clone of `open_stream_profiles` under `tokio::time::timeout(remaining, lock())`, return it when `len() >= n`, return `Err` when the deadline passed, otherwise `tokio::time::timeout(remaining, state.open_stream_recorded.notified())`.
7. Create `tests/consumer_profile_test.rs` with `mod support;` and helpers: `async fn mock_pool(profiles: Vec<CxfProfileConfig>) -> Result<(Arc<CxfBridgePool>, MockState), BoxErr>` (pattern of `consumer_emission_test.rs::start_consumer` lines 54-68: `spawn_mock_bridge`, channel connect under a 5 s timeout, `BridgeSlot::new_ready_for_test`, `insert_slot_for_test(CxfBridgePool::slot_key(), slot)`, with `reconnect: NetworkRetryPolicy { initial_delay: Duration::from_millis(10), max_delay: Duration::from_millis(50), jitter_factor: 0.0, ..NetworkRetryPolicy::default() }`), `fn profile_config(name: &str) -> CxfProfileConfig` (wsdl `/wsdl/hello.wsdl`, service `HelloService`, port `HelloPort`, address `None`, default security), and `fn consumer_ctx(route_id: &str) -> (ConsumerContext, mpsc::Receiver<ExchangeEnvelope>, CancellationToken)`.

**Tests:**
- `consumer_sends_profile_metadata_on_open`: setup: `mock_pool(vec![])`, `CxfConsumer::new(pool, "v11".into(), Arc::new(NoopRuntimeObservability))` → action: `start(ctx)` → assert: `wait_for_open_count(&state, 1, Duration::from_secs(5))` returns `vec![Some("v11".to_string())]`; then `tokio::time::timeout(Duration::from_secs(10), consumer.stop())` is `Ok(Ok(()))`. expected: fails before step 4 (recorded `None`), passes after.
- Existing `consumer.rs` unit test `consumer_new` still asserts `profile_name == "baleares"`.
- command: `cargo test -p camel-component-cxf --test consumer_profile_test` and `cargo test -p camel-component-cxf --lib consumer::tests`

**Acceptance:**
- Both commands pass; `cargo test -p camel-component-cxf --tests` passes (shared `support` module still compiles in every test binary).
- `grep -n "allow(dead_code)" crates/components/camel-cxf/src/consumer.rs` returns no hit.

- [x] 2.1

### Task 2.2: Treat `INVALID_ARGUMENT` on open as terminal (one `error!`, no retry, task returns `Ok(())`)

**Files:**
- `crates/components/camel-cxf/src/consumer.rs` (modified)
- `crates/components/camel-cxf/tests/support/mock_bridge.rs` (modified)
- `crates/components/camel-cxf/tests/consumer_profile_test.rs` (modified)

**Steps:**
1. In the `Err(e)` arm of `match stream_result` (`consumer.rs` line 275), insert first: `if e.code() == tonic::Code::InvalidArgument { // log-policy: system-broken` (comment on its own line directly above the macro) `error!(profile = %profile_name, "CXF bridge rejected consumer profile with INVALID_ARGUMENT; consumer stops without reconnecting: {}", e.message()); return Ok(()); }`. Do not return `Err`: the runtime would log a second error and supervision would restart the route. The transport retry branch below stays for every other code.
2. In `mock_bridge.rs` add `#[allow(dead_code)] pub open_stream_rejections: Arc<Mutex<VecDeque<tonic::Code>>>` to `MockState`. In `open_consumer_stream`, after recording the profile (Task 2.1 step 5) and before spawning the inbound reader or touching `consumer_requests_tx`, `if let Some(code) = state.open_stream_rejections.lock().await.pop_front() { return Err(Status::new(code, "mock bridge rejected OpenConsumerStream")); }`.

**Tests:** (in `consumer_profile_test.rs`, `#[tokio::test]` current-thread flavor so the spawned consumer task logs on the test thread)
- `invalid_argument_on_open_is_terminal` (scenario "Consumer does not retry a rejected profile"): setup: `mock_pool(vec![])`, push `tonic::Code::InvalidArgument` into `open_stream_rejections`; copy `CapturingWriter`, `ensure_global_tracing_default` and `capture_sink` from `src/pool_env_test.rs` lines 156-215 into the test file, call `ensure_global_tracing_default()` before `set_default` (convention: `docs/testing/tracing-capture-guards.md`), then `let (sink, subscriber) = capture_sink(); let _guard = tracing::subscriber::set_default(subscriber);` → action: start consumer `v11`, take `background_task_handle()`, `tokio::time::timeout(Duration::from_secs(5), handle)` → assert: outcome is `Ok(Ok(Ok(())))`; `open_stream_profiles`, read through `acquire_deadline(&state.open_stream_profiles, "open_stream_profiles", Duration::from_secs(5))`, equals `vec![Some("v11".into())]` (the task already exited, so no later reconnect is possible); captured output (the `sink` bytes as UTF-8) has exactly one line containing `ERROR`, and that line contains `v11` and `INVALID_ARGUMENT`.
- `transport_unavailable_on_open_still_retries`: push `tonic::Code::Unavailable` → start consumer `v11` → assert `wait_for_open_count(&state, 2, Duration::from_secs(5))` returns two `Some("v11")` entries; stop under a 10 s timeout.
- command: `cargo test -p camel-component-cxf --test consumer_profile_test`
- expected: `invalid_argument_on_open_is_terminal` fails before step 1 (second open recorded / handle does not finish), passes after.

**Acceptance:**
- The command above passes.
- `cargo xtask lint-log-levels` exits 0 (the new `error!` carries `// log-policy: system-broken`).
- `cargo xtask lint-log-redaction` exits 0.

- [x] 2.2

### Task 2.3: Supervised route with a rejected profile records no crash and no restart

**Files:**
- `crates/components/camel-cxf/Cargo.toml` (modified: `[dev-dependencies]` adds `camel-core = { workspace = true }`, precedent `crates/components/camel-http/Cargo.toml` dev-deps)
- `crates/components/camel-cxf/tests/consumer_profile_test.rs` (modified)

**Steps:**
1. Add the `camel-core` dev-dependency with the comment used in camel-http: publish-order-safe only while camel-core does not depend back on camel-cxf (ADR-0055).
2. Add the test below, built on `CamelContext::builder().supervision(SupervisionConfig)` as in `crates/camel-core/tests/runtime_consistency_test.rs::supervision_restart_reflects_crash_recovery_progress_in_runtime_projection`.

**Tests:**
- `rejected_profile_does_not_crash_or_restart_route` (scenario "Rejected profile does not restart the route"): setup: `mock_pool(vec![profile_config("v11")])`; push one `tonic::Code::InvalidArgument` (a restart would open again and be accepted and recorded); `CamelContext::builder().supervision(SupervisionConfig { max_attempts: Some(3), initial_delay: Duration::from_millis(10), backoff_multiplier: 1.0, max_delay: Duration::from_millis(100) }).build().await?`; `register_component(CxfComponent::new(Arc::clone(&pool)))`; `add_route_definition(RouteDefinition::new("cxf://http://127.0.0.1:1/cxf/v11?wsdl=/wsdl/hello.wsdl&service=HelloService&port=HelloPort&profile=v11", vec![]).with_route_id("cxf-rejected-r1"))` → action: `ctx.start()` → assert: `wait_for_open_count(&state, 1, Duration::from_secs(5))` is `Ok`; `wait_for_open_count(&state, 2, Duration::from_millis(750))` is `Err` (no restart within 75 supervision delays); `RuntimeQuery::GetRouteStatus { route_id: "cxf-rejected-r1".into() }` returns a status that is not `"Failed"`; `ctx.stop()` under a 10 s timeout succeeds.
- command: `cargo test -p camel-component-cxf --test consumer_profile_test rejected_profile_does_not_crash_or_restart_route`
- expected: passes after Task 2.2; would fail (second open recorded) if the terminal path returned `Err`.

**Acceptance:**
- The command above passes.
- `cargo xtask lint-component-deps` and `cargo xtask lint-publish-cycles` exit 0.

- [x] 2.3

### Task 2.4: Pool-level consumer profile registry with RAII guard and registration token

**Files:**
- `crates/components/camel-cxf/src/pool.rs` (modified)

**Steps:**
1. Add a section `// ── Consumer profile registry ──` above `// ── CxfBridgePool ──` (line 224) with:
   - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub(crate) struct RegistrationToken(u64);`
   - `#[derive(Debug, Default)] pub(crate) struct ConsumerProfileRegistry { active: DashMap<String, RegistrationToken>, next_token: AtomicU64 }`
   - `impl ConsumerProfileRegistry`: `pub(crate) fn register(self: &Arc<Self>, profile: &str) -> Result<ConsumerProfileGuard, CamelError>` that mints `RegistrationToken(self.next_token.fetch_add(1, Ordering::Relaxed) + 1)` and uses `self.active.entry(profile.to_string())`: `Occupied` returns `CamelError::Config(format!("cxf consumer profile '{profile}' already has an active consumer route in this pool; one consumer route per profile is supported"))`, `Vacant` inserts the token and returns the guard; `#[cfg(any(test, feature = "test-util"))] pub(crate) fn is_active(&self, profile: &str) -> bool` (the crate defines the `test-util` feature; the cfg keeps `clippy -D warnings` free of `dead_code`); private `fn release(&self, profile: &str, token: RegistrationToken) -> bool` implemented as `self.active.remove_if(profile, |_, t| *t == token).is_some()`.
   - `pub(crate) struct ConsumerProfileGuard { registry: Arc<ConsumerProfileRegistry>, profile: String, token: RegistrationToken, released: bool }` with `#[cfg(test)] pub(crate) fn token(&self) -> RegistrationToken` (test-only, avoids `dead_code`) and `pub(crate) fn release(&mut self)` (no-op when `released`; else sets `released = true` and calls `registry.release(&self.profile, self.token)`), plus `impl Drop for ConsumerProfileGuard { fn drop(&mut self) { self.release(); } }`.
2. Add field `pub(crate) consumer_profiles: Arc<ConsumerProfileRegistry>` to `CxfBridgePool` (line 228) and initialise it with `Arc::new(ConsumerProfileRegistry::default())` in `from_config` (`pool.rs` line 247; add the field to its `Ok(Self {` literal at line 256). Import `std::sync::atomic::AtomicU64` and `dashmap::mapref::entry::Entry`.
3. Add `pub(crate) fn register_consumer_profile(&self, profile: &str) -> Result<ConsumerProfileGuard, CamelError>` delegating to `self.consumer_profiles.register(profile)`, and `#[cfg(any(test, feature = "test-util"))] pub fn consumer_profile_active_for_test(&self, profile: &str) -> bool` delegating to `is_active`.

**Tests:** (in `pool.rs` `mod tests`)
- `consumer_profile_registry_rejects_duplicate`: `register("v11")` Ok → second `register("v11")` is `Err(CamelError::Config(msg))` with `msg.contains("'v11'")` → `register("v12")` Ok.
- `consumer_profile_guard_drop_releases`: inside a block hold `register("v11")`; after the block `is_active("v11")` is false and `register("v11")` is Ok again.
- `consumer_profile_guard_release_is_idempotent_and_token_scoped`: `g1 = register("v11")`; `g1.release(); g1.release();` → `g2 = register("v11")` Ok with `g2.token() != g1.token()`; `registry.release("v11", g1.token())` returns false; `drop(g1)` → `is_active("v11")` still true; `drop(g2)` → false.
- command: `cargo test -p camel-component-cxf --lib pool::tests::consumer_profile`

**Acceptance:**
- The command above passes; `cargo xtask lint-unwrap` exits 0 (no new `unwrap()` outside test code).

- [x] 2.4

### Task 2.5: `CxfConsumer` owns the guard: register in `start()`, release synchronously in `stop()`, release on drop

**Files:**
- `crates/components/camel-cxf/src/consumer.rs` (modified)
- `crates/components/camel-cxf/tests/consumer_profile_test.rs` (modified)

**Steps:**
1. Add field `profile_guard: Option<ConsumerProfileGuard>` to `CxfConsumer` (line 19) with a doc comment: owned by the consumer struct, never by the spawned task, so a late exit of an old task cannot release a newer registration; dropping the consumer (runtime abort path in `camel-core` `consumer_management.rs`, which aborts the outer task after the stop budget without calling `stop()`) releases it through `Drop`. Initialise `None` in `new()`.
2. In `start()`, before the metadata parse of Task 2.1: `if let Some(mut stale) = self.profile_guard.take() { stale.release(); }` then `let guard = self.pool.register_consumer_profile(&self.profile_name)?;`. Keep `guard` a local until the end; every early return (`?` on the header parse, slot errors, `Degraded`, other states) drops it and so releases the registration before returning. Assign `self.profile_guard = Some(guard);` just before `ctx.mark_ready()` (line 462).
3. In `stop()` (line 467), as the first statement: `if let Some(mut guard) = self.profile_guard.take() { guard.release(); }` (synchronous, independent of the task handle).

**Tests:**
- `failed_start_releases_profile_registration` (in `consumer.rs` `mod tests`; scenario "Stopped route can start again", failed-start variant): setup: `test_pool()`, slot from `BridgeSlot::new_ready_for_test(Channel::from_static("http://127.0.0.1:1").connect_lazy())`, then `slot.state_tx.send_replace(BridgeState::Degraded("test".into()))`, `insert_slot_for_test` → action: `start(ctx)` on consumer `v11` → assert: `Err` whose text contains `CXF bridge not available`; `pool.consumer_profile_active_for_test("v11")` is false.
- `stopped_consumer_profile_can_start_again` (scenario "Stopped route can start again", stop variant): A start, A `stop()` under timeout → B (`v11`) start is Ok; stop B.
- `aborted_task_and_dropped_consumer_release_profile` (scenario "Stopped route can start again", aborted-task variant; RAII path): A start; `h = A.background_task_handle()`; `h.abort()`; `tokio::time::timeout(Duration::from_secs(5), h)` yields a cancelled `JoinError`; `drop(A)` without `stop()` → B (`v11`) start is Ok; stop B.
- `immediate_restart_after_abort_and_stop_keeps_new_registration` (scenario "Immediate restart after stop does not race the old task"): A start; `h = A.background_task_handle()`; `h.abort()` without awaiting; `A.stop()` under timeout; B (`v11`) start is Ok immediately; then the late exit: `tokio::time::timeout(Duration::from_secs(5), h)` and `drop(A)` → assert: `pool.consumer_profile_active_for_test("v11")` is true and C (`v11`) `start` returns `Err(CamelError::Config(m))` naming `'v11'`; stop B.
- command: `cargo test -p camel-component-cxf --test consumer_profile_test` and `cargo test -p camel-component-cxf --lib consumer::tests::failed_start_releases_profile_registration`
- expected: the race and restart tests fail before steps 1-3, all pass after; existing `consumer_unit_test.rs` and `consumer_emission_test.rs` still pass (`cargo test -p camel-component-cxf --tests`).

**Acceptance:**
- All commands above pass.
- `cargo xtask lint-test-sleep` and `cargo xtask lint-unbounded-wait` exit 0 (unbounded-wait count not above 296).

- [x] 2.5

### Task 2.6: Duplicate-profile start fails and the first consumer keeps receiving

**Files:**
- `crates/components/camel-cxf/tests/consumer_profile_test.rs` (modified)

**Steps:**
1. Add the test below; it relies on the guard wiring of Task 2.5 and the mock recording of Task 2.1. No production change.

**Tests:**
- `duplicate_profile_fails_at_start_and_first_keeps_receiving` (scenario "Duplicate profile"): A (`v11`) started, `wait_for_open_count(1)` Ok; B (`v11`) `start` → `Err(CamelError::Config(m))` with `m.contains("'v11'")`; spawn a reply task on A's envelope receiver that replies `Ok` with output body `<ok/>` (pattern `consumer_emission_test.rs::reply_handler`, receive under a 5 s timeout); get the request sender with a local copy of `consumer_emission_test.rs::wait_for_consumer_request_sender` (it reads `state.consumer_requests_tx` through `acquire_deadline` with a 5 s budget), register `state.expect_response_completion("dup-1")`, send `ConsumerRequest { request_id: "dup-1".into(), operation: String::new(), payload: b"<hello/>".to_vec(), headers: Default::default(), soap_action: String::new(), security_profile: "v11".into(), signer: None }`; await the completion under a 5 s timeout → assert: recorded response `request_id == "dup-1"` and `!fault`; `open_stream_profiles`, read through `acquire_deadline(&state.open_stream_profiles, "open_stream_profiles", Duration::from_secs(5))`, has `len() == 1` (B never opened). Stop A under a 10 s timeout.
- command: `cargo test -p camel-component-cxf --test consumer_profile_test duplicate_profile_fails_at_start_and_first_keeps_receiving`
- expected: passes on top of Task 2.5.

**Acceptance:**
- The command above passes.
- `cargo xtask lint-test-sleep` and `cargo xtask lint-unbounded-wait` exit 0 (unbounded-wait count not above 296).

- [x] 2.6

### Task 2.7: Native end-to-end: two profiles dispatch to their own routes; a profile without a stream gets 503

**Files:**
- `crates/camel-test/tests/cxf_test.rs` (modified)

**Steps:**
1. Add `fn two_profile_consumer_component(bind_port: u16) -> CxfComponent` next to `cxf_component_with_bind` (line 244): same pool fields, profiles `v11` and `v12` (both `cxf_wsdl_path()`, service `{http://example.com/hello}HelloService`, port `{http://example.com/hello}HelloPort`, address `http://localhost:8080/service`), `bind_address: Some(format!("http://127.0.0.1:{bind_port}/cxf"))`.
2. Add `async fn post_say_hello(port: u16, profile: &str) -> reqwest::Response` that POSTs the envelope literal used in `cxf_consumer_receives_request_and_returns_response` (lines 520-528) to `/cxf/` + `profile` with `soapaction: sayHello`, via `http_client()`.
3. Route YAML per profile: `from` `cxf://http://127.0.0.1:{port}/cxf/v11?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile=v11`, steps `set_body` constant `<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><hel:message>from-v11</hel:message></hel:sayHelloResponse>` and `to: "mock:v11"` (same shape for `v12`).

**Tests:** (each starts with `init_tracing(); install_crypto_provider(); let _binary = require_cxf_bridge_binary();`)
- `cxf_consumer_two_profiles_dispatch_to_own_route` (scenarios "Request goes to its own route" and "Two profiles keep two streams", end to end): routes for `v11` and `v12` → `wait_until("v11 and v12 streams connected", Duration::from_secs(30), Duration::from_millis(200), cond)` where `cond` returns `Ok(true)` once a POST to each profile returns 200 → then 3 POSTs per profile → assert: every 200 body for `v11` contains `from-v11` and not `from-v12`, and vice versa; every exchange in `mock:v11` has header `CxfSecurityProfile == "v11"` and every exchange in `mock:v12` has `"v12"`.
- `cxf_consumer_profile_without_stream_returns_503` (scenario "No cross-profile fallback", end to end): only the `v12` route → `wait_until` a POST to `v12` returns 200 → POST to `v11` → assert: status 503, `retry-after` header present, body does not contain `Envelope`; every exchange in `mock:v12` has `CxfSecurityProfile == "v12"`.
- command: `cd bridges/cxf && ./build-native.sh` then, from the worktree root, `CAMEL_CXF_BRIDGE_BINARY_PATH=$PWD/bridges/cxf/build/native/cxf-bridge cargo test -p camel-test --features integration-tests --test cxf_test cxf_consumer_profile -- --nocapture` and the same with filter `cxf_consumer_two_profiles`.
- expected: the first test fails against a bridge built before Tasks 1.1-1.4 (both profiles answered by the last-started route); both pass with the rebuilt bridge.

**Acceptance:**
- Both tests pass with the rebuilt native bridge.
- `cargo test -p camel-test --features integration-tests --test cxf_test -- --test-threads=1` passes in full with the rebuilt bridge (it now rejects opens without the header, so every existing consumer test must go through the updated crate).
- `cargo clippy -p camel-test --features integration-tests --tests -- -D warnings` exits 0.

- [x] 2.7

## Docs

### Task 3.1: Correct `ws-soap.md` (per-profile gate, per-profile streams, operator diagnosis)

**Files:**
- `docs/src/components/ws-soap.md` (modified)

**Steps:**
1. Line 260 table row becomes: `| No consumer stream for the addressed profile (pre-stream, or lost after the gate) | HTTP 503 + `Retry-After`, plaintext `routes unavailable` | Availability gate for the profile in the request path — before XML parsing and WS-Security |`.
2. Line 385 paragraph becomes: "Pre-stream availability gate: the gate is per profile. A request to a profile path whose consumer stream is not open receives HTTP 503 with a `Retry-After` header and a plaintext body (`routes unavailable`) — never a SOAP fault — even when another profile has a connected consumer. The same response serves a stream lost after the gate."
3. Directly after it add the operator-diagnosis line: "Diagnosis: if a profile keeps answering 503 while its route is started, check the log for `CXF bridge rejected consumer profile with INVALID_ARGUMENT`. The bridge rejected the profile the consumer announced (crate and bridge versions out of step, or a profile missing from the bridge configuration); the consumer logged that one error, stopped and does not reconnect. Fix the version pair or profile configuration, then restart the route."
4. In `## CXF consumer` (after the first paragraph, line 209) add: "Each consumer route serves one profile. The consumer opens its own gRPC stream and announces the profile as the `x-cxf-profile` call metadata; the bridge delivers a request only to the stream of the profile in its path and never falls back to another profile's stream. A pool accepts one consumer route per profile: a second route on the same profile fails at start with a configuration error naming the profile. Stopping and restarting a route is allowed."

**Tests:**
- `doc_row_names_profile`: `grep -n "No consumer stream for the addressed profile" docs/src/components/ws-soap.md` returns exactly one hit.
- `doc_diagnosis_present`: `grep -c "CXF bridge rejected consumer profile with INVALID_ARGUMENT" docs/src/components/ws-soap.md` prints `1`, and the same string appears in `crates/components/camel-cxf/src/consumer.rs` (`grep -c` prints `1`).
- `doc_no_bridge_wide_gate`: `grep -n "before a consumer stream is open receives" docs/src/components/ws-soap.md` returns no hit.

**Acceptance:**
- All three greps give the stated results.

- [x] 3.1

### Task 3.2: Correct `camel-cxf/CONTEXT.md` gate wording and record per-profile streams

**Files:**
- `crates/components/camel-cxf/CONTEXT.md` (modified)

**Steps:**
1. Lines 112-114: replace "Before a consumer stream is open, requests take the pre-stream gate" with "Before the consumer stream of the addressed profile is open, requests to that profile take the pre-stream gate" (rest of the sentence unchanged).
2. Append to the same paragraph: "Consumer streams are per profile: `CxfConsumer` announces its profile as `x-cxf-profile` metadata, the bridge dispatches by the profile in the request path with no cross-profile fallback and answers an unknown or missing profile with `INVALID_ARGUMENT` (terminal for the consumer, no reconnect), and the pool-level `ConsumerProfileGuard` rejects a second consumer route on one profile at start (rc-gsy1v)."
3. Do not cite bare `file.rs` plus line numbers (lint-context-citations Rule C).

**Tests:**
- `context_citations_clean`: `cargo xtask lint-context-citations` exits 0 (no `struct`/`fn`/`Type::item` citations are added).
- `context_gate_wording`: `grep -n "Before a consumer stream is open" crates/components/camel-cxf/CONTEXT.md` returns no hit.

**Acceptance:**
- Both checks pass.

- [x] 3.2

## Quality gates

### Task 4.1: Quality gates

**Files:**
- none (verification only; fix findings in the files of the owning task)

**Steps:**
1. Run, in the feature worktree, each command and fix every failure:
   - `cargo fmt --check --all`
   - `cargo clippy -p camel-component-cxf --all-targets --all-features -- -D warnings`
   - `cargo clippy -p camel-cli -- -D warnings`
   - `cargo clippy -p camel-cli --no-default-features --features flavor-regular,exec --all-targets -- -D warnings`
   - `cargo clippy -p camel-test --features integration-tests --tests -- -D warnings`
   - `cargo test -p camel-component-cxf --all-targets`
   - `cargo xtask lint-unwrap`
   - `cargo xtask lint-log-levels`
   - `cargo xtask lint-log-redaction`
   - `cargo xtask lint-test-sleep`
   - `cargo xtask lint-unbounded-wait` (must not exceed the 296 ratchet; new tests add no unbounded wait)
   - `cargo xtask lint-component-deps`
   - `cargo xtask lint-publish-cycles`
   - `cargo xtask lint-context-citations`
   - `cargo xtask lint-ignore`
   - `cargo xtask lint-cancel-tokens`
   - `cargo xtask lint-secrets`
   - `cargo xtask lint-publish-registration`
   - `bridges/cxf/gradlew -p bridges/cxf test spotlessCheck`
2. `lint-commits` is skipped here (it fetches from origin; CI owns the branch-diff check). Do not raise any ratchet file under `scripts/xtask/`.

**Tests:**
- `quality_gates`: every command in step 1 exits 0.

**Acceptance:**
- Every command in step 1 exits 0; `git diff --stat -- scripts/xtask` is empty.

- [x] 4.1

## Scenario coverage

| Spec | Scenario | Owning task(s) |
| --- | --- | --- |
| cxf-consumer-dispatch | Two profiles keep two streams | 1.3 (`secondProfileStreamDoesNotReplaceFirst`), 2.7 |
| cxf-consumer-dispatch | Open without a profile is rejected | 1.4 (`openConsumerStreamRejectsMissingEmptyMiscasedAndUnknownProfile`), 1.5 (`miscasedProfileIsRejectedOverRealGrpc`) |
| cxf-consumer-dispatch | Consumer does not retry a rejected profile | 2.2 (`invalid_argument_on_open_is_terminal`) |
| cxf-consumer-dispatch | Rejected profile does not restart the route | 2.3 (`rejected_profile_does_not_crash_or_restart_route`) |
| cxf-consumer-dispatch | Metadata reaches the handler over real gRPC | 1.5 (`streamRegistersUnderAnnouncedProfile`) |
| cxf-consumer-dispatch | Request goes to its own route | 1.3 (`requestDispatchesOnlyToItsProfileStream`), 2.7 (`cxf_consumer_two_profiles_dispatch_to_own_route`) |
| cxf-consumer-dispatch | No cross-profile fallback | 1.3 (`missingProfileStreamFailsWithoutFallback`), 1.6 (`noCrossProfileFallbackWithRealManager`), 2.7 (`cxf_consumer_profile_without_stream_returns_503`) |
| cxf-consumer-dispatch | Closing one profile does not fail another | 1.3 (`closingOneProfileFailsOnlyItsRequests`) |
| cxf-consumer-dispatch | Reconnect of one profile keeps new requests | 1.2 (rewritten `reconnectDuringPendingCleanupPreservesNewRequests`, rewritten `replacedStreamCloseFailsOnlyItsPendingRequests`) |
| cxf-consumer-dispatch | Duplicate profile | 2.4 (`consumer_profile_registry_rejects_duplicate`), 2.6 (`duplicate_profile_fails_at_start_and_first_keeps_receiving`) |
| cxf-consumer-dispatch | Stopped route can start again | 2.5 (`stopped_consumer_profile_can_start_again`, `failed_start_releases_profile_registration`, `aborted_task_and_dropped_consumer_release_profile`) |
| cxf-consumer-dispatch | Immediate restart after stop does not race the old task | 2.4 (`consumer_profile_guard_release_is_idempotent_and_token_scoped`), 2.5 (`immediate_restart_after_abort_and_stop_keeps_new_registration`) |
| cxf-fault-status | Request before the consumer stream returns 503 without a fault | 1.2 (`preStream503IsTransportAvailabilityNotFault`, stub per profile), 1.6 (`gateUsesAddressedProfileNotAnyProfile`) |
| cxf-fault-status | Availability is checked before WS-Security | 1.6 (extended `availabilityCheckedBeforeWssForSignatureProfile`) |
| cxf-fault-status | Late stream loss yields the same 503 | 1.2 (rewritten `lateStreamLossYieldsSame503`) |
