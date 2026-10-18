# Design: cxf-consumer-per-profile-dispatch

## Approach

Single phase. Key the consumer observer by profile name.

**Java bridge (`bridges/cxf`)**
- `CxfServerManager`: replace `StreamObserver<ConsumerRequest> consumerRequestObserver`
  with `Map<String, StreamObserver<ConsumerRequest>>` guarded by the existing
  lock. `setConsumerRequestObserver(profile, observer)` replaces only that key.
  `onConsumerStreamClosed(profile, observer)` clears the key only if it still
  holds the same observer (`remove(key, observer)`) and fails only that
  stream's pending requests. `hasConsumerObserver(profile)` is per profile.
  `handleSoapRequest` looks up the observer with `request.getSecurityProfile()`;
  a missing key throws `StreamUnavailableException("no consumer stream for
  profile <p>")`. `cleanup()` clears the whole map. The capacity counter, the
  duplicate `request_id` check and `PendingRequest.owner` identity stay as they
  are.
- `CxfBridgeService.openConsumerStream`: read the profile from gRPC metadata key
  `x-cxf-profile`. A `ServerInterceptor` copies it into a gRPC `Context` key; it
  is a CDI bean registered with `@RegisterInterceptor` on `CxfBridgeService` (the
  service is `@GrpcService @Blocking`). The handler reads the Context key as its first
  action. The match is exact and case-sensitive against the configured profile
  names. A missing, empty or unknown profile closes the call with
  `INVALID_ARGUMENT` and leaves every registered stream untouched.
- `SoapEndpointPublisher`: the availability gate calls
  `CxfServerManager.hasConsumerObserver(profile)` with the profile resolved from
  the path.

**Rust (`camel-cxf`)**
- `CxfConsumer` sends `profile_name` as `x-cxf-profile` metadata when it opens
  the stream. The `#[allow(dead_code)]` on `profile_name` is removed.
- `CxfConsumer` treats `INVALID_ARGUMENT` on open as terminal. It checks
  `tonic::Code::InvalidArgument` on the `Status` returned by the open call, logs
  one error naming the profile (with a `// log-policy:` tag), and the consumer
  task returns `Ok(())`. It must not return `Err`: the runtime would log a
  second error and supervision would restart the route, which is a reconnect
  loop at route level. The transport-error retry loop in `consumer.rs` stays for
  transport failures only. The doc comment on `profile_name` (`consumer.rs:21-24`)
  is rewritten, not only the `dead_code` attribute removed.
- The pool keeps a set of active consumer profiles. `CxfConsumer::start()`
  registers its profile (not `create_consumer`); a second registration fails with
  `CamelError::Config` naming the profile. The guard has one owner:
  the `CxfConsumer` struct, not the spawned task. `stop()` releases it
  synchronously (the runtime aborts the task handle without awaiting it, then
  calls `stop()`, so the release cannot depend on the task future dropping). A
  failed `start()` releases it before returning. The release is idempotent and
  keyed by a registration token, so a late exit of an old task cannot remove the
  registration of a newer consumer on the same profile.

**Wire:** no `.proto` change. Metadata keeps both proto copies identical.

## Affected crates

- `camel-cxf`: consumer sends the profile; duplicate-profile rejection.
- `bridges/cxf` (Java): per-profile observer map, interceptor, availability gate.
- Docs: `docs/src/components/ws-soap.md` (lines 260, 385) and
  `crates/components/camel-cxf/CONTEXT.md` (line 112).

## Architecture boundaries

Components layer only (`camel-cxf` and its sidecar). No change to Runtime, DSL,
Services or Languages. The bridge stays a sidecar behind the existing gRPC
channel (ADR-0036 mTLS unchanged).

## Alternatives considered

- **Reject any second consumer per bridge.** Simple, but contradicts the existing
  per-profile catalogs, WSS processors and `/cxf/<profile>` paths. Rejected.
- **Typed hello frame (`register_profile` field).** Equivalent, but changes both
  proto copies and needs a first-frame state machine. Metadata is available
  before the first frame and needs no schema change. Rejected for now.
- **Default slot when the profile is absent.** Silently restores last-route-wins.
  Rejected.
