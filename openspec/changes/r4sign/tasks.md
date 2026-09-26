# Tasks: r4sign

All cargo commands run inside the worktree, never the main checkout.
Compile-test batteries use `TMPDIR=/home/shared/tmp`. Key fixtures are
32-byte test seeds written by the tests at runtime — never committed key
material, never real material.

## camel-cli compile (signature envelope)

### Task 1.1: Signature envelope codec

**Files:**
- `Cargo.toml` (modified — add workspace dep `ed25519-dalek = { version = "2.2", features = ["hazmat", "digest"] }`)
- `crates/camel-cli/Cargo.toml` (modified — `ed25519-dalek.workspace = true`)
- `crates/camel-cli/src/compile/signature.rs` (new)
- `crates/camel-cli/src/compile/mod.rs` (modified — `pub mod signature;` + doc line)

**Steps:**
1. Add the workspace dependency at the root `Cargo.toml` (follow the
   existing `blake3` pattern) and reference it from camel-cli. Do NOT add
   any other crate.
2. Create `signature.rs` following the module style of `trailer.rs`
   (module doc describing the byte layout, consts, errors):
   - Consts: `SIGNATURE_MAGIC: [u8; 8] = *b"CAMELSG1"`, `ENVELOPE_VERSION: u16 = 1`,
     `ALGORITHM_ED25519PH: u8 = 1`, `ENVELOPE_LEN: usize = 148`,
     checksum domain `b"rust-camel-signature-v1"`.
   - `EnvelopeError` enum (match `CompileError`'s plain-enum + `Display`
     style, no new error dep): variants at minimum `Truncated`,
     `BadMagic`, `UnsupportedVersion(u16)`, `UnsupportedAlgorithm(u8)`,
     `BadFlags(u8)`, `ChecksumMismatch`, `FingerprintMismatch`,
     `AlgorithmMismatch`, `SignatureInvalid`, `BadKeyFile { path, size }`.
   - `fn fingerprint(verifying_key_bytes: &[u8; 32]) -> String` —
     `"blake3:" + hex(BLAKE3(verifying_key_bytes))`.
   - `fn load_signing_key(path: &Path) -> Result<ed25519_dalek::SigningKey, EnvelopeError>`
     — read file, require exactly 32 bytes (error names path and size).
3. Codec:
   - `fn encode_envelope(key: &SigningKey, message_sha512: &[u8; 64]) -> [u8; ENVELOPE_LEN]`
     using `sign_prehashed(..., None)` (null context); layout per module
     doc: magic, LE u16 version, u8 algorithm, u8 flags=0, 32B verifying
     key, 64B signature, 32B BLAKE3 checksum over
     `b"rust-camel-signature-v1" || 0u8 || version || algorithm || flags || pubkey || signature`,
     terminal magic.
   - `struct VerifiedEnvelope { pub algorithm_name: &'static str, pub fingerprint: String }`
     and `fn verify_envelope(bytes: &[u8], message_sha512: &[u8; 64], expected_fingerprint: &str, expected_algorithm: &str) -> Result<VerifiedEnvelope, EnvelopeError>`
     — parse+validate every framing field and checksum, then fingerprint
     match, then `verify_prehashed(..., None)`. Errors name the failing step.
4. Prehash streaming stays caller-side: document in the module doc that
   callers feed the artifact stream through `ed25519_dalek::Sha512`
   (`Digest` trait) — no message buffering anywhere in this module.

**Tests:** (unit tests in `signature.rs`, seeds via
`SigningKey::from_bytes(&[u8; 32])` with obvious pattern bytes like
`b"r4sign-test-seed-000000000000000\0"` — 32 bytes, clearly non-secret)
- `envelope_roundtrip_and_determinism`: fixed seed + fixed digest → `encode_envelope` twice → byte-identical, len 148, leading+terminal magic `CAMELSG1`, flags byte zero.
- `envelope_parse_rejects_corruption`: for each mutation — truncate to 147, flip leading magic, version 2, algorithm 9, flags 1, flip one checksum byte, flip one pubkey byte (checksum catches) — `verify_envelope` returns the matching named error, never panics.
- `envelope_wrong_fingerprint_and_signature_fail`: valid envelope + expected fingerprint of a DIFFERENT key → `FingerprintMismatch`; valid envelope + modified digest (tampered message) → `SignatureInvalid`.
- `fingerprint_format_is_stable`: fingerprint of a fixed seed equals the exact committed `blake3:<64 lowercase hex>` string (hardcode expected value in the test after first run — public material, not a secret).
- `load_signing_key_rejects_wrong_size`: files of 31 and 33 bytes → `BadKeyFile` naming path and actual size.

