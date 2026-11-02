# ADR-0095: MSRV policy and the 1.96 floor

- Status: Accepted (decided 2026-11-02)
- Source: bd `rc-y4ev7` (P1), bd `rc-s026j` (deps migration wave 2)
- Ruling: e_opus master report 2026-11-02; e_gpt pre-flight GO-WITH-FIXES
  2026-11-02; owner ratification via mission order 490 resume directive
  ("proceed with families 1-6 in order", 2026-11-02)
- Cross-refs: ADR-0090 (pre-1.0 compatibility), ADR-0083 (artifact
  signing envelope)

## Context

The workspace declared `rust-version = "1.89"` with `resolver = "2"`.
That declaration was already false when written back: the lockfile
requires newer toolchains. wasmtime/cranelift/pulley/wiggle 49 need
1.96, `fastnum` needs 1.94, `boa_*` need 1.91, and `redb` — a direct
`camel-core` dependency — needs 1.90. `camel-core` therefore does not
build on 1.89. `resolver = "2"` is MSRV-unaware, and CI only runs
stable, so nothing enforced any floor.

The declared MSRV is a public contract for every published crate. This
ADR exists because the contract must match reality and be enforced.

## Decision

1. **Floor.** `rust-version = "1.96"` at the workspace root. 1.96 is
   the highest floor required by the current lockfile (the wasmtime 49
   family). The floor moves only with a new ADR or an explicit
   revision of this one, never silently, never as a side effect of a
   dependency bump. A dependency bump that raises the true floor above
   the declared floor must either park or land together with a floor
   revision.

2. **Ratification.** The 1.89 → 1.96 raise was ratified by the owner
   through the mission order 490 resume directive of 2026-11-02, which
   ordered execution of the MSRV family first. It is a public-contract
   change and was never applied silently.

3. **Coverage.** The workspace `rust-version` is the project floor.
   Every workspace member inherits it via `rust-version.workspace =
   true`; no member declares its own value. The `fuzz` crate (a
   standalone workspace) declares `rust-version = "1.96"` directly.
   Guest/example crates excluded from the root workspace are out of
   scope for this contract.

4. **Enforcement.** CI gains an `msrv-check` job pinned to the exact
   floor toolchain (`1.96.0`) that runs `cargo check --workspace
   --locked`. Local development runs stable (≥ floor); exact-floor
   verification is CI's responsibility because local machines carry a
   single system toolchain without rustup.

5. **Resolver.** `resolver = "2"` stays for now. Resolver 3
   (MSRV-aware resolution) would change resolution behavior for every
   future bump and is deferred until the dependency migration wave
   settles; revisit it as a dedicated decision once the wave lands,
   never in the same commit as a migration.

## Consequences

- Publishes crates stop advertising a floor they cannot meet.
- Dependency majors that need a higher floor (wasmtime 50 needs 1.97,
  mail-parser 1.0 needs 1.98) stay parked until a floor revision is
  ratified.
- The `msrv-check` job keeps the declared floor honest: a bump that
  silently exceeds 1.96 fails CI on the pinned toolchain.
