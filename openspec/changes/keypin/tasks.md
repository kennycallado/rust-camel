# Tasks: keypin

## Phase 1: Pinning — deployment truststore closes the third-party anchor gap

### camel-cli artifact surface

#### Task 1.1: `--truststore` modifier argument and `CAMEL_TRUSTSTORE` supply surface

**Files:**
- `crates/camel-cli/src/compile/runtime.rs` (modified — `ArtifactArgs` field + parse rules + env read)
- `crates/camel-cli/src/commands/compile.rs` (modified — clean-environment guard carve-out)
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified — new integration tests)

**Steps:**
1. Add `pub truststore: Option<PathBuf>` to `ArtifactArgs` in `runtime.rs` (default `None` in the
   constructor used by tests and the parse path).
2. Extend the artifact-argument parser where `--report`/`--verify` are handled: `--truststore`
   consumes the next argument as its value. Rules: a missing trailing value exits 2 with the
   existing argument diagnostic; `--truststore` twice exits 2; `--truststore` alongside `--help`,
   `--version`, or `--manifest` exits 2 with the existing duplicate-exclusive diagnostic (it is a
   modifier that pairs with boot and `--verify` only); `--truststore` with `--verify` and with
   `--report` is accepted.
3. Add env fallback in the same place `ArtifactArgs` is finalized: when `args.truststore` is `None`
   and `CAMEL_TRUSTSTORE` is set and non-empty, use it as the path. The argument wins when both are
   present. Store the resolved path once; later tasks read only `args.truststore`.
4. In `commands/compile.rs`, extend the clean-environment guard's accepted-variable list with
   `CAMEL_TRUSTSTORE`, with a comment: verify-side input, never read at compile time, benign in any
   compile (mirrors the existing `CAMEL_COMPILE_SIGNING_KEY` comment style at the guard).

**Tests:** (integration tests beside the existing artifact-argument tests in
`crates/camel-cli/tests/compiled_artifact_test.rs`, run with
`cargo test -p camel-cli --test compiled_artifact_test truststore`; one unit test in `runtime.rs`)
- `truststore_argument_requires_value`: valid artifact, last argument `--truststore` -> exit 2,
  diagnostic names the argument.
- `truststore_rejected_with_exclusive_modes`: for each of `--manifest`, `--help`, `--version`:
  `artifact --truststore <ts> <mode>` -> exit 2.
- `artifact_args_resolve_env_truststore` (unit, `runtime.rs` `#[cfg(test)]`): (a) parse artifact
  arguments without the flag while `CAMEL_TRUSTSTORE` is set -> `truststore` is `Some(env path)`;
  (b) parse with both the flag and the env var -> `truststore` is the flag path (argument wins).
  Set/clear the variable through the suite's established unsafe-`set_var` discipline serialized by
  a local `static Mutex` guard (the repo's pattern for environment-dependent unit tests — do not
  add a serial-test dependency).
- `compile_with_camel_truststore_env_is_benign`: `camel compile` with `CAMEL_TRUSTSTORE` set and no
  `--sign` -> compile succeeds (the guard does not reject it).

**Acceptance:**
- `cargo test -p camel-cli --test compiled_artifact_test truststore` passes.
- `cargo test -p camel-cli --lib artifact_args_resolve_env_truststore` passes.
- `cargo clippy -p camel-cli --all-targets -- -D warnings` exits 0.

- [x] 1.1

### camel-cli verify chain

#### Task 1.2: Truststore codec and the pin check plus strip rule at both verify sites

The codec and its callers land in ONE task: the module is `pub(crate)`, so shipping it without a
production caller would trip the lib target's dead-code lint and fail this task's own clippy gate.

**Files:**
- `crates/camel-cli/src/compile/trust.rs` (new)
- `crates/camel-cli/src/compile/mod.rs` (modified — add `mod trust;`)
- `crates/camel-cli/src/compile/runtime.rs` (modified — trust-policy helper hooked into
  `verify_for_boot` and `run_verify_only`)
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified — new integration tests)

**Steps:**
1. Create `trust.rs` with `pub(crate) struct PinEntry { fingerprint: String }` and
   `pub(crate) struct TrustStore { entries: Vec<PinEntry> }`. Floor STORAGE lands in Task 2.2 with
   its first production reader; Phase 1 only VALIDATES the optional floor column so the file
   format is frozen from day one (see step 2).
