# ADR-0083: Artifact signing envelope (detached Ed25519ph sidecar)

- Status: Accepted (decided 2026-09-26; roadmap R4, epic rc-rye74, bd rc-osv9x); Amended 2026-09-26: truststore pinning and rollback freshness (keypin change)
- Source: bd rc-osv9x (R4 acceptance criteria); openspec change `r4sign`
- Amends: ADR-0075 (adds the R4 signing surface; trailer framing is unchanged)

## Context

A compiled artifact is a copy of the Camel executable with a `CAMELTR1`
trailer appended (ADR-0075). The trailer carries a BLAKE3 checksum. That
checksum proves integrity: a changed byte breaks it. It does not prove
authenticity. Anyone who can edit the artifact can recompute the checksum
and rebuild the trailer.

Stripping the trailer removes the checksum with it. The image then looks
like a trailer-free executable. The self-detect probe treats it as ordinary
`camel` and never validates it. BLAKE3 therefore cannot bind an artifact to
the identity that produced it.

The signature must cover the final artifact, not an intermediate product.
The final bytes are the executable copy plus the appended trailer. A host
executes exactly those bytes. The signer must see that whole stream, so
signing runs after compile, when the artifact is complete.

## Decision

**Detached envelope.** A signed artifact gets a sidecar file
`<artifact>.sig`. The envelope is fixed at 148 bytes:

| offset | length | field |
|--------|--------|-------|
| 0 | 8 | magic `CAMELSG1` |
| 8 | 2 | little-endian envelope version = 1 |
| 10 | 1 | algorithm = 1 (Ed25519ph) |
| 11 | 1 | flags = 0 (reserved, must be zero) |
| 12 | 32 | Ed25519 verifying key (compressed) |
| 44 | 64 | signature |
| 108 | 32 | BLAKE3 checksum |
| 140 | 8 | terminal magic `CAMELSG1` |

The checksum covers `b"rust-camel-signature-v1" || 0u8 || version ||
algorithm || flags || pubkey || signature`. The envelope carries public
material only: the verifying key, the signature, and the checksum. No key
material enters the envelope or the artifact.

**Ed25519ph, not pure Ed25519.** Pure Ed25519 needs the whole message in
memory. An artifact is the full Camel executable (about 287 MiB in tests).
Ed25519ph (RFC 8032 §5.1 prehash with a null context) hashes the message
with SHA-512 while it streams, then signs the 64-byte digest. Signing and
verification both use flat memory and one stream per invocation. The
compile side feeds the same byte stream that writes the artifact (executable
copy plus trailer), with no re-read and no buffering. Boot verification
streams the artifact file the same way. The algorithm id is recorded in the
envelope, so a future algorithm fits without a format change.

**Fingerprint binding chain.** The manifest records the algorithm, the
`key_fingerprint`, and a `required` bit. The fingerprint is `blake3:` plus
64 lowercase hex characters over the 32-byte verifying key. The binding
chain is:

- The manifest, including the fingerprint, is part of the artifact bytes.
- The signature covers those bytes.
- The envelope's verifying key must hash to the manifest fingerprint.

The chain proves that the artifact bytes were signed by the holder of the
private key whose public key hashes to the manifest fingerprint. Re-signing
under another key breaks the fingerprint match. Editing the manifest breaks
the signature. Swapping the envelope breaks both. The chain does not
establish that the key holder is trusted; see Consequences.

**Manifest schema 4.** A signed compile emits `manifest_schema` 4. Schema 4
is schema 3 plus a mandatory `signing` block:
`{ algorithm: "ed25519ph", key_fingerprint: "blake3:<hex>", required: bool }`.
The block serializes only when signing, so unsigned compiles stay schema 3
and byte-identical. Readers accept schemas 2, 3, and 4. Schema 4 without a
valid signing block is rejected. Store schema 2 pairs with manifest schema 3
or 4; manifest schema 3 or 4 requires store schema 2. `--require-signature`
sets `required: true`.

**Fail-closed verification.** Boot and `--verify` use the same chain. The
verdict table is:

| envelope | manifest signing block | result |
|----------|------------------------|--------|
| absent | absent, or `required: false` | boot with no signature hashing (v1 compatibility; the trailer checksum still verifies) |
| absent | `required: true` | exit 2; the diagnostic names the missing required signature |
| present | absent (manifest schema ≤ 3) | exit 2; unpaired signature envelope |
| present | present | parse envelope → match algorithm → match `fingerprint(pubkey)` to the manifest fingerprint → verify Ed25519ph over the artifact bytes. Valid → boot. Any step fails → exit 2 with the failing step named. |

`--verify` runs the chain without a boot. Exit 0 prints `algorithm:
ed25519ph` and `key_fingerprint: blake3:<hex>`, one line each. Every failure
exits 2 and names the failing step (envelope, fingerprint, or signature).
`--verify` is exclusive with every other artifact argument. Verification
failures use exit 2, the existing rejection class.

**Key handling.** The key file holds exactly 32 bytes: an Ed25519 seed. The
path comes from `--signing-key <PATH>` or the `CAMEL_COMPILE_SIGNING_KEY`
environment variable; the argument wins when both are present. These rules
hold:

