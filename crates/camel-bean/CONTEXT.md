# Bean

camel-bean connects named business-logic objects to route `bean:` steps. It owns registration,
method-name dispatch, and the `BeanProcessor` trait. `camel-bean-macros` generates implementations
from `#[bean_impl]` blocks and `#[handler]` methods.

camel-bean requires a crate-local context under the `CONTEXT-MAP.md` coverage policy. Its registry
is user-visible and stateful.

## Language

Crate-specific vocabulary. Cross-cutting Exchange and route terms remain in `CONTEXT-MAP.md`.

**BeanRegistry**:
The named registry of `BeanProcessor` trait objects. Registration rejects blank names and duplicate
names. A successful registration does not replace an existing object.
_Avoid_: service locator, dependency-injection container

**Bean binding (EIP)**:
The route behavior that selects a registered object and method, then maps Exchange data to method
parameters. This is the Apache Camel EIP concept. It is not the macro syntax that exposes a method.
_Avoid_: handler, registry lookup

**handler (macro marker)**:
A method marked with `#[handler]` inside a `#[bean_impl]` block. The macro exposes that method
through `BeanProcessor`. The marker identifies an invocable method. It does not name the Bean
binding EIP.
_Avoid_: Bean binding, route handler

## Registry and synchronization

`BeanRegistry` stores `HashMap<String, Arc<dyn BeanProcessor>>` behind a
`std::sync::Mutex`. `register(&self, ...)` can mutate the registry because the lock provides
interior mutability. Callers can share registered processors through cloned `Arc` values.

The synchronous mutex is deliberate. Critical sections only validate, insert, look up, clone, or
count entries. Lock acquisition recovers poisoned state with
`unwrap_or_else(|error| error.into_inner())`, so one panic does not make the registry unusable.

No registry lock remains held across `.await`. `invoke()` clones the processor `Arc` through
`get()`, releases the lock, validates the method name, and only then awaits `BeanProcessor::call`.
A Tokio mutex would add async lock overhead without protecting an asynchronous critical section.

## Dispatch and overload resolution

Dispatch currently uses the registered object name and method name. `BeanProcessor::methods()`
supplies the method-name allowlist. The generated implementation matches only the method string.

`BeanProcessor::method_params()` exposes optional parameter-type hints, but runtime overload
resolution is not implemented. The source records this limitation in `processor.rs` at the
`method_params()` default. Callers must not assume Java-style overload selection by argument type.

Bean binding and handler marking are separate concerns. Bean binding is the EIP that maps Exchange
data to a call. `#[handler]` is only the macro marker that includes a Rust method in generated
dispatch. Current generated binding supports body, headers, and mutable Exchange parameters.

## `#[non_exhaustive]` posture

camel-bean is outside ADR-0049's binding scope because it is not one of the three contract crates.
The crate applies the same API-evolution test by choice to public error contracts.

| Type | Posture | Rationale |
|---|---|---|
| `BeanError` | Needs `#[non_exhaustive]` before the 1.0 freeze | External callers receive this public enum from `BeanRegistry::register` and can match its variants. A future error variant must remain additive. The current code lacks the attribute; `rc-sfy1` tracks the pre-freeze correction. |
| `BeanRegistry` | Stays exhaustive | Callers construct it with `new()` or `default()` and cannot access its private storage field. |
| Future public error or contract enums | `#[non_exhaustive]` by default | Use a documented exception only when a closed variant set is the contract. |

This table records the intended pre-freeze posture. It does not claim that `rc-sfy1` is complete.

## Apache Camel parity anchor

The registry role follows `org.apache.camel.spi.Registry`: route configuration resolves a named
object before method invocation. The route behavior follows Apache Camel's Bean EIP and its bean
binding model. rust-camel uses Rust trait objects and generated typed extraction instead of Java
reflection.

Apache Camel is an inspiration corpus, not a conformance authority under ADR-0046. Name-based
dispatch is present. Runtime overload resolution by parameter type remains a documented parity
gap, not an implemented capability.

## Related decisions

- ADR-0046 defines the Apache Camel consultation and divergence policy.
- ADR-0049 supplies the contract-enum decision framework. Its binding scope does not include this
  crate.
- ADR-0012 applies no `error!` annotations here because this crate has no `error!` sites.
- `rc-sfy1` tracks the missing `#[non_exhaustive]` attribute on `BeanError`.