2. Implement `TrustStore::parse(path: &Path) -> Result<TrustStore, TrustError>`: read the file
   (failure -> `TrustError::Unreadable { path }`); decode UTF-8 (failure ->
   `TrustError::NotUtf8 { path }`); split lines; skip blank lines and lines whose first non-space
   char is `#`; each other line is whitespace-separated tokens: exactly one token
   `blake3:<64 lowercase hex>` or two tokens where the second parses as `u64` decimal (reject `-`,
   `+`, whitespace-padded, non-digit forms — the validated floor value is discarded in this task).
   Any other shape -> `TrustError::MalformedEntry { path, line }` (1-based line number). A
   fingerprint already pinned -> `TrustError::DuplicatePin { path, line }`. Zero-byte files and
   comments/blank-only files parse to a valid empty `TrustStore`.
3. Implement `TrustStore::is_pinned(&self, fingerprint: &str) -> bool` (exact string match on the
   `blake3:<hex>` form).
4. Define `pub(crate) enum TrustError` with the parse-time variants `Unreadable { path }`,
   `NotUtf8 { path }`, `MalformedEntry { path, line }`, `DuplicatePin { path, line }`; implement
   `Display` so each message starts with the step token `truststore-parse` and names the path, plus
   `:line N` for entry variants. (Task 2.2 adds `LockFailure`, `Unwritable`, `Io` when the boot
   path first constructs them.)