**Acceptance:**
- `cargo test -p camel-cli signature` green in the worktree.
- `cargo fmt --check` and both clippy legs clean:
  `cargo clippy -p camel-cli -- -D warnings` and
  `cargo clippy -p camel-cli --no-default-features --features flavor-regular,exec --all-targets -- -D warnings`.

- [x] 1.1

### Task 1.2: Compile-side signing

**Files:**
- `crates/camel-cli/src/commands/compile.rs` (modified)
- `crates/camel-cli/src/compile/manifest.rs` (modified)
- `crates/camel-cli/src/compile/trailer.rs` (modified — schema-pairing widening)
- `crates/camel-cli/tests/compile_command_test.rs` (modified)

**Steps:**
1. `CompileArgs`: add `--sign` (bool), `--signing-key <PATH>` (Option<PathBuf>),
   `--require-signature` (bool). Validation in `run_compile` after the
   existing target/env guards, all rejections `EXIT_REJECTION` (2) with a
   diagnostic naming the broken rule, no artifact written:
   - `--signing-key` without `--sign` → exit 2.
   - `--require-signature` without `--sign` → exit 2.
   - `--sign` with neither `--signing-key` nor `CAMEL_COMPILE_SIGNING_KEY` → exit 2.
   - `--sign` with key file not exactly 32 bytes → exit 2 (path + size named).
   - Argument wins over env when both present.
2. Clean-environment guard carve-out: the `CAMEL_*` filter now allows the
   single name `CAMEL_COMPILE_SIGNING_KEY` through; every other `CAMEL_*`
   variable still rejects as today. If `CAMEL_COMPILE_SIGNING_KEY` is set
   but `--sign` is absent → exit 2 (stray signing variable), diagnostic
   names it and `--sign`.
3. `manifest.rs`: add `MANIFEST_SCHEMA_V4: u64 = 4`; `SigningBlock { algorithm: String, key_fingerprint: String, required: bool }`
   and `signing: Option<SigningBlock>` on the manifest struct
   (`skip_serializing_if = "Option::is_none"` so unsigned output stays
   byte-identical). `derive_for_store` (or the compile call site) gains the
   signing input: signed compiles emit `manifest_schema: 4` + signing block
   (`algorithm: "ed25519ph"`, fingerprint from task 1.1, `required` from
   `--require-signature`); unsigned compiles emit schema 3 unchanged.
   Validation: schema 4 = schema 3 fields PLUS mandatory signing block with
   `algorithm == "ed25519ph"`, fingerprint matching `^blake3:[0-9a-f]{64}$`,
   boolean `required`; readers accept schemas 2, 3, 4, reject others;
   schema 4 without a signing block → error. Pairing rule widened where
   enforced: store 2 pairs with manifest 3 or 4; manifest 3 or 4 requires
   store 2.
4. `trailer.rs` pairing widening: `enforce_schema_pairing` currently requires
   exactly manifest schema 3 for store schema 2 (the `SchemaPairing` error
   carries a single `required_manifest_schema`). Widen it to accept
   store-2 ↔ manifest-{3,4}: adjust the error variant to carry the accepted
   set, update the diagnostic wording and the doc comment so both schemas
   read naturally. `./artifact --manifest` decodes through this path —
   without this step schema-4 artifacts exit 2 before printing.
