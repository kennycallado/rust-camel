## ADDED Requirements

### Requirement: Prebuilt assets are binstall-addressable

The `camel-cli` crate MUST carry `[package.metadata.binstall]` so that
`cargo binstall camel-cli` installs a prebuilt binary from the project's
GitHub releases instead of falling back to a long compile. Flavor selection
MUST use only `cfg(target)` overrides — binstall's template language has no
feature variable — and MUST follow the platform-ceiling rule. Flavor selection
uses only `cfg(target)` overrides — binstall's template language has no
feature variable — and follows the platform-ceiling rule: the default is
`full` everywhere full exists; targets where full cannot exist get `regular`
via a single override.

#### Scenario: non-musl target resolves to the full tarball

- **Given** the release pipeline has published flat tarballs named
  `camel-full-<target>.tar.gz` for the non-musl targets
  (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
  `x86_64-apple-darwin`, `aarch64-apple-darwin`,
  `x86_64-pc-windows-msvc`)
- **When** `cargo binstall --dry-run --target <non-musl-target> camel-cli`
  resolves its download URL
- **Then** the URL is
  `{repo}/releases/download/v{version}/camel-full-<target>.tar.gz`
  with the literal `.tar.gz` suffix (never `{archive-suffix}`, which
  resolves to `.tgz`), and the URL points at an asset that exists in the
  release

#### Scenario: musl target resolves to regular through the override

- **Given** full tarballs do not exist for musl targets (librdkafka does
  not build on musl) and `camel-<target>.tar.gz` regular tarballs exist for
  `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`
- **When** `cargo binstall --dry-run --target <musl-target> camel-cli`
  resolves its download URL
- **Then** the `cfg(all(target_os = "linux", target_env = "musl"))`
  override selects
  `{repo}/releases/download/v{version}/camel-<target>.tar.gz`, and no musl
  resolution ever names a `camel-full-` asset

#### Scenario: the installed binary is named camel

- **Given** tarballs are flat — the binary sits at the archive root as
  `camel` (or `camel.exe` on Windows) while the asset filename carries the
  flavor and target prefix
- **When** binstall extracts a fetched tarball during install
- **Then** `bin-dir = "{bin}{binary-ext}"` places the binary as `camel`
  (`camel.exe` on Windows) in the cargo bin directory — never under the
  asset stem name

#### Scenario: quickinstall is disabled and compile stays enabled

- **Given** third-party quickinstall serves default-features builds that
  would masquerade as official prebuilts
- **When** the metadata's `disabled-strategies` is read
- **Then** it contains exactly `["quick-install"]` — `crate-meta-data`
  (the project's own releases) and `compile` (which yields the
  `flavor-regular` default) remain enabled

#### Scenario: smoke check maps every supported target to a real asset

- **Given** the current release's published asset list
- **When** the repeatable smoke check resolves, for every supported
  target, the URL binstall would fetch
- **Then** every resolved URL matches a published asset name exactly, and
  the check fails if any target resolves to a missing asset or falls
  through to a URL miss
