# Design: r4sign

## Approach

**Envelope format.** Detached sidecar `<artifact>.sig`, fixed 148 bytes,
house trailer style:

```
off  len  field
0    8    magic "CAMELSG1"
8    2    u16 LE envelope version = 1
10   1    u8 algorithm = 1 (Ed25519ph, RFC 8032 §5.1, null context)
11   1    u8 flags = 0 (reserved, must be zero)
12   32   ed25519 verifying key (compressed)
44   64   signature
108  32   BLAKE3 over b"rust-camel-signature-v1" || 0u8 || version ||
          algorithm || flags || pubkey || signature
140  8    terminal magic "CAMELSG1"
```

**Why Ed25519ph, not pure ed25519.** The signature must cover the FINAL
artifact bytes (bd `rc-osv9x` AC); artifacts are the full camel executable
(~287 MiB in tests). Pure ed25519 needs the whole message in memory.
Ed25519ph hashes the message with SHA-512 while it streams, then signs the
64-byte digest — flat memory, RFC-standard, deterministic. Verify streams
the same way. The envelope records algorithm id 1 so future algorithms fit
without format change.

**Fingerprint.** `blake3:` + 64 hex chars over the 32-byte verifying key.
Identity only — independent of the signature prehash mode.

**Manifest schema 4.** New optional `signing` block
(`{ algorithm: "ed25519ph", key_fingerprint: "blake3:<hex>", required: bool }`),
serialized only when signing. Unsigned compiles emit schema 3 byte-identical
to today. Schema 4 validation = schema 3 fields plus mandatory signing block.
Pairing widens: store 2 pairs with manifest 3 or 4; manifest 4 requires
store 2. Readers accept 2, 3, 4.

**Compile side** (`commands/compile.rs`, `compile/trailer.rs`,
`compile/manifest.rs`, new `compile/signature.rs`). New flags: `--sign`,
`--signing-key <PATH>`, `--require-signature` (requires `--sign`). Key file
must be exactly 32 bytes (ed25519 seed); path from arg or
`CAMEL_COMPILE_SIGNING_KEY` env (arg wins). The clean-environment guard keeps
rejecting every other `CAMEL_*` variable, and rejects a stray
`CAMEL_COMPILE_SIGNING_KEY` when `--sign` is absent — the variable is a
signing input, not configuration. Signing never touches embedded content.
`write_artifact` already streams exe→tmp→rename; the same stream feeds a
SHA-512 prehash digest, so the signature is computed without re-reading or
buffering the artifact. After the artifact rename succeeds, the envelope is
written `<output>.sig` via tmp+rename, mode 0644 (public material). Envelope
failure removes the artifact and exits 2 — no unsigned output may carry a
signed manifest.

**Run side** (`compile/runtime.rs`). `ArtifactArgs` gains `--verify`
(exclusive with all other flags). In `self_detect_artifact`, after trailer
decode and manifest validation succeed:

| envelope | manifest signing | result |
|---|---|---|
| absent | absent / not required | boot (v1 compat) |
| absent | required | exit 2 |
| present | absent (schema ≤3) | exit 2 — unpaired envelope |
| present | present | verify chain: envelope parse → algorithm match → fingerprint(pubkey) == manifest → Ed25519ph over artifact bytes. Valid → boot; any step fails → exit 2 with a named diagnostic |

`--verify` runs the same chain without booting: exit 0 prints algorithm and
fingerprint; every failure exits 2. Verification failures are exit 2 (boot
class), consistent with existing integrity handling.

**Binding chain (why wrong-key fails).** The manifest — including the
fingerprint — lives inside the artifact bytes; the signature covers those
bytes; the envelope key must hash to the manifest fingerprint. Re-signing
under another key breaks the fingerprint match; editing the manifest breaks
the signature; swapping the envelope breaks both.

## Affected crates

- `camel-cli`: `src/compile/signature.rs` (new codec + verify), `src/commands/compile.rs`
  (flags, key loading, env carve-out, envelope emission), `src/compile/manifest.rs`
  (schema 4 + signing block), `src/compile/trailer.rs` (docs only), `src/compile/runtime.rs`
  (`--verify`, boot verify chain), `tests/compile_command_test.rs`,
  `tests/compiled_artifact_test.rs`.
- Root + camel-cli `Cargo.toml`: `ed25519-dalek = "2"` with `hazmat` + `digest` features (the prehashed API is digest-gated)
  (prehashed API); no other new deps (SHA-512 via `ed25519_dalek::Sha512`,
  BLAKE3 already present).
- Docs: ADR-0083 (new), ADR-0075 pointer amendment, `docs/src/cli/compile.md`,
  `CONTEXT-MAP.md` glossary, `camel-cli/CONTEXT.md`.

## Architecture boundaries

Signing is a CLI-boundary concern: compile output post-processing plus
pre-boot runtime gate. No component, DSL, core, or service crate changes.
The artifact argument surface grows only by the pre-sanctioned `--verify`.
BLAKE3 keeps its integrity role untouched — the signature adds authenticity
on top, outside the trailer checksum domain.

## Alternatives considered

- **Append-after-trailer signature block** — rejected: breaks `read_probe_tail`'s
  EOF-anchored framing for every existing reader; detached sidecar changes zero
  artifact bytes.
- **Pure ed25519 with full buffering** — rejected: ~287 MiB RSS spike per
  sign/verify, multiplied across parallel tests.
- **Fingerprint or signature inside the trailer** — rejected: BLAKE3 checksum
  domain must stay integrity-only (bd AC); trailer framing must not change.
- **Third-party trust store / key pinning flags** — deferred: operators pin by
  comparing fingerprints from `--verify`/`--manifest`; recorded in deferrals.
- **Runtime `--require-signature` flag** — rejected: a boot-time flag cannot
  enforce policy an attacker can simply omit; the compile-time bit travels
  inside the signed manifest.
