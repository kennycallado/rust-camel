# Design: binstall-metadata

## Approach

Add a single static metadata block to `crates/camel-cli/Cargo.toml` — no
code, no pipeline asset changes. Every value is fixed by decisions already
blessed in the e_opus consult (docs/audits/2026-09-19-binstall-metadata-
design-consult.md) and by the asset format that rc-5t5fo.2 landed:

```toml
[package.metadata.binstall]
pkg-url = "{repo}/releases/download/v{version}/camel-full-{target}.tar.gz"
pkg-fmt = "tgz"
bin-dir = "{bin}{binary-ext}"
disabled-strategies = ["quick-install"]

[package.metadata.binstall.overrides.'cfg(all(target_os = "linux", target_env = "musl"))']
pkg-url = "{repo}/releases/download/v{version}/camel-{target}.tar.gz"
```

Key decisions and their reasons:

- Literal `.tar.gz` in `pkg-url`, not `{archive-suffix}`: that variable
  resolves to `.tgz` for the tgz format, which mismatches our tarball names.
  Filenames carry no version — version lives in the release path only.
- Default = full. On every non-musl target binstall asks for
  (`*-linux-gnu` x2, `*-apple-darwin` x2, `x86_64-pc-windows-msvc`) a full
  tarball exists — including `camel-full-x86_64-apple-darwin` (verified in
  the rc.2 asset list; the darwin-x86 leg that does NOT upload is the
  regular compile-guard, irrelevant to the full default).
- Single musl override → regular: full cannot exist on musl (librdkafka);
  regular is the platform ceiling there. One `cfg` clause covers both musl
  architectures; gnu/mac/win keep the default.
- `bin-dir = "{bin}{binary-ext}"`: tarballs are flat (`camel`/`camel.exe`
  at root), so the installed binary is named `camel`, not the asset stem.
- `disabled-strategies = ["quick-install"]` only: quickinstall's
  default-features builds must not masquerade as official prebuilts;
  `crate-meta-data` (our releases) and `compile` (yields regular — the
  default feature set) stay enabled. Per-target blocks are unnecessary —
  `disabled-strategies` has no per-target variance.
- Metadata is inert until the next crates.io publish; older versions carry
  no routing (binstall may serve a third-party quickinstall build; a
  guaranteed source compile is `cargo install camel-cli --version <version>`)
  (documented in README/INSTALL lines).

Smoke check: a standalone repeatable script (repo `scripts/` or a docs
command block) that resolves, per target, the URL binstall WOULD fetch —
by templating the same strings against the release list — and asserts each
resolves to a real published asset of the current release. It runs
`cargo binstall --dry-run --target <t> camel-cli` where the toolchain is
available and falls back to pure URL string templating otherwise (CI
runners without cargo-binstall). Placement as a release-matrix job is
REJECTED: the release workflow must not gain a new failure surface that
depends on a third-party tool's availability; the check is a manual/CI-docs
invocation (may be wired into CI later as its own workflow).

## Affected crates

- `camel-cli`: adds `[package.metadata.binstall]` + override block to
  `Cargo.toml`. No source changes, no dependency changes, no feature
  changes (`default = ["flavor-regular"]` already correct).

## Architecture boundaries

Respects every boundary by construction: no runtime, DSL, component,
service, language, or function code is touched. The change lives in
packaging metadata (Cargo.toml `[package.metadata]` is ignored by cargo
itself) and in docs. The hexagonal boundary set (camel-api / camel-core /
components) is untouched; the release pipeline's asset contract
(22 files) is untouched.

## Phases

Single-phase: one metadata block, two doc lines, one smoke script — a
single coherent slice with no milestone structure (no `## Phase N`
headings in tasks.md).