5. Signing path in `run_compile`: when `--sign`, wrap the existing
   `write_artifact` stream (exe copy + trailer append) so the same byte
   stream feeds a `ed25519_dalek::Sha512` digest — no re-read, no full
   buffering of the artifact. After the artifact rename succeeds, encode
   the envelope from the streamed digest and write `<output>.sig` via
   tmp+rename with mode 0644. If envelope emission fails: remove the
   artifact and the tmp file, exit 2 (no artifact may carry a signed
   manifest without its envelope). Unsigned compiles do no hashing and
   write no `.sig`.

**Tests:** (CLI battery, real binary via `CARGO_BIN_EXE_camel`, seed files
written at runtime)
- `sign_emits_envelope_alongside_artifact`: compile a route with `--sign --signing-key` → exit 0; assert: `<artifact>` exists, `<artifact>.sig` exists, is exactly 148 bytes, mode 0644, starts+ends with `CAMELSG1` (boot-time verification is task 1.3's scope).
- `signed_manifest_records_fingerprint_only`: `./artifact --manifest` → exit 0, `manifest_schema` 4, signing block shows `ed25519ph` + `blake3:` fingerprint + required flag; assert the hex of the private seed appears NOWHERE in artifact, `.sig`, or manifest output.
- `unsigned_compile_stays_byte_identical`: same route compiled twice without `--sign` → artifact files byte-identical, no `.sig` emitted, manifest schema 3, no `signing` key in `--manifest` JSON.
- `sign_input_validation_rejections`: each rule of step 1+2 (arg without --sign; require without --sign; --sign without key source; 31-byte and 33-byte key files; stray `CAMEL_COMPILE_SIGNING_KEY` without `--sign`) → exit 2, diagnostic names the rule, no artifact and no `.sig` left behind.
- `sign_key_from_env`: compile with `CAMEL_COMPILE_SIGNING_KEY` set and no `--signing-key` → exit 0 with `.sig`; both present → arg wins (fingerprint equals the arg key's).
- `require_signature_flag_flows_to_manifest`: `--sign --require-signature` → manifest `required: true`; plain `--sign` → `required: false`.
- `envelope_write_failure_removes_artifact`: make `.sig` path unwritable (pre-create `<artifact>.sig` as a directory) → compile exits 2 and `<artifact>` does not exist afterwards.

**Acceptance:**
- `TMPDIR=/home/shared/tmp cargo test -p camel-cli --test compile_command_test sign` green (filter catches the new tests), plus the full `compile_command_test` battery green.
- `cargo fmt --check` + both clippy legs clean.

- [x] 1.2

### Task 1.3: Run-side verification

**Files:**
- `crates/camel-cli/src/compile/runtime.rs` (modified)
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. `ArtifactArgs::parse`: add `--verify` (no value). Exclusivity: `--verify`
   with any of `--report`/`--help`/`--version`/`--manifest` → existing
   duplicate-exclusive error path (exit 2).
2. Order of operations in `self_detect_artifact` AFTER trailer decode and
   manifest validation succeed: parse `ArtifactArgs` FIRST, then route —
   `--verify` goes to a verify-only chain (step 3) and never reaches the
   boot-verification path; everything else goes to boot verification
   (step 4) before its normal dispatch. This keeps the artifact
   SHA-512-streamed exactly once per invocation.
3. `--verify` handling (no boot): `.sig` path = current exe path + `.sig`.
   Absent → exit 2 "no signature envelope present" (also for
   required-but-missing — the diagnostic names the requirement). Present →
   if the manifest has NO signing block (schema ≤3) → exit 2 "unpaired
   signature envelope"; else stream the artifact file through `Sha512`
   once and call `verify_envelope` with the manifest fingerprint and
   algorithm — on error exit 2 with the named step
   (envelope/fingerprint/signature); on success print
   `algorithm: ed25519ph` and `key_fingerprint: blake3:<hex>` (one line
   each), exit 0. No route/job boot.
4. Boot verification (for every non-`--verify` invocation, including bare
   boot): `.sig` absent → if the manifest signing block exists and
   `required` → `eprintln` diagnostic naming the missing required
   signature, return `Some(2)`; else proceed (v1 compat, no verification,
   zero hashing). `.sig` present → read envelope (IO error → exit 2
   envelope diagnostic); manifest without signing block → exit 2
   "unpaired signature envelope"; else stream the artifact through
   `Sha512` once and `verify_envelope` — on error exit 2 with the named
   step; on success proceed to the normal arg dispatch and boot.
5. Exit-class note: verification failures use the existing
   `EXIT_REJECTION` (2) path and never fall through to Clap.

**Tests:** (runtime battery, one shared signed fixture — extend the
existing `OnceLock` fixture pattern; tampering mutates COPIES, never the
shared artifact)
- `signed_artifact_boots_with_valid_envelope`: signed fixture boots and completes as the unsigned twin does (same exit/output).
- `tampered_exe_body_fails_closed`: copy signed artifact, flip one byte at offset 1000 (exe region), keep `.sig` → starts → exit 2, stderr names signature failure, no boot.
- `tampered_trailer_span_fails_closed`: flip one byte inside the trailer manifest span → exit 2 (trailer integrity diagnostic), no boot.
- `wrong_key_envelope_fails_closed`: build envelope with a second key over the untampered artifact digest (unit-level helper or recompile-twin approach), place as `.sig` → exit 2 fingerprint mismatch.
- `corrupt_envelope_fails_closed`: truncate `.sig` by one byte → exit 2 envelope diagnostic.
- `required_signature_missing_envelope_fails_closed`: compile `--sign --require-signature` twin, delete `.sig` → exit 2 naming required signature.
- `unsigned_with_stray_envelope_fails_closed`: unsigned artifact + any `.sig` → exit 2 unpaired envelope.
- `unsigned_without_envelope_still_runs`: unsigned artifact, no `.sig` → boots (existing behavior regression guard).
- `verify_flag_roundtrip_and_output`: signed fixture `--verify` → exit 0, stdout contains `ed25519ph` and the manifest fingerprint; unsigned artifact `--verify` → exit 2 no envelope.
- `verify_stays_exclusive`: `--verify --manifest` → exit 2 duplicate-exclusive, no manifest print.

**Acceptance:**
- `TMPDIR=/home/shared/tmp cargo test -p camel-cli --test compiled_artifact_test` green (full battery).
- `cargo fmt --check` + both clippy legs clean.

- [x] 1.3

### Task 1.4: ADR and documentation

**Files:**
- `docs/adr/0083-artifact-signing-envelope.md` (new)
- `docs/adr/0075-self-contained-executable-artifact-format.md` (modified — short amendment pointing to 0083)
- `docs/src/cli/compile.md` (modified)
- `CONTEXT-MAP.md` (modified — glossary: `signature envelope`, `key fingerprint`, cite ADR-0083)
- `crates/camel-cli/CONTEXT.md` (modified — signing surface in the compile section)

**Steps:**
1. Read `docs/adr/0082-*.md` for the house ADR format, then write
   ADR-0083: Context (integrity vs authenticity, strip-removes-trailer
   reality), Decision (detached envelope layout table copied from
   `design.md`, Ed25519ph streaming rationale, fingerprint binding chain,
   manifest schema 4, fail-closed matrix, key handling rules, v1 compat),
   Consequences (incl. deferred third-party key pinning — operators pin by
   fingerprint comparison; schema-4 artifacts need R4-capable readers).
2. Append a one-paragraph amendment note to ADR-0075: R4 signs outside the
   trailer checksum domain via a detached envelope, decision recorded in
   ADR-0083.
3. `docs/src/cli/compile.md`: add `--sign`/`--signing-key`/`--require-signature`
   to Usage, a short "Signing artifacts" section (envelope file, `--verify`,
   fingerprint pinning guidance), keep STE style.
4. Glossary terms `signature envelope` and `key fingerprint` in
   CONTEXT-MAP.md with ADR citations; mirror one-paragraph summary in
   `crates/camel-cli/CONTEXT.md`.

**Tests:**
- `cargo xtask lint-context-citations` green.
- `grep -c "ADR-0083\|ADR 0083"` in the four updated docs ≥ 1 each.

**Acceptance:**
- ADR numbering and format consistent with `docs/adr/` house style; English prose per language policy; `cargo xtask lint-context-citations` green.

- [x] 1.4
