# Proposal: sealed-artifact-materialization

## Why

`docs/src/cli/compile.md` states a sealed artifact "performs no extraction", yet four
asset classes still materialize into a confined per-boot directory under
`std::env::temp_dir()` at boot: the TLS family (certificate / private key / client CA),
`xslt stylesheet`, `xsd schema`, and `sql file`. In `FROM scratch` containers there is
often no `/tmp` and the rootfs is read-only, so those artifacts fail at boot. Mission 350
(bd `rc-joask`) already fixed `protoFile` with compile-time embedding plus in-process
resolution; the four remaining classes were not covered. bd: `rc-l39dj`.

## What Changes

- **Inventory** every materialization call site for the four classes and record it in
  `design.md` (call site, what is written, who reads it, boot vs first use).
- **Per-class boot decision** (binding, recorded in design.md):
  - `xslt stylesheet`, `xsd schema`, `sql file` → **(b) in-memory materialization**,
    the mission-350 pattern: boot registers the verified asset bytes into a new
    process-global embedded-asset registry in `camel-component-api`
    (`camel_component_api::embedded_assets`, mirroring `camel-proto-compiler::embedded`)
    and rewrites the substitution site to a `camel-embedded:` reference; the owning
    component resolves the reference in-process. Zero temp writes.
  - TLS family → **(a) create-on-boot with fallback**: the per-boot directory creation
    falls back from the system temp directory to a writable directory next to the
    artifact; when no candidate is writable the boot fails with the existing typed
    `TempUnavailable` error **(c)** that names the class and the requirement. The full
    TLS in-memory conversion is out of scope (14+ read sites plus hot-reload semantics
    across camel-http/ws/grpc) and is filed as a bd follow-up.
- **`docs/src/cli/compile.md`** corrected to state the per-class truth.
- **`CHANGELOG.md`**: one fix entry.

Affected crates: `camel-cli`, `camel-component-api`, `camel-xslt`, `camel-validator`,
`camel-sql`. Docs: `docs/src/cli/compile.md`, `CHANGELOG.md`, owning `CONTEXT.md`.

## Acceptance criteria

- Hermetic regression tests named exactly: `tls_asset_boots_without_system_temp`,
  `xslt_asset_boots_without_system_temp`, `xsd_asset_boots_without_system_temp`,
  `sql_asset_boots_without_system_temp`, `error_names_the_asset_class_and_requirement`,
  `no_unrequested_temp_writes_at_boot` — path injection only, no env mutation.
- A TLS artifact on a host with a missing temp dir but a writable artifact directory
  boots; with no writable location it fails naming the class and requirement, never
  leaking an absolute host path.
- xslt/xsd/sql sealed artifacts perform zero filesystem writes for their asset class and
  resolve their assets in-process; non-sealed (plain-path) boots are unchanged.
- compile.md matches the implementation per class.

## Risk budget

Acceptable: new leaf module in `camel-component-api`, CLI boot-partition change,
confined-directory fallback chain. Out of bounds: sealed-artifact format changes,
`protoFile` changes, container-based verification (hermetic path-injection tests are the
gate; real scratch containers stay CI's job), TLS in-memory conversion.
