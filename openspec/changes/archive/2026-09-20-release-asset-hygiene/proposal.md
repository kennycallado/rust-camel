# Proposal: release-asset-hygiene

## Why

v0.50.0 shipped eleven raw, uncompressed binaries (30–80 MB each) with no
integrity verification. Downloaders cannot detect corruption or tampering,
and the size is wasteful for an edge-oriented product (slim exists
precisely for small footprints). `cargo-binstall` support (bd rc-5t5fo.8,
e_opus consult `docs/audits/2026-09-19-binstall-metadata-design-consult.md`)
is blocked on this change: binstall metadata must be written once against
the FINAL asset format, and raw-bin is not it.

## What Changes

- Every release asset becomes `<artifact-name>.tar.gz` (binary at archive
  root, exec bit preserved by tar; Windows ships `camel.exe` inside the
  tarball, same extension policy as today).
- Every tarball gets a `.sha256` sidecar in `sha256sum -c` format —
  11 tarballs + 11 sidecars = 22 release files.
- Build legs pack and checksum after the existing flavor probes (probes
  keep running on the bare binary, pre-packaging).
- The docker job verifies the sidecar before extracting its flavor-matched
  tarballs (consumption-point integrity).
- The release-collection assert is updated for the 22-file topology
  (classification by tarball name; sidecars must match their tarballs).
- Docs (`distribution-flavors.md`) updated; release notes of the first
  tarball release announce the raw-name retirement (one-time break,
  same loud-break policy as the desktop clean-name retirement).

Explicitly excluded: binstall metadata (rc-5t5fo.8, lands against this
format), signing/cosign (out of scope), asset renaming beyond the
extension, zstd/zip alternatives (uniform tgz — see design).

## Acceptance criteria

- A tag run uploads exactly 11 `.tar.gz` + 11 `.sha256` files, no raw
  binary names.
- Every sidecar verifies: `sha256sum -c <file>.sha256` passes against its
  tarball on the release, and the docker job fails on checksum mismatch.
- Docker variants still smoke-assert their flavor suffix after extraction.
- Dev harness exercises packaging on its 3 legs (cheap, catches tar
  regressions early).
- Breaking change documented: scripts fetching `camel-<target>` raw names
  from v0.50.0 must switch to `camel-<target>.tar.gz` from the next release.

## Risk budget

Acceptable: one-time asset-name break (early adopter base, announced);
marginal CI time for tar+checksum (~seconds per leg). Out of bounds:
touching flavor composition, matrix topology, or the publish gating; any
change to the -rc.* prerelease guard; GPG signing.

Bd: rc-5t5fo.2 (blocks rc-5t5fo.8).
