# Design: release-asset-hygiene

## Approach

Packaging is a build-leg concern; verification is a consumer concern.

1. **Pack (build legs).** After the existing flavor probes (which run on
   the bare `target/<triple>/release/camel` binary), each uploading leg
   tars the binary at archive root (`camel` / `camel.exe`) into
   `dist/<artifact-name>.tar.gz` and emits
   `dist/<artifact-name>.tar.gz.sha256` (`sha256sum -c` format:
   `<hex>  <basename>`, two spaces). Both files upload as the workflow
   artifact. The compile-guard leg (darwin-x86 regular) uploads nothing —
   unchanged.
2. **Assert (release job).** Classification moves from raw names to
   tarball names: exactly 11 `.tar.gz` (2 slim + 5 full + 4 enumerated
   regular), exactly 11 `.sha256`, every sidecar's basename matches a
   present tarball, zero unexpected files. Sidecar CONTENT is verified in
   the docker job (see 3) and spot-checked here for format (hex64 +
   two-space + matching basename).
3. **Verify + extract (docker job).** Each download step fetches the
   tarball artifact, runs `sha256sum -c` against its sidecar, then
   `tar -xzf` into the flavor's binary dir. A tampered or corrupted
   tarball fails the build before any image layer exists.
4. **Docs.** `distribution-flavors.md` asset table gains the `.tar.gz`
   and `.sha256` columns; the breaking-change section gains the raw-name
   retirement note.

Flatten archives (no wrapper directory): docker's `tar -xzf` produces
`./camel` directly, and the future binstall `bin-dir` template stays
`{ bin }{ binary-ext }`.

## Decision 1 — Uniform tar.gz, including Windows

Windows convention is `.zip`, but: (a) Windows 10 1803+ ships `tar.exe`
(bsdtar) that handles `.tar.gz` natively; (b) one format = one assert
class, one extraction path, one binstall `pkg-fmt`; (c) tar preserves
exec bits and stores no ambiguity about directory structure; (d) the
e_opus consult's `cfg(windows) → zip` example was illustrative, not a
ruling. Uniform tgz is the simpler system. Users on pre-1803 Windows /
Server 2016 must supply their own tar (7-Zip/bsdtar); this floor is
documented in `distribution-flavors.md` alongside the asset table.

## Decision 2 — Sidecar format: `sha256sum -c` compatible

`<hex>  <basename>` (GNU coreutils format) verifies with the standard
tool everywhere (including busybox in the alpine image build) and is
what most release tooling emits. No separate manifest file — per-asset
sidecars parallel the assets and keep the release listing self-describing.

## Decision 3 — Probes stay pre-packaging

The flavor capability probes and `--version` asserts keep running against
the bare binary (they execute the binary or string-scan it; scanning a
gz stream would be absurd). Packaging happens after the last probe.

## Decision 4 — Dev harness packs too

The 3 dev legs run the pack+checksum steps (seconds). Tar regressions
then surface in the dev harness, not at the next tag. Docker's dev leg
(gnu variant) exercises verify+extract on every dev run.

## Affected crates

- None. This change touches CI (`.github/workflows/release-matrix.yml`)
  and docs (`docs/src/operations/distribution-flavors.md`). No Rust code,
  no Cargo.toml, no flavor bodies.

## Architecture boundaries

Distribution-plane only: the release pipeline's asset format and its
consumers. Flavor composition, matrix topology, publish gating, and the
-rc.* guard are untouched. The hexagonal core never sees this. rc-5t5fo.8
(binstall) consumes the format defined here via `pkg-fmt = "tgz"`.
