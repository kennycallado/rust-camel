## ADDED Requirements

### Requirement: Runtime-agnostic endpoint creation

The XJ Component SHALL create endpoints successfully on any Tokio runtime
flavor (multi-thread, current-thread) and outside any runtime context.

#### Scenario: current-thread runtime does not panic

- **GIVEN** the XJ Component is registered in a CamelContext running on a
  current-thread Tokio runtime
- **WHEN** `create_endpoint` is called with a valid `xj:` URI
- **THEN** the call SHALL NOT panic and SHALL either return a valid Endpoint
  or return a `CamelError` describing a bridge-startup failure (not a runtime
  incompatibility)

#### Scenario: no ambient runtime does not produce a dead channel

- **GIVEN** the XJ Component is constructed and `create_endpoint` is called
  from a thread with no ambient Tokio runtime
- **WHEN** `ensure_bridge_started` succeeds and a tonic Channel is stored in
  `BridgeState::Ready`
- **THEN** the Channel's internal dispatch task SHALL remain alive on a stable
  runtime, and a subsequent gRPC call through that Channel SHALL NOT receive
  `DispatchGone` or `Unavailable` due to dispatch-task termination

#### Scenario: multi-thread runtime behaviour unchanged

- **GIVEN** the XJ Component is registered in a CamelContext running on a
  multi-thread Tokio runtime
- **WHEN** `create_endpoint` is called with a valid `xj:` URI
- **THEN** the call SHALL use `block_in_place` on the ambient runtime, and the
  Channel's dispatch task SHALL run on that same ambient runtime, identical to
  pre-change behaviour

**NOTE:** On the multi-thread path, the ambient runtime MUST outlive the XJ
Component, because the Channel's dispatch task is hosted on it. The runtime
registry holds the Component via `Arc<dyn Component>` for the context lifetime,
and the production runtime (`Runtime::new()`) is never dropped before context
teardown. This invariant is implicit in the pre-change design and is stated
here for completeness.

### Requirement: Offload runtime lifetime

The XJ Component SHALL host an owned multi-thread Tokio runtime (the offload
runtime) whose lifetime spans the Component's lifetime, ensuring that any
tonic Channel dispatch task spawned during endpoint creation on the offload
runtime remains alive until the Component is dropped.

#### Scenario: offload runtime outlives endpoint creation

- **GIVEN** an XJ Component with an offload runtime
- **WHEN** `create_endpoint` runs a future on the offload runtime that produces
  a tonic Channel
- **THEN** the offload runtime SHALL remain alive after `create_endpoint`
  returns, and the Channel's dispatch task SHALL continue to be polled

#### Scenario: offload runtime cleaned up on component drop

- **GIVEN** an XJ Component whose offload runtime hosts Channel dispatch tasks
- **WHEN** the XJ Component is dropped
- **THEN** the offload runtime SHALL be dropped, cancelling all tasks hosted on
  it, after the bridge process has been stopped via `shutdown()`

### Requirement: No block_in_place on current-thread runtime

The XJ Component SHALL NOT call `tokio::task::block_in_place` when the ambient
runtime is `RuntimeFlavor::CurrentThread`.

#### Scenario: current-thread runtime uses offload path

- **GIVEN** the ambient runtime is `RuntimeFlavor::CurrentThread`
- **WHEN** `block_on_result` is invoked
- **THEN** the future SHALL be driven by the offload runtime, and
  `block_in_place` SHALL NOT be called

#### Scenario: offload path runs on scoped thread without ambient runtime

- **GIVEN** the offload path is taken (ambient runtime is current-thread or
  absent)
- **WHEN** `block_on_result` drives the future via `std::thread::scope`
- **THEN** the future SHALL execute on a scoped OS thread that has no ambient
  Tokio runtime context, ensuring `Runtime::block_on` on the offload runtime
  does not panic
