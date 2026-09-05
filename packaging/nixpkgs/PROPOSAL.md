# nixpkgs packaging proposal

## Recommendation

Submit the package as `camel` in `pkgs/by-name/ca/camel/package.nix`.
The installed command is `camel`, so the attribute follows the primary binary.
Check for an attribute collision again when the patch is submitted. Use
`rust-camel` only as a search keyword. The draft targets
the latest upstream tag, `v0.28.0` (commit
`8f8eca3f9c63da98e332f89a31b042a5d02f94db`, dated 2026-08-12).

The draft uses `rustPlatform.buildRustPackage` and builds only `camel-cli` from
the release workspace. It does not enable Kafka. This matches the CLI defaults:
`otel`, `grpc`, `wasm`, `http-static`, `llm`, `surrealdb`, `exec`, and `mqtt`.

## Lock file and source

The committed `Cargo.lock` is Cargo lock format 4. All locked third-party
packages use the crates.io registry and have checksums. It has no Git sources.
This is suitable for the `cargoLock.lockFile` mode of `buildRustPackage`
without a `cargoLock.outputHashes` table or a separately maintained
`cargoHash`.

The GitHub source archive has the verified unpacked hash
`sha256-f9lBYu3RI5lKp1t3BZlacWiw9xQl6mFudJ1Cuu4v1DY=`.

## Native dependencies

For the default CLI feature set, the direct Nix inputs are `cmake` and
`pkg-config` at build time and `libxml2` at build and run time. XML support
uses `libxml2`. The default TLS stack selects `aws-lc-sys`, which builds its
bundled C source with CMake. The remaining default dependencies build with the
Rust and C toolchains supplied by `buildRustPackage` and `stdenv`.

Kafka is correctly optional and must remain outside the default package. If a
future `camelWithKafka` variant is useful, enable `kafka,dynamic-linking` and
add nixpkgs `rdkafka` plus `pkg-config`. Do not make Kafka a default: it adds a
large native dependency to users who do not use Kafka. Prefer system
`librdkafka` over `kafka-static` in nixpkgs for shared security updates and to
avoid a second bundled build.

## Sandbox tests

Run the `camel-cli` library tests during `checkPhase`; they do not require
Kafka, Redis, Kubernetes, or another external service. Run `camel --help` in
`installCheckPhase` to verify the installed binary. Do not run all workspace
tests: component integration suites can require service processes, containers,
network ports, fixtures, or guest toolchains.

## Platforms

The first required builder is `x86_64-linux`. The dependency choices are also
available on `aarch64-linux`, `x86_64-darwin`, and `aarch64-darwin`, so the
proposed platform set is `lib.platforms.unix`. Darwin may require
`Security` and `SystemConfiguration` frameworks if a build shows that a
dependency uses native TLS. Do not add those frameworks preemptively.

## Complete metadata

Use the upstream project page as `homepage`, the tagged GitHub release as
`changelog`, `lib.licenses.asl20` as `license`, `camel` as `mainProgram`, and
`lib.platforms.unix` as `platforms`. The nixpkgs submitter must add themselves
to `maintainers`; an empty list is suitable only for this upstream draft.

## Upstream improvements

The release is already reproducible enough for nixpkgs because it commits a
checksum-complete lock file and keeps Kafka optional. Two small improvements
would reduce downstream investigation:

- Document the native libraries for each CLI feature and operating system.
- Add a release CI job that builds `camel-cli` with its default features in a
  clean Nix sandbox on Linux and Darwin.
