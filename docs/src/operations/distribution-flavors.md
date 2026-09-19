# Distribution flavors

The `camel` CLI ships in three flavors: slim, regular, and full. Each flavor is a preset feature set selected by a `flavor-*` marker in `crates/camel-cli/Cargo.toml`.

## Flavor table

| Group | What ships | Targets | Artifact names |
|-------|-----------|---------|----------------|
| Base (unconditional) | core, direct, seda, log, file, timer, stream (in/out/err), http (server and client, REST DSL, health and metrics, ADR-0052), template (minijinja), master, cron, controlbus, mock, validator, camel test harness | every flavor, every target | none (no separate artifact) |
| Edge pack (slim additions) | mqtt, mqtt-tls, http-static, sql, lang-jsonpath, lang-rhai | x86_64-unknown-linux-musl, aarch64-unknown-linux-musl | `camel-slim-<target>` |
| Regular additions | otel, grpc, wasm, llm, mcp, security, redis, redis-tls, jms, cxf, xj, xslt, opensearch, ws, lang-xpath, lang-js (boa), lang-minijinja, lsp, kubernetes, integration-http, integration-sql | all 7 targets | `camel-<target>` |
| Full additions | exec, kafka, surrealdb, containers | x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu, x86_64-apple-darwin, aarch64-apple-darwin, x86_64-pc-windows-msvc | `camel-full-<target>` |

The bodies are chained. Each flavor includes the marker of the flavor below it. `slim ⊆ regular ⊆ full` is structural, not tested-in. `camel lint` is part of the base set, so it works in every flavor.

Regular excludes only four items, each by a named principle:

- `kafka`: a C dependency (librdkafka) that cannot build for musl targets.
- `surrealdb`: BUSL-1.1, the only non-OSI license in the dependency graph.
- `exec`: arbitrary host-binary execution, rejected by ADR-0037.
- `containers`: an infrastructure-daemon client (camel-function and camel-component-container as one feature).

## Docker tag map

| Variant | Base image | Flavor | Legacy tag | Semantic tag |
|---------|-----------|--------|-----------|--------------|
| production | scratch, musl | regular | `latest`, no suffix | `:regular` |
| alpine | musl | slim | `-alpine` | `:slim` |
| gnu | distroless | full | `-gnu` | `:full` |

The `latest` tag re-aliases to regular in the same release as the artifact re-alias. One announcement covers both. musl-based images never link rdkafka.

## Install guidance

### Release download

GitHub release assets carry the flavor prefix in the name:

- slim: `camel-slim-<target>` (musl x2)
- regular: `camel-<target>` (all 7 targets)
- full: `camel-full-<target>` (gnu x2, macOS x2, Windows)

### Docker

Pull the semantic tag for the flavor you want:

```text
docker pull ghcr.io/kennycallado/rust-camel:regular
docker pull ghcr.io/kennycallado/rust-camel:slim
docker pull ghcr.io/kennycallado/rust-camel:full
```

### cargo install

`cargo install camel-cli` builds the regular flavor from source. The build is pure Rust. It needs no C toolchain. The four principled exclusions stay out of the default build.

### Everything

`--features flavor-full` enables every feature:

```text
cargo install camel-cli --features flavor-full
```

### Composition beyond presets

Flavors are presets, not walls. Compose beyond them with any feature name:

```text
cargo install camel-cli --features flavor-regular,cxf
```

## Breaking change at 0.50

The `camel-<target>` release asset re-aliases from the historical full-ish closure to the regular flavor at 0.50. This is a one-time semantic break. The `latest` Docker tag re-aliases to regular in the same release. The 0.50 release notes reference this section as the canonical callout.

Before 0.50, `camel-<target>` carried the full-ish closure. After 0.50, it carries the regular closure. Users who need the full closure download `camel-full-<target>` or pull the `:full` image.

## Iteration policy

Flavors are presets, not walls. Additions flow down freely in minor releases. Moving a feature from full to regular to slim is non-breaking. Removals happen only at major releases.

## How to move a feature between flavors

1. Edit the one flavor list in `crates/camel-cli/Cargo.toml` where the feature should start appearing. Bodies are chained, so one edit moves the feature for every flavor above it.
2. Update the contract prefix sets in `crates/camel-cli/tests/feature_profiles.rs` if a principle boundary is crossed. The sets are `SLIM_FORBIDDEN_PREFIXES`, `REGULAR_FORBIDDEN_PREFIXES`, `REGULAR_REQUIRED_PREFIXES`, and `FULL_REQUIRED_PREFIXES`.
3. Regenerate the golden fixture with the command documented in the `feature_profiles.rs` header.
4. Update the flavor table in this doc.

CI matrix legs never change. Legs are target by flavor. `full_covers_universe` goes red if a new feature is placed in no flavor.

**Reference**: [CLI crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-cli/CONTEXT.md)