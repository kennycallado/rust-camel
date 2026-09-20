# Proposal: binstall-metadata

## Why

The release pipeline now ships binstall-grade assets (rc-5t5fo.2 merged): 11
flat `.tar.gz` tarballs + 11 `.sha256` sidecars per release. But
`cargo binstall camel-cli` still falls back to a ~15-minute compile because
the crate carries no `[package.metadata.binstall]` — historically the crate
name never matched the `camel-*` asset names, so binstall never found
prebuilts. There is zero installed base to protect: prebuilts effectively
begin at the first metadata-bearing release (metadata is read from the crate
version being installed; `camel-cli@0.39` and `@0.50.0` carry none — they
have no project-hosted prebuilt routing and binstall may serve a third-party
quickinstall build; a guaranteed source compile of an older version is
`cargo install camel-cli --version <version>`, which yields `flavor-regular`
— the platform ceiling — by default).

Flavor selection has a hard constraint: binstall's template language has no
feature/flavor variable; the only selection surface is `cfg(target)`
overrides. Full is absent on musl (kafka's librdkafka C dependency does not
build there), so a plain full default would 404 on musl and fall to compile.

## What Changes

- ONE `[package.metadata.binstall]` block in `crates/camel-cli/Cargo.toml`:
  - `pkg-url = "{repo}/releases/download/v{version}/camel-full-{target}.tar.gz"`
    — literal `.tar.gz` suffix: `{archive-suffix}` resolves to `.tgz` for the
    tgz format, which does NOT match our tarball names. Version lives in the
    release path, never in the filename.
  - `pkg-fmt = "tgz"`, `bin-dir = "{bin}{binary-ext}"` — tarballs are flat
    (binary at root), so the binary installs as `camel` on every platform.
  - `overrides.'cfg(all(target_os = "linux", target_env = "musl"))'` →
    `pkg-url` `camel-{target}.tar.gz` (regular = the platform ceiling where
    full cannot exist). One clause covers both musl architectures.
  - `disabled-strategies = ["quick-install"]` — third-party quickinstall
    default-features builds must never masquerade as official prebuilts.
    `crate-meta-data` (ours) and `compile` (honest fallback = regular) stay.
- One install line each in `README.md` and the docs install section:
  `cargo binstall camel-cli` installs the fullest prebuilt for the platform
  (regular on musl); other flavors via docker tags, direct download, or
  `cargo install --features`.
- A repeatable smoke check running `cargo binstall --dry-run` for each
  supported target, asserting the resolved URL matches a real release asset
  (gnu x2 → full, musl x2 → regular via override, darwin x2 → full,
  windows x86 → full). Placement (release-matrix leg vs standalone check)
  is a design decision.
- Explicitly excluded: touching default features (already `flavor-regular`,
  done in rc-5t5fo.5), wrapper crates `camel-slim`/`camel-full` (deferred —
  adding later is non-breaking), renaming assets or adding version to
  filenames, `.sha256` consumption wiring (sidecars exist for manual
  verification; binstall checksum use is opt-in and separate).

## Acceptance criteria

- `cargo binstall --dry-run camel-cli` resolves to an EXISTING asset for
  every supported target — no target falls through to compile by URL miss.
- The installed binary is named `camel` (not `camel-full-<target>`) on every
  platform.
- `quick-install` is disabled; `compile` remains enabled and yields regular.
- Release asset topology is untouched (still exactly 22 files).

## Risk budget

- Acceptable: metadata is inert until the next crates.io publish (0.51.0+);
  old versions carry no routing (binstall may serve a third-party
  quickinstall build; guaranteed source compile:
  `cargo install camel-cli --version <version>`) (documented).
- Out of bounds: any change to flavor closure, feature defaults, asset
  names, or the release pipeline's asset set.
