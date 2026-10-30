# Design: 451-cli-v1-artifact-drop

## References

- ADR-0075 (artifact format; amendment target), ADR-0090 (pre-1.0 compatibility policy —
  deletion with tombstone), ADR-0083 (signing envelope; schemas 4/5 stay valid).
- bd `rc-n6dof` (this row), `rc-v6wiz` (epic). Base main `8c28e9ca1`.

## Current state (verified)

- Writer (`commands/compile.rs:428`) emits only `encode_v2` artifacts: trailer version 2,
  `store_schema: 2` (`camel_dsl::embedded_store::STORE_SCHEMA`), `manifest_schema` 3/4/5.
- Reader v1 surface: `trailer.rs` `Trailer` + `encode`/`decode`/`checksum_domain` +
  `FOOTER_LEN`/`FORMAT_VERSION`/`CHECKSUM_DOMAIN` + `DecodedArtifact::V1` +
  `to_single_entry_store`; `manifest.rs` `MANIFEST_SCHEMA_LEGACY`/`MANIFEST_SCHEMA_V2` +
  `to_legacy_json` + `derive` + the v1 arm of `validate_manifest` + legacy default in
  `manifest_schema_of` + legacy branches in `from_canonical_json`; `enforce_schema_pairing`
  store-1 arm; `embedded_store.rs` schema-1 index shape (`RawIndexV1`, asset-entry guard).
- One integration test constructs a v1 artifact (`tests/compiled_artifact_test.rs:4484`).

## Decisions

1. **Decode returns `Option<TrailerV2>`.** `DecodedArtifact` collapses; `decode_artifact`
   becomes `Result<Option<TrailerV2>, TrailerError>`. The v1 codec, `Trailer`,
   `to_single_entry_store`, `checksum_domain` (v1), and the v1 constants are deleted
   (deletion, not hiding — ADR-0090).
2. **Tombstone reuses `TrailerError::InvalidVersion(u16)`.** Its `Display` becomes
   `unsupported compiled artifact version {v}: recompile from source with this version
   (camel compile)`. Payload-free: the error carries only the declared version number.
   `manifest::TrailerError::InvalidManifestSchema` and `camel_dsl`
   `StoreError::UnsupportedStoreSchema` displays gain the same remedy suffix.
3. **Dispatch (version evidence before any v1 parsing).** Terminal magic absent → `Ok(None)`
   (unchanged trailer-free fall-through). 76-byte window declares version 2 → v2 decode
   (unchanged). Window version ≠ 2: if the last 68 bytes open with the family magic (a
   genuine v1 footer), read the version field from THAT footer and return
   `InvalidVersion(declared)` — the tombstone names 1 for real v1 artifacts; otherwise
   `InvalidVersion(window)` as today. `probe_declared_span` needs no v1 section bytes for the
   tombstone: the v1-footer branch returns the footer-window span, keeping the probe-tail
   equivalence contract (tail decode == whole-image decode).
4. **Manifest narrows to schemas 3/4/5.** `manifest_schema_of` rejects a missing
   `manifest_schema` (the schema-less legacy form) instead of defaulting to 1; the missing
   field maps to an explicit payload-free diagnostic naming the missing
   `manifest_schema` field and the recompile remedy (`InvalidManifestFields` in the trailer
   decode path, `CompileError::InvalidDocument` in `from_canonical_json`) — never the generic
   "not valid manifest JSON" error. `validate_manifest` loses the v1 trailer-version arm and
   its `trailer_version` parameter (single v2 call site remains in `decode_v2_marked`);
   unknown/removed schemas fail with `InvalidManifestSchema` naming the schema;
   `from_canonical_json` (used by the manifest/store agreement check and `--manifest`)
   accepts only 3/4/5. `to_legacy_json` and `derive` are deleted with their tests.
   Precedence preserved: unknown manifest schema is named before the pairing check.
5. **Pairing narrows to store 2 ↔ manifest 3/4/5.** The store-1 arm of
   `enforce_schema_pairing` is removed; the store decoder now rejects schema 1 before pairing
   ever sees it. The fail-closed `other` arm stays as defense in depth. Diagnostic
   precedence follows the existing decode order (manifest validation, then store decode,
   then pairing, then agreement): the legacy store-1/manifest-2 pair therefore surfaces the
   manifest-schema-2 tombstone; the store-schema-1 tombstone is exercised with an otherwise
   current (3/4/5) manifest.
