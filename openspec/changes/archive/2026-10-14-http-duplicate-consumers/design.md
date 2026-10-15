# Design: http-duplicate-consumers

## Approach

Both registration methods in HttpRouteRegistry return Result<(), CamelError>. Under the existing write lock, inspect the occupied key before mutation: legacy entries use the path; method-aware entries use method plus parsed PathSegment vector. Return CamelError::Config for duplicates; preserve the existing sender. The successful check and insert stay under one lock, including concurrent startup.

HttpConsumer::start propagates registration errors with ? before mark_ready and before the receive loop. The rejected consumer therefore never runs registry cleanup. Existing cancellation cleanup removes only the successful registration. After cancellation completes, another consumer can claim the freed key. No owner token or overwrite exception is needed: active registrations cannot be replaced.

Keep separate registries and ADR-0009 precedence, including legacy exact-path priority over method-aware templates. Distinct methods remain independent. Differently named parameter templates retain existing ambiguity handling; this issue concerns duplicate keys, not a matcher redesign. Public registration helpers become fallible; migrate every caller.

## Affected crates

- camel-http: registry.rs, HttpConsumer::start in lib.rs, and lib_tests.rs.
- camel-test: migrate the HTTP/static co-hosting fixture to assert successful registration.
- HTTP Component documentation: state duplicate rejection in its crate-local CONTEXT.md.

## Architecture boundaries

The Component owns shared-listener dispatch state. Runtime's Explicit startup already propagates pre-readiness errors, so no Runtime or DSL changes are required. Preserve ADR-0007 supervision and ADR-0009 co-hosting. Tests use staged bound listeners (ADR-0070), readiness signals and bounded joins, with no added sleeps.

## Alternatives considered

- DSL-only validation misses programmatic routes and concurrent registration.
- Last-write-wins plus owner-aware removal still silently steals traffic.
- Route-id replacement exceptions hide two concurrently running instances and require new ownership state.
- A combined registry would change co-hosting precedence and exceeds this bug's scope.
