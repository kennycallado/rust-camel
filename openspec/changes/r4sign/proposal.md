# Proposal: r4sign

## Why

Compiled artifacts are self-contained deployment units, but nothing proves
who produced them. The CAMELTR1 trailer gives integrity (BLAKE3); it cannot
give authenticity. Roadmap R4 (epic `rc-rye74`, bd `rc-osv9x`) closes this
gap: a signature over the FINAL artifact bytes, applied after compile, never
before. The cli-compile spec already sanctions the R4 signature-verification
surface in its non-goals prose.

## What Changes

- **Outer envelope, detached.** `camel compile --sign --signing-key <path>`
  emits a fixed-size `<artifact>.sig` envelope (`CAMELSG1` framing) beside
  the artifact. The signature covers the complete final artifact bytes
  (executable copy plus trailer) using Ed25519ph (RFC 8032 prehash), so a
  multi-hundred-MiB artifact is signed and verified with flat memory. The
  envelope carries the public key, the signature, and a BLAKE3 envelope
  checksum. No key material enters the artifact.
- **Fingerprint, not keys.** A signed artifact's manifest moves to schema 4
  and records only the algorithm, the BLAKE3 fingerprint of the public key,
  and a required bit. Unsigned compiles stay manifest schema 3 and stay
  byte-identical to today.
- **Fail-closed verify at boot.** When the envelope is present, the artifact
  verifies signature, envelope, and fingerprint-to-manifest binding before
  boot; any mismatch exits 2. Unsigned artifacts without an envelope still
  run (v1 compat). `--require-signature` at compile bakes the required bit
  into the signed manifest; boot then refuses a missing or invalid envelope.
- **`--verify` artifact argument.** The sanctioned R4 surface: verify without
  booting, exit 0 naming algorithm and fingerprint, exit 2 on any failure.
- **ADR-0083** records the envelope decision in the ADR-0075 amendment chain.

Excluded: third-party key pinning / trust stores (operator compares
fingerprints), key management, R6 compression, R7 cross-target.

## Acceptance criteria

- Sign + verify round trip: `--sign` emits `.sig`; `--verify` exits 0; boot
  proceeds with a valid envelope.
- Tampered payload fails closed: any byte flipped in the artifact → boot and
  `--verify` exit 2.
- Wrong key fails closed: envelope re-signed under a different key →
  fingerprint mismatch → exit 2.
- Unsigned + required fails closed: `--require-signature` artifact without a
  valid envelope → exit 2 before boot.
- Unsigned artifacts without an envelope run unchanged (v1 compat).
- Manifest records algorithm + fingerprint + required only; no key material
  in any output or log; key fixtures are test seeds only.
- Full gate battery passes; unsigned compile output stays byte-identical.

## Risk budget

Acceptable: one new direct dep (`ed25519-dalek 2`, already in the workspace
lock graph); manifest schema 4 rejected by pre-R4 readers (signed artifacts
are new — no legacy signed artifacts exist). Out of bounds: key material in
artifacts or logs, changes to trailer framing, any widening of the artifact
argument surface beyond `--verify`, changes to BLAKE3's integrity role.