6. **`camel-dsl` store decode accepts only `STORE_SCHEMA`.** `RawIndexV1`, the schema-1 shape
   branch, and the schema-1 asset guard are deleted; schema 1 fails as
   `UnsupportedStoreSchema(1)`. This is a 0.x API narrowing of a published crate — allowed by
   ADR-0090 (crate API removals need only a 0.x minor bump).
7. **Runtime/self-detect adaptation is mechanical.** `runtime.rs` `artifact_manifest` takes
   `&TrailerV2`; doc comments drop the v1 dispatch sentence. `mod.rs` module docs describe the
   single current format. The writer, signing, r2embed, and CLI flags are untouched.
8. **Current-artifact behavior is byte-identical.** The v2 path (framing, checksum, store
   decode, pairing, agreement, boot) is not modified. `--manifest`/`--report` unchanged.

## Affected boundaries

- `camel-cli` `compile` module (reader) — the whole change.
- `camel-dsl` `embedded_store` — schema-1 removal (crate API narrowing).
- No component, service, language, or camel-core surface changes.

## Test strategy (hermetic: no sleeps, no free ports, no env mutation)

- Unit (trailer): constructed v1 footer → `InvalidVersion(1)` naming version + remedy;
  unknown version with and without v1 footer magic; trailer-free → `None`; probe-tail
  equivalence for v1 images and unsupported versions; v2 round trip unchanged.
- Unit (manifest): schema-1/2/absent rejected with remedy; 3/4/5 still parse; schema-2
  `check_schema2_fields` helper deleted with its test.
- Unit (store, camel-dsl + cli re-export): schema-1 index now rejected; schema-99 case keeps
  its error; canonical index bytes unchanged.
- Unit (regression guard): schema-3/4/5 manifests still parse and sign/verify unchanged —
  the delta narrows the same requirements the freshness-floor and strict-unsigned specs live
  in, so their existing suites must stay green untouched.
- Integration (`tests/compiled_artifact_test.rs`): constructed v1 artifact exits 2 with the
  tombstone message naming version 1 and the remedy, never booting or falling through; legacy
  store-1/manifest-2 pair surfaces the manifest-schema-2 tombstone (precedence: manifest
  validation precedes store decode); store-schema-1 tombstone exercised beside a current
  manifest; missing `manifest_schema` field names the field and the remedy; current artifact
  still compiles, `--manifest`s, and boots (existing suites, unchanged); malformed trailer
  and unknown-version artifacts fail closed.

## Docs

- ADR-0075: append dated amendment (2026-10-29) — v1 read path and store-1/manifest-2 pairing
  removed per ADR-0090 ("A removal carries a tombstone"; shedding-plan row 10, `rc-v6wiz`; `rc-n6dof`); reason: own-format downgrade/rollback attack
  surface; migration: recompile from source with the current version. Historical format
  description untouched.
- CONTEXT-MAP.md: ADR-0075 index entry, `Compiled artifact`, `EOF trailer`, `Self-detect`,
  `Operational manifest`, `Store index` terms.
- `crates/camel-cli/CONTEXT.md`: v1 codec mention, `EmbeddedRequest` adaptation sentence,
  schema-pairing sentence (accept 3/4/5).
- ADR-0083: dated one-line cross-reference amendment — the "Compatibility" paragraph's
  "Readers accept manifest schemas 2, 3, 4, and 5" statement is superseded by the ADR-0075
  2026-10-29 amendment (schemas 1 and 2 are tombstones); signing decisions otherwise
  unchanged. Historical signing text is not rewritten beyond this dated note.

## Risks

- A hidden v1 consumer outside `compile/` — mitigated: workspace-wide grep showed none; full
  `cargo test -p camel-cli` and clippy `-D warnings` gate each task.
- Published-crate API removal in `camel-dsl` — accepted pre-1.0 (ADR-0090); no in-workspace
  caller remains after the change.
