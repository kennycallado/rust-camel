## ADDED Requirements

### Requirement: Single opt-in allocator override

The `camel` binary SHALL offer exactly one optional global-allocator
override: the `jemalloc` cargo feature. Default builds SHALL use the
platform system allocator (glibc malloc on gnu images; the Dockerfile
MALLOC_ARENA_MAX guidance applies). Release builds that opt into
`jemalloc` SHALL keep the allocator memory gauges and heap-profiling
behavior owned by the component-metrics-emission capability. The binary
SHALL NOT ship a second allocator alternative: no `mimalloc` cargo
feature, dependency, or `#[global_allocator]` path MAY exist.

#### Scenario: default build uses the system allocator

- **GIVEN** the workspace at rest
- **WHEN** `cargo tree -p camel-cli -e no-dev` resolves the default
  feature set
- **THEN** no allocator crate (jemalloc, mimalloc, or their -sys crates)
  appears in the resolved graph

#### Scenario: jemalloc remains the sole opt-in override

- **GIVEN** the `jemalloc` feature still exists
- **WHEN** the binary is built with `--features jemalloc`
- **THEN** `tikv_jemallocator::Jemalloc` backs the process and the
  allocator gauges compile in

#### Scenario: the mimalloc stack is unreachable under any feature combination

- **GIVEN** the `mimalloc` feature and dependency are removed
- **WHEN** `cargo tree -p camel-cli --all-features` resolves
- **THEN** neither `mimalloc` nor `libmimalloc-sys` appears anywhere in
  the graph (jemalloc MAY appear there — it is the surviving override),
  and the workspace lockfile contains no camel-cli edge that can
  re-introduce them
