## ADDED Requirements

### Requirement: Context drop terminates controller tasks

The system SHALL terminate the route-controller actor task and the
supervision task when a `CamelContext` is dropped. Dropping the context
is a NON-graceful termination: it does not replace route and service
teardown, which callers SHALL drive with `stop()` before dropping.
`stop()` SHALL continue to keep the actor alive for a subsequent
`start()`; only `abort()` and dropping the context SHALL be destructive.

#### Scenario: scenario-tier batch does not accumulate tasks

- **GIVEN** a process that boots, runs, stops, and drops one full
  `CamelContext` per scenario document, for N documents
- **WHEN** the batch completes
- **THEN** the process thread count and open file-descriptor count are
  the same (±1) as after the first document

#### Scenario: registered components drop with the context

- **GIVEN** a booted `CamelContext` and a probe component registered in
  the context's component registry, owned exclusively by that registry
  (its `Drop` sets an `Arc<AtomicBool>` flag)
- **WHEN** the context is stopped and dropped
- **THEN** the probe flag is set within a 5-second bounded wait

#### Scenario: stop-start restart is unaffected

- **GIVEN** a booted `CamelContext`
- **WHEN** `stop()` then `start()` complete
- **THEN** routes restart through the still-alive controller actor and
  subsequent stop-then-drop still terminates the actor

### Requirement: Component-context registry reference is non-owning

`RegistryComponentContext` SHALL hold a non-owning (weak) reference to
the component registry. Resolution through the context SHALL return
`None` when no strong reference to the registry remains. No component
bundle construction SHALL be able to keep the component registry alive
after its owning context is dropped. Programs that build a standalone
registry for the context SHALL keep a strong anchor alive for as long as
they need resolution to work.

#### Scenario: wasm bundle does not pin the component registry

- **GIVEN** a booted `CamelContext` whose component registry includes
  the wasm-bundle-registered component and a probe component owned
  exclusively by that registry
- **WHEN** the context is stopped and dropped
- **THEN** the probe component's `Drop` runs (the registry→component→
  context→registry cycle does not outlive the context)

#### Scenario: standalone registry resolves while anchored

- **GIVEN** a standalone `Registry` with a registered component, a
  `RegistryComponentContext` built over it, and a strong anchor `Arc`
  to the registry still held
- **WHEN** `resolve_component` is called for the registered scheme
- **THEN** it returns the component

#### Scenario: resolution after registry drop returns None

- **GIVEN** a `Registry` with a registered component and a
  `RegistryComponentContext` built over it, and the last strong
  registry reference dropped
- **WHEN** `resolve_component` is called for the registered scheme
- **THEN** it returns `None`
