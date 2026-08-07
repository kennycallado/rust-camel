# Design: audit-fix-http-clippy-gate

## Approach

Mirror the existing `#[allow(clippy::await_holding_lock)]` pattern already applied to 17 test functions in `camel-component-http`. The 3 affected test functions hold `REGISTRY_TEST_MUTEX` (serializing access to the global `ServerRegistry`) across an `.await` point. The mutex ensures tests do not interfere with each other via shared global state; holding it across `.await` is inherent to the test's async setup. The correct fix is the allow annotation, not restructuring the test logic.

## Affected crates

- **camel-component-http**: 2 files, 3 test functions receive `#[allow(clippy::await_holding_lock)]`.

## Architecture boundaries

This change touches **test code only** (functions prefixed `test_`). No runtime, DSL, component SPI, service, language, or function boundary is affected. The annotations are compile-time directives local to the annotated function.

## Alternatives considered

- **Restructure tests to avoid holding locks across await:** rejected — the mutex serializes access to the global `ServerRegistry`, preventing test interference via shared state. Restructuring would change the test's isolation guarantee, not how they compile.
- **Add crate-level `#![allow(clippy::await_holding_lock)]`:** rejected — the existing convention is per-function, which scopes the allow tightly. A crate-level allow would suppress the lint in production code where it IS a real bug.

## Decision

Follow the existing per-function convention. Add 3 `#[allow(clippy::await_holding_lock)]` attributes immediately above each affected `#[tokio::test]` / `#[test]` function, matching the indentation and placement of the 17 existing annotations.
