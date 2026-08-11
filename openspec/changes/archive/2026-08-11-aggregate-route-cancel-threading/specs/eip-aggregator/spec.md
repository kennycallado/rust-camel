# Spec Delta: eip-aggregator

## ADDED Requirements

### Requirement: Aggregator sweep lifecycle binds to StepLifecycle

The DSL Aggregate step-compiler SHALL register the aggregator's
`StepLifecycle` handle on the compiled step (`lifecycle: Some(...)`), so the
runtime's `start_route`/`stop_route` drain drives the sweep lifecycle. The
aggregator SHALL own its sweep cancellation token internally through a swappable
cell, independent of any externally-threaded route token.

#### Scenario: Shutdown cancels the TTL sweep

- **Given** a route with an `<aggregate>` step (with `bucket_ttl` set) compiled
  via the DSL step-compiler, and the sweep task running
- **When** `StepLifecycle::shutdown(RouteStop)` is called
- **Then** the background TTL-sweep task terminates within a bounded window

#### Scenario: Start respawns the TTL sweep after restart

- **Given** a route that was stopped (`shutdown` called, sweep terminated)
- **When** `StepLifecycle::start()` is called followed by `poll_ready`
- **Then** the sweep task respawns bound to a fresh cancellation token

#### Scenario: HotSwap shuts down the sweep

- **Given** a route with a running sweep undergoing hot-swap reload
- **When** `StepLifecycle::shutdown(HotSwap)` is called
- **Then** the sweep task terminates (same code path as `RouteStop`)

#### Scenario: DSL step registers lifecycle handle

- **Given** an `<aggregate>` step compiled via the DSL step-compiler
- **When** the compiled step is inspected
- **Then** its `lifecycle` field is `Some(Arc<dyn StepLifecycle>)` (not `None`)
