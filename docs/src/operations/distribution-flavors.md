# Distribution flavors

The `camel` CLI ships in three flavors: slim, regular, and full. Each flavor is a preset feature set selected by a `flavor-*` marker in `crates/camel-cli/Cargo.toml`.

## Flavor table

| Group | What ships | Targets | Artifact names |
|-------|-----------|---------|----------------|
| Base (unconditional) | core, direct, seda, log, file, timer, stream (in/out/err), http (server and client, REST DSL, health and metrics, ADR-0052), template (minijinja), master, cron, controlbus, mock, validator, camel test harness | every flavor, every target | none (no separate artifact) |
| Edge pack (slim additions) | mqtt, mqtt-tls, http-static, sql, lang-jsonpath, lang-rhai | x86_64-unknown-linux-musl, aarch64-unknown-linux-musl | `camel-slim-<target>.tar.gz` |
| Regular additions | otel, grpc, wasm, llm, mcp, security, redis, redis-tls, jms, cxf, xj, xslt, opensearch, ws, lang-xpath, lang-js (boa), lang-minijinja, lsp, kubernetes, integration-http, integration-sql | the 4 Linux targets (gnu ×2, musl ×2) | `camel-<target>.tar.gz` |
| Full additions | exec, kafka, surrealdb, containers | gnu ×2, macOS ×2, Windows — desktop platforms ship full only | `camel-full-<target>.tar.gz` |

Each asset has a matching `.tar.gz.sha256` sidecar that stores the tarball checksum.

The bodies are chained. Each flavor includes the marker of the flavor below it. `slim ⊆ regular ⊆ full` is structural, not tested-in. `camel lint` is part of the base set, so it works in every flavor.

Regular excludes only four items, each by a named principle:

- `kafka`: a C dependency (librdkafka) that cannot build for musl targets.
- `surrealdb`: BUSL-1.1, the only non-OSI license in the dependency graph.
- `exec`: arbitrary host-binary execution, rejected by ADR-0037.
- `containers`: an infrastructure-daemon client (camel-function and camel-component-container as one feature).

## Docker tag map

| Flavor | Tags | Base image |
|--------|------|-----------|
| regular | `{VERSION}`, `latest`, `regular` | scratch |
| slim | `{VERSION}-slim`, `latest-slim`, `slim` | scratch (from 0.51.0) |
| full | `{VERSION}-full`, `latest-full`, `full` | distroless cc |

The base image is a function of the toolchain. Musl flavors ship on scratch. The gnu flavor ships on distroless.

### Discontinued tags

The `-alpine` and `-gnu` tags are discontinued. They are never pushed again. Existing tags freeze at their last content. Pulls keep working. Nothing 404s. Use the `-slim` and `-full` families instead.

### DIY alpine wrapper

The slim flavor has no shell and no wget. Build a wrapper image when you need them:

```dockerfile
FROM alpine:3.21
COPY --from=ghcr.io/kennycallado/rust-camel:{VERSION}-slim /usr/local/bin/camel /usr/local/bin/camel
```

## Install guidance

### Release download

GitHub release assets carry the flavor prefix in the name:

- slim: `camel-slim-<target>.tar.gz` (musl x2)
- regular: `camel-<target>.tar.gz` (the 4 Linux targets)
- full: `camel-full-<target>.tar.gz` (gnu x2, macOS x2, Windows)

### Docker

Pull the semantic tag for the flavor you want:

```text
docker pull ghcr.io/kennycallado/rust-camel:regular
docker pull ghcr.io/kennycallado/rust-camel:slim
docker pull ghcr.io/kennycallado/rust-camel:full
```

### cargo install

`cargo install camel-cli` builds the regular flavor from source. The build is pure Rust. It needs no C toolchain. The four principled exclusions stay out of the default build.

### cargo binstall

`cargo binstall camel-cli` installs the fullest prebuilt binary for your platform from GitHub releases. On musl it installs the regular flavor. Other flavors come via docker tags, direct release download, or `cargo install --features`. Slim additionally needs `--no-default-features`. Prebuilts via binstall start at the first metadata-bearing release (0.51.0). Older versions carry no binstall metadata, so they have no project-hosted prebuilt routing and binstall may serve a third-party quickinstall build instead; for a guaranteed source compile of an older version use `cargo install camel-cli --version <version>` (yields the regular flavor).

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