5. In `runtime.rs`, hook the trust policy into both `verify_for_boot` and `run_verify_only` through
   one shared helper so the sites cannot drift. Two hook points inside it:
   - The envelope-absent branch in EACH site (the boot continuation for `required: false`, and
     `run_verify_only`'s no-envelope rejection — both currently fire before any trust step): when a
     truststore is supplied and the manifest carries a signing block (schema 4 or 5), exit 2 with a
     `truststore-pin` diagnostic naming the manifest `key_fingerprint` (strip rule, both sites per
     the spec scenario). This holds for ANY required bit — under a truststore the strip-rule
     diagnostic governs; the pre-existing `required: true` missing-envelope diagnostic keeps its
     current form only on the no-truststore path. Schema-3-or-earlier manifests keep their existing
     diagnostics everywhere.
   - After `verify_envelope_bytes` fully passes (envelope present): parse the truststore
     (`TrustStore::parse`), and require `is_pinned(manifest.key_fingerprint)`; otherwise exit 2
     with a `truststore-pin` diagnostic naming the fingerprint. A `TrustError` from parse prints
     the `truststore-parse` diagnostic and exits 2.
   - Manifest schema 3 or earlier (no signing block): the truststore is not consulted anywhere;
     existing behavior, including the stray-envelope exit 2, is unchanged.
6. Keep all diagnostics on stderr, prefixed `camel:` like the neighboring artifact diagnostics,
   and keep exit code 2 (the existing rejection class).
7. Do not read the truststore when no path was supplied: zero new file reads on the R4 path.

**Tests:** (unit tests in `trust.rs` run with `cargo test -p camel-cli --lib trust`; integration
tests beside the r4sign signing tests in `crates/camel-cli/tests/compiled_artifact_test.rs` run
with `cargo test -p camel-cli --test compiled_artifact_test`. REUSE the suite's shared OnceLock
fixtures — `Fixture.signed_job` / `signed_required_job` plus the deploy-to-tempdir pattern — for
every test that needs a signed artifact; do NOT compile new 287 MiB artifacts per test.
Truststores are tiny hand-written text files in the test tempdir.)
- `truststore_parse_accepts_pins_comments_and_valid_floor_column`: temp file with two comment
  lines, one blank line, one pin without floor, one pin with a valid floor token `42` -> parse Ok;
  entries in file order (the floor value is validated, not stored, in this task).
- `truststore_parse_rejects_malformed_line`: line 3 is `not-a-pin` -> `MalformedEntry` whose
  Display contains the path and `line 3`.
- `truststore_parse_rejects_duplicate_pin`: same pin on lines 1 and 4 -> `DuplicatePin` naming
  line 4.
- `truststore_parse_rejects_non_utf8`: bytes `0x62 0x6c 0x61 0x6b 0x65 0x33 0x3a 0xff` ->
  `NotUtf8` naming the path.
- `truststore_parse_unreadable_names_path`: path to a missing file -> `Unreadable` naming the
  path.
- `truststore_empty_and_comments_only_files_pin_nothing`: a zero-byte file and a
  comments/blank-only file both parse Ok with `is_pinned(any) == false` for the manifest
  fingerprint form.
- `truststore_rejects_bad_fingerprint_shapes`: uppercase hex, 63 hex chars, missing `blake3:`
  prefix, 65 hex chars -> all `MalformedEntry`.
- `truststore_floor_column_must_be_decimal_u64`: floor tokens `abc`, `-1`, `+1`, `1.5` ->
  `MalformedEntry`.
- `pinned_key_verifies_under_truststore`: deploy the signed fixture; write a truststore file with
  its fingerprint (read from the fixture's `--verify` output or manifest JSON); (a)
  `artifact --truststore <ts>` boots; (b) `artifact --verify --truststore <ts>` exits 0.
- `env_truststore_supplies_and_argument_wins`: (a) `CAMEL_TRUSTSTORE` set to a pinning truststore,
  no argument -> `--verify` exits 0; (b) env set to a pinning truststore AND `--truststore`
  pointing at a comments-only (empty) truststore -> `--verify` exits 2 with a `truststore-pin`
  diagnostic, proving the argument path was used.
- `unpinned_key_fails_closed_at_boot_and_verify`: truststore pins a different valid fingerprint ->
  (a) boot exits 2 with `truststore-pin` and the manifest fingerprint in the diagnostic; (b)
  `--verify --truststore <ts>` exits 2 the same way.
- `stripped_envelope_fails_closed_under_truststore`: deploy the signed fixture (required bit NOT
  set); delete `<artifact>.sig`; boot with truststore -> exit 2, diagnostic contains
  `truststore-pin` and the manifest fingerprint; the same artifact+truststore under `--verify`
  also exits 2 with the same diagnostic.
- `malformed_truststore_fails_closed_at_boot`: (a) truststore with a malformed line 2 -> boot
  exits 2, diagnostic contains the path and `line 2`; (b) truststore path pointing at a missing
  file -> boot exits 2, diagnostic names the path.
- `empty_truststore_pins_nothing_at_boot`: comments-only truststore -> boot exits 2
  `truststore-pin`.
- `unsigned_artifact_ignores_truststore`: unsigned fixture, truststore supplied -> boots
  unchanged.
- `no_truststore_r4_chain_unchanged`: the full existing r4sign signing test set passes unmodified
  (verified by the suite run; no test edits in this task).

**Acceptance:**
- `cargo test -p camel-cli --lib trust` passes.
- `cargo test -p camel-cli --test compiled_artifact_test` passes (all new + all pre-existing).
- `cargo test -p camel-cli --test compile_command_test` passes (compile surface untouched).
- `cargo clippy -p camel-cli --all-targets -- -D warnings` exits 0.
- No `unwrap()` on user-input paths (lint-unwrap clean: `cargo xtask lint-unwrap`).

- [x] 1.2

## Phase 2: Freshness — schema-5 marker and floors close the rollback gap

### camel-cli manifest

#### Task 2.1: Manifest schema 5 with the signed freshness marker

**Files:**
- `crates/camel-cli/src/compile/manifest.rs` (modified — schema 5, `freshness` field, strictness,
  reader acceptance, pairing rule, canonical writer)
- `crates/camel-cli/src/compile/trailer.rs` (modified — accepted manifest-schema list for store
  pairing, doc comments)
- `crates/camel-cli/src/compile/runtime.rs` (modified — `requires_signature` and
  `verify_envelope_bytes` accept schema 5)
- `crates/camel-cli/src/commands/compile.rs` (modified — signed compiles emit schema 5)
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified — new integration test)

**Steps:**
1. In `manifest.rs`: add `pub const MANIFEST_SCHEMA_V5: u64 = 5;` beside the existing schema
   constants (the existing schema constants and the `manifest_schema` field are `u64` — a `u32`
   constant will not type-check against them).
2. Add `pub freshness: Option<u64>` to `SigningBlock` with serde attribute so the field
   serializes only when `Some`.
3. Extend decode strictness: schema 5 requires the signing block AND `freshness: Some(_)`; schema 4
   requires the signing block AND `freshness: None`; both violations reject with the existing
   per-schema strictness error pattern. Reader acceptance becomes schemas 2, 3, 4, and 5; the
   pairing rule becomes: manifest schemas 3, 4, and 5 pair with store schema 2, and store schema 2
   pairs with manifest schema 3, 4, or 5. Update the doc comment at the top of the file.
4. In `trailer.rs`, extend the accepted-manifest-schema pairing check (currently rejects anything
   beyond schema 4) to accept schema 5 under store schema 2, and update the framing doc comments
   that enumerate the accepted schemas.
5. In `runtime.rs`, replace both schema-4 hard keys so the R4 chain accepts schema 5 the moment the
   compiler emits it (no transient window where a schema-5 required artifact boots without its
   envelope): `requires_signature` must treat manifest schema 4 OR 5 with
   `signing.required == true` as required, and `verify_envelope_bytes` must accept a present
   envelope paired with manifest schema 4 OR 5 (schema ≤ 3 plus envelope stays the unpaired
   rejection). Update the neighboring doc comments that name schema 4.
6. In `manifest.rs`, extend `Manifest::to_canonical_json`'s manually-built `signing` JSON object to
   include `"freshness": <value>` when `Some` (the canonical writer builds the signing block by
   hand with a `json!` literal; the derived `Serialize` never runs on this path — without this
   step every schema-5 artifact would carry a freshness-less signing block and reject at its own
   strictness gate).
7. In `commands/compile.rs` at the single signing-block call site: when `--sign`, set the manifest
   schema to `MANIFEST_SCHEMA_V5` and `freshness: Some(<unix seconds>)` where the value is
   `SystemTime::now()` seconds since epoch as `u64`. Unsigned compiles keep emitting schema 3 with
   byte-identical output (no code path change for them). The `--manifest` output needs no printer
   change because it prints the stored canonical JSON bytes verbatim once step 6 embeds
   freshness.

**Tests:**
- `signed_compile_emits_schema5_freshness_marker` (integration, `compiled_artifact_test.rs`):
  compile+sign (reuse the suite's shared signing fixture setup) -> `--manifest` JSON reports
  `manifest_schema` 5 and the signing block carries a `freshness` value within 60 seconds of the
  test's `SystemTime::now()`.
- `schema5_artifact_passes_the_full_r4_chain` (integration): the existing r4sign signing tests
  (`signed_artifact_boots_with_valid_envelope`, `verify_flag_roundtrip_and_output`,
  `required_signature_missing_envelope_fails_closed`, and neighbors) pass unmodified against
  schema-5 fixtures — this is the gate proving steps 3-4 landed; no new test body needed beyond
  the suite run.
- `unsigned_compile_stays_byte_identical` (existing test in `compile_command_test.rs`, must pass unmodified).
- `schema5_without_freshness_rejected` (unit, `manifest.rs`): synthetic manifest JSON with schema 5
  and a signing block lacking `freshness` -> decode error.
- `schema4_with_freshness_rejected` (unit, `manifest.rs`): schema 4 signing block carrying
  `freshness` -> decode error.
- `readers_accept_manifest_schemas_2_through_5` (unit, `manifest.rs`): decode succeeds for
  schema-2, 3, 4 fixtures (existing ones) and the new schema-5 fixture; schema 6 rejects.

**Acceptance:**
- `cargo test -p camel-cli --lib manifest` passes.
- `cargo test -p camel-cli --test compiled_artifact_test` passes (all new + all pre-existing,
  including the unmodified r4sign tests against schema-5 output).
- `cargo test -p camel-cli --test compile_command_test` passes.
- `cargo clippy -p camel-cli --all-targets -- -D warnings` exits 0.

- [x] 2.1

### camel-cli trust policy

#### Task 2.2: Floor storage, lock-serialized boot critical section, schema-4 boundary

**Files:**
- `crates/camel-cli/src/compile/trust.rs` (modified — floor storage, decision fn, floor recording,
  lockfile)
- `crates/camel-cli/src/compile/runtime.rs` (modified — freshness wiring at both sites)
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified — new integration tests)
- `Cargo.toml` (modified — workspace dependency)
- `crates/camel-cli/Cargo.toml` (modified — dependency)

**Steps:**
1. Add the `fs4` crate for the truststore lockfile: edit the root `Cargo.toml`
   `[workspace.dependencies]` to add `fs4 = "1.1"` with a one-line comment
   (`# truststore lockfile critical section, keypin`), and use it from camel-cli as
   `fs4.workspace = true` (the ed25519-dalek hoist pattern; `cargo add fs4 --workspace` would add
   it to every member — do not use it).
2. In `trust.rs`, grow the Phase-1 surface with its first readers: add `floor: Option<u64>` to
   `PinEntry` and a `lines: Vec<String>` field to `TrustStore`, both populated by `parse` (the
   validated floor value is now stored; the verbatim lines are preserved for order- and
   comment-preserving rewrites), add `TrustStore::floor(&self, fingerprint: &str) ->
   Option<u64>`, and extend `TrustError` with `LockFailure { path }`, `Unwritable { path }`,
   `Io { path }` (Display step token `truststore-update` for these three).
3. Add `pub(crate) enum FreshnessVerdict { Accept, AcceptRecord, Rollback }` and
   `pub(crate) fn decide(marker: Option<u64>, floor: Option<u64>) -> FreshnessVerdict`:
   `(None, None) -> Accept`; `(None, Some(_)) -> Rollback` (legacy schema-4 below a recorded
   floor); `(Some(m), None) -> AcceptRecord`; `(Some(m), Some(f)) -> Rollback if m < f`,
   `Accept if m == f`, `AcceptRecord if m > f`.
4. Add `pub(crate) fn with_truststore_lock<T>(lock_path: &Path, deadline: Duration, f: impl
   FnOnce() -> Result<T, TrustError>) -> Result<T, TrustError>`: open-or-create
   `<truststore>.lock`, `try_lock_exclusive` in a bounded retry loop (50 ms interval) until
   `deadline`; on deadline or IO failure -> `TrustError::LockFailure { path: lock_path }`; run
   `f()`; unlock on drop.
5. Add `pub(crate) fn record_floor(store: &TrustStore, path: &Path, fingerprint: &str, marker:
   u64) -> Result<(), TrustError>`: rewrite the truststore from the verbatim `lines` of the
   `TrustStore` passed in (every comment, blank line, and pin preserved in order); on the line
   whose pin is `fingerprint`, set or replace the floor token to `max(existing_floor_if_any,
   marker)`; a `fingerprint` with no line in `store` returns `Err(TrustError::Unwritable { path })`
   as a defensive unreachable (the caller re-checks the pin under the lock first); write via tmp
   file in the same directory plus rename (envelope-write precedent), removing the tmp file on any
   write or rename failure before returning `TrustError::Unwritable { path }`. Callers MUST pass
   the snapshot parsed UNDER the lock, never a pre-lock parse.
6. Wire into `runtime.rs` (shared helper for both sites): after the Task 1.2 pin check passes —
   - `--verify` (dry run): `decide(signing.freshness, truststore.floor(fp))` on the floors read at
     parse time, no lock, no writes. `Rollback` -> exit 2, `freshness-rollback` diagnostic naming
     the fingerprint and the floor.
   - Boot: `with_truststore_lock(<ts>.lock, TRUSTSTORE_LOCK_DEADLINE, ..)`: re-parse the
     truststore INSIDE the lock, RE-CHECK `is_pinned` on the fresh snapshot (a pin removed while
     this boot waited for the lock must exit 2 `truststore-pin` — the floor logic never runs for a
     depinned key), then `decide` on the fresh floors. `Rollback` -> exit 2
     `freshness-rollback`. `AcceptRecord` -> `record_floor` on the under-lock snapshot (failure ->
     exit 2 `truststore-update`). `Accept` -> proceed, no write. Lock or write failure -> exit 2
     `truststore-update`.
   - The freshness step runs only when a truststore is supplied, the envelope verified, and the
     key is pinned; a schema-5 artifact with no truststore takes the plain R4 chain.
7. Keep the 10-second lock deadline as a named const `TRUSTSTORE_LOCK_DEADLINE` next to the
   helper.

**Tests:**
- `decide_covers_the_policy_table` (unit, `trust.rs`): assert all six rows of the Step-3 table,
  including `(Some(m), Some(m)) -> Accept`.
- `truststore_parse_stores_floors` (unit, `trust.rs`): pin with floor `42` -> `floor(fp)` is
  `Some(42)`; pin without floor -> `None`; unpinned fingerprint -> `None`.
- `rollback_below_floor_fails_closed` (integration; reuse the shared signed fixture, truststores
  hand-written): read the marker from the fixture's `--manifest` JSON; write the truststore as
  `<fingerprint> <marker+1000>`; boot -> exit 2 `freshness-rollback`; `--verify --truststore` ->
  exit 2 `freshness-rollback`.
- `first_sight_records_floor` (integration): pin without floor -> boot succeeds; the truststore
  line for the fingerprint now ends with the marker value read from `--manifest`.
- `floor_never_decreases` (integration): (a) floor == marker -> boot succeeds and the truststore
  bytes are unchanged; (b) floor = marker - 5 -> boot succeeds and the floor becomes the marker.
- `verify_is_a_dry_run` (integration): floor = marker - 5 -> `--verify` exits 0 and the
  truststore file is byte-identical before and after.
- `unwritable_truststore_fails_closed` (integration): pin without floor; make the truststore's
  parent directory read-only (`0o555`) -> boot exits 2 `truststore-update`; restore `0o755` in
  the test's cleanup path.
- `lock_deadline_fails_closed` (unit, `trust.rs`): hold the flock on `<ts>.lock` from the test
  via `fs4`, call `with_truststore_lock` with a 150 ms deadline -> `LockFailure` whose Display
  names the lock path.
- `schema4_boundary_is_the_first_recorded_floor` (unit, `runtime.rs`, against the shared
  trust-policy helper): build a synthetic `Manifest` whose signing block is schema 4 (no
  freshness) for the pinned fingerprint; (a) truststore without floor -> the policy helper
  returns proceed/boot; (b) truststore whose line carries any floor -> the helper returns the
  exit-2 `freshness-rollback` path. This exercises the boundary at the policy layer; CLI-level
  schema-4 fixtures are not constructible once the compiler emits schema 5, because the manifest
  lives inside the signed bytes and the trailer checksum.
- `pin_removal_under_lock_fails_closed` (unit, `runtime.rs`, against the boot wiring): hold the
  lock, rewrite the truststore without the pin, release, and let the boot proceed through the
  under-lock re-parse -> exit 2 `truststore-pin` (the re-check of Step 6 fires; freshness never
  runs).
- `freshness_without_truststore_skips_the_step` (integration): signed fixture, no truststore ->
  boots (plain R4 chain, matching the existing round-trip test).
- `concurrent_floor_writes_settle_on_the_max` (unit, `trust.rs`): truststore with one pin, no
  floor; two threads run the lock-guarded re-parse/re-check/decide/record sequence with markers
  100 and 200 through the same helper sequence the boot path uses; each thread reports the floor
  it observed at decision time; after both join: the recorded floor is 200, every thread that
  accepted observed a floor <= its own marker (no stale-accept), and a follow-up
  `decide(Some(100), Some(200))` is `Rollback`.

**Acceptance:**
- `cargo test -p camel-cli --lib trust` passes.
- `cargo test -p camel-cli --test compiled_artifact_test` passes.
- `cargo clippy -p camel-cli --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-unbounded-wait` passes (the retry loop is deadline-bounded).

- [x] 2.2

### docs

#### Task 2.3: ADR-0083 amendment and CONTEXT-MAP glossary entries

**Files:**
- `docs/adr/0083-artifact-signing-envelope.md` (modified — amendment section + status line)
- `CONTEXT-MAP.md` (modified — Key Terms additions)

**Steps:**
1. Load the `ste-writing` skill before editing (docs governance rule for `docs/` markdown).
2. Append the amendment section from `openspec/changes/keypin/design.md` `## ADR-0083 amendment
   (draft)` verbatim (the fenced markdown block), as a new `## Amendment: truststore pinning and
   rollback freshness (2026-09-26)` section before `## References`.
3. Update the ADR Status line to append exactly: `; Amended 2026-09-26: truststore pinning and
   rollback freshness (keypin change)` (the same string design.md specifies).
4. In `CONTEXT-MAP.md` Key Terms, add four entries following the existing entry format:
   **truststore** (deployment-owned pin file consulted at verify time), **pin** (a pinned
   `blake3:` verifying-key fingerprint line), **freshness marker** (signed u64 unix-seconds value
   in a schema-5 signing block), **floor** (per-key highest accepted marker recorded in the
   truststore). Cross-reference ADR-0083 in each entry per the two-source rule.
5. Do not touch any other ADR or spec file.

**Tests:**
- `docs_amendment_matches_design`: manual diff check — the appended section equals the design.md
  draft block (assert during review).
- `cargo xtask lint-context-citations` exits 0 (glossary entries cite ADR-0083).

**Acceptance:**
- ADR-0083 contains the amendment section and updated status line; no other ADR changed.
- `CONTEXT-MAP.md` Key Terms contains the four new entries.
- `cargo xtask lint-context-citations` exits 0.

- [x] 2.3