- `--signing-key` without `--sign` exits 2.
- `--require-signature` without `--sign` exits 2.
- `--sign` with no key source exits 2.
- `--sign` with a key file that is not exactly 32 bytes exits 2, naming the
  path and the size.
- The clean-environment guard accepts only `CAMEL_COMPILE_SIGNING_KEY`; every
  other `CAMEL_*` variable still rejects. The variable is a signing input,
  not configuration. A stray `CAMEL_COMPILE_SIGNING_KEY` without `--sign`
  rejects.

The envelope is written `<artifact>.sig` after the artifact rename, through
tmp plus rename, mode 0644 (public material). If the envelope write fails,
the compiler removes the artifact and the tmp file and exits 2. No artifact
may carry a signed manifest without its envelope.

**v1 compatibility.** An unsigned artifact without an envelope boots without
signature hashing (the trailer checksum still verifies). Unsigned compiles stay schema 3 and byte-identical. Readers
accept schemas 2, 3, and 4. An artifact with manifest schema ≤ 3 and a stray
envelope exits 2 as unpaired.

**Magic spelling.** The plan prose writes `CAMELSIG1`. That string is nine
bytes, and the magic field is eight bytes, so the literal cannot fit. The
fixed spelling is `CAMELSG1`: `CAMEL` plus the two-letter domain code `SG`
plus the format version `1`, mirroring `CAMELTR1` (`CAMEL` plus `TR` plus
`1`). This is a ratified correction to the plan, not a format change.

## Consequences

A schema-4 artifact needs an R4-capable reader. Pre-R4 readers reject
schema 4. This is acceptable because signed artifacts are new: no legacy
signed artifact exists.

Third-party key pinning and trust stores are deferred. The format carries
no trust anchor and no key management. An operator pins a producer by
comparing the `key_fingerprint` printed by `--verify` or `--manifest`
against a known value from that producer. A verified envelope proves
possession of the private key; it does not prove that the producer is
trusted.

BLAKE3 keeps its integrity role in the trailer. Signing adds authenticity
on top of it and stays outside the trailer checksum domain. The trailer
framing does not change. The artifact argument surface grows only by the
sanctioned `--verify`. Because the signature covers the complete final
bytes, stripping the trailer or mutating the executable fails verification.

Signing protects against tamper, not confidentiality. An artifact that
embeds a private key stays a secret. Key material must never enter the
artifact, the envelope, the manifest, or a log.

## Amendment: truststore pinning and rollback freshness (2026-09-26)

The `keypin` change adds a deployment-owned truststore and a rollback
freshness rule. It amends the Consequences deferral of third-party key
pinning and trust stores.

**Truststore.** A truststore is a single text file owned by the
deployment. Each non-empty, non-comment line pins one verifying key as
`blake3:<64 lowercase hex>`, the same form as the manifest
`key_fingerprint`, optionally followed by a recorded freshness floor.
The truststore holds public fingerprints and floors only; no key
material enters it. It is supplied by the artifact argument
`--truststore <PATH>` or the `CAMEL_TRUSTSTORE` environment variable;
the argument wins. Malformed input fails closed with exit 2: a
file-level failure names the path, a line-level failure names the path
and the line. An empty truststore pins nothing, so every signed
artifact fails its pin check. The truststore does not apply to unsigned
artifacts: unsigned compiles stay schema 3 and boot as before. Under a
supplied truststore, a manifest with a signing block and no envelope
fails closed: deleting the `.sig` file must not convert a signed
artifact into an unsigned one.

**Freshness.** A signed compile emits manifest schema 5: schema 4 plus
a mandatory `freshness` marker in the signing block, encoded as u64
unix-seconds. The marker lives inside the signed byte domain, so an
attacker cannot change it without breaking the signature. The
truststore records, per pinned key, the highest previously accepted
marker (the floor). At verify time, a marker below the floor fails
closed as a rollback with exit 2; a marker at or above the floor is
accepted. The first sight of a pinned key records its marker as the
floor. A pre-keypin schema-4 artifact passes pin-only until its key has
a recorded floor; afterwards it fails as a rollback — the first
schema-5 boot of a key is the migration boundary, and it does not
re-open. Rollback detection is bounded by the marker's one-second
resolution: artifacts compiled in the same second are not orderable by
the marker. Boot records the floor inside a lock-serialized critical
section (an advisory lock on `<truststore>.lock`) so concurrent boots
cannot accept on a stale floor or lower a recorded one; `--verify` is
a dry run and does not lock or write. A boot that must record a floor
and cannot lock or write the truststore fails closed.

**Compatibility.** Without a truststore, behavior is unchanged from R4:
the self-contained chain verifies and boots, and `--verify` prints the
algorithm and fingerprint. With a truststore, a signed artifact whose
key is not pinned fails closed, a signed manifest without its envelope
fails closed, and a pinned artifact below its floor fails closed.
Readers accept manifest schemas 2, 3, 4, and 5. The 148-byte envelope
framing does not change.

## References

- bd rc-osv9x (R4 acceptance criteria); epic rc-rye74
- ADR-0075 (self-contained executable artifact format; amended by this ADR)
- RFC 8032 (EdDSA; §5.1 Ed25519ph)
- `crates/camel-cli/src/compile/signature.rs` (envelope codec)
- openspec change `r4sign`