The `camel-<target>` release asset re-aliases from the historical full-ish closure to the regular flavor at 0.50. This is a one-time semantic break. The `latest` Docker tag kept the full-no-kafka content at 0.50.0. It tracks regular from 0.51.0 (see the migration note). The 0.50 release notes reference this section as the canonical callout.

Before 0.50, `camel-<target>` carried the full-ish closure. After 0.50, it carries the regular closure. Users who need the full closure download `camel-full-<target>` or pull the `:full` image.

v0.50.0 shipped release assets with extensionless names, such as `camel-x86_64-unknown-linux-gnu`. From the next release, every asset is a `.tar.gz` tarball with a matching `.tar.gz.sha256` sidecar. Update a download script with one line:

```text
curl -O https://github.com/kennycallado/rust-camel/releases/download/<tag>/camel-x86_64-unknown-linux-gnu.tar.gz https://github.com/kennycallado/rust-camel/releases/download/<tag>/camel-x86_64-unknown-linux-gnu.tar.gz.sha256 && sha256sum -c camel-x86_64-unknown-linux-gnu.tar.gz.sha256 && tar -xzf camel-x86_64-unknown-linux-gnu.tar.gz
```

Desktop targets (macOS, Windows) ship full only from 0.50: the clean
`camel-<target>` regular name does not exist there. Scripts that download
`camel-x86_64-apple-darwin` or `camel-x86_64-pc-windows-msvc.exe` must
switch to the `camel-full-` prefixed names. `cargo install camel-cli`
still compiles the regular closure from source on every platform (a
compile-guard leg proves macOS regular builds).

Windows 10 before version 1803 and Windows Server 2016 do not ship `tar`. Users on these systems must supply their own tar, such as 7-Zip or bsdtar.

## Iteration policy

Flavors are presets, not walls. Additions flow down freely in minor releases. Moving a feature from full to regular to slim is non-breaking. Removals happen only at major releases.

## How to move a feature between flavors

1. Edit the one flavor list in `crates/camel-cli/Cargo.toml` where the feature should start appearing. Bodies are chained, so one edit moves the feature for every flavor above it.
2. Update the contract prefix sets in `crates/camel-cli/tests/feature_profiles.rs` if a principle boundary is crossed. The sets are `SLIM_FORBIDDEN_PREFIXES`, `REGULAR_FORBIDDEN_PREFIXES`, `REGULAR_REQUIRED_PREFIXES`, and `FULL_REQUIRED_PREFIXES`.
3. Regenerate the golden fixture with the command documented in the `feature_profiles.rs` header.
4. Update the flavor table in this doc.

CI matrix legs never change. Legs are target by flavor. `full_covers_universe` goes red if a new feature is placed in no flavor.

## Appendix: migration note for 0.51.0

Ready to paste into the 0.51.0 release notes:

> The `latest` Docker tag now tracks the regular flavor. The `-slim` and `-full` families are the flavor-named tag families. The `-alpine` and `-gnu` tags are discontinued and frozen at their last content. Release-candidate rehearsals never move floating tags.

## Appendix: rehearsal runbook

Release-time verification for the floating-tag quarantine. Executed at the 0.51.0-rc release time. Follow-up ticket rc-5t5fo.9 owns the post-0.51.0 verification pass.

0. Capture the digests of the six floating tags on both registries before pushing the rc tag. Example: `docker manifest inspect ghcr.io/kennycallado/rust-camel:latest`.
1. Push the `v0.51.0-rc.N` tag.
2. After the run, query both registries' tag timestamps. Use the Docker Hub API and GHCR. Assert `latest`, `latest-slim`, `latest-full`, `regular`, `slim`, `full` are unchanged. Compare against the pre-rehearsal digests.
3. Assert the rc-suffixed immutable tags exist (`v0.51.0-rc.N`, `v0.51.0-rc.N-slim`, `v0.51.0-rc.N-full`).

**Reference**: [CLI crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-cli/CONTEXT.md)