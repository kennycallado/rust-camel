# Proposal: 451-cli-v1-artifact-drop

## Why

Row 10 of the pre-1.0 shedding plan (bd `rc-n6dof`, epic `rc-v6wiz`). The CLI still accepts
compiled artifacts in our own retired formats: the v1 single-document trailer (68-byte footer)
and the legacy store-schema-1 / manifest-schema-2 pairing inside v2 framing (ADR-0075). Nothing
external depends on them — the format is ours, no external users exist (ADR-0090), and today's
compiler emits only v2 framing with store schema 2 and manifest schemas 3/4/5. Keeping the read
paths maintains a downgrade/rollback surface: an old or tampered artifact boots instead of
failing loudly. ADR-0090 orders deletion with a tombstone: a removal must fail at load with an
error that names the removed version and the replacement, never a silent fallback.

## What Changes

- **Reader narrows to the current format.** `camel-cli`'s artifact decoder accepts only the
  current pair(s) today's compiler emits: trailer framing version 2, `store_schema: 2`, and
  `manifest_schema` 3 (unsigned), 4 (signed), or 5 (signed freshness). The v1 trailer decode,
  the one-entry-store adaptation, and the store-1/manifest-2 legacy pairing are deleted.
- **Tombstone on rejected versions.** A marked artifact carrying the v1 trailer or any
  unsupported framing/manifest/store version is rejected with a payload-free error that names
  the declared version and the remedy — recompile from source with this version (`camel
  compile`). It is never treated as a trailer-free executable and never falls back. Truly
  trailer-free executables keep the normal CLI fall-through.
- **Writer untouched.** `camel compile` still emits exactly the current v2 artifacts. No
  signing, CLI-flag, or asset-embedding change.
- **Spec and ADR aligned.** `openspec/specs/cli-compile` states the accepted versions
  explicitly and drops the "Single-document v1 remains readable" scenario. ADR-0075 gains a
  dated amendment recording the removal, the reason (ADR-0090, attack surface), and the
  migration (recompile); ADR-0083's compatibility paragraph gets a dated cross-reference for
  the same narrowing. CONTEXT-MAP and `camel-cli` CONTEXT terms that promise v1 readability
  are corrected in the same change.

## Capabilities

### Modified

- `cli-compile` — artifact compatibility promise: current accepted version set stated
  explicitly, v1 acceptance removed, tombstone behavior specified.

## Impact

- `crates/camel-cli/src/compile/{trailer.rs,manifest.rs,runtime.rs,mod.rs}` — decode path
  narrowing, tombstone diagnostics, dead legacy helpers removed.
- `crates/camel-dsl/src/embedded_store.rs` — store index decode accepts only `STORE_SCHEMA`;
  the schema-1 shape is deleted.
- Tests — v1-acceptance tests flip to tombstone tests; current-artifact suites must stay green
  unchanged.
- Docs — ADR-0075 amendment, CONTEXT-MAP.md, `crates/camel-cli/CONTEXT.md`.
