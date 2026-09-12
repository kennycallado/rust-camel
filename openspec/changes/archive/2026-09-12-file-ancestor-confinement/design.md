# Design: file-ancestor-confinement

## Approach

All producer-side, in `crates/components/camel-file/src/lib.rs` (plus tests and
`CONTEXT.md`). Pre-flight (e_gpt, 2026-09-12) confirmed Option 1 —
nearest-existing-ancestor resolve — over an openat-family descriptor walk
(that would force fd-relative temp creation, rename, done-file, and durable
fsync across all five strategy paths; a partial hybrid was rejected as giving
misleading guarantees).

`validate_path_is_within_base(base, target)` is hardened in three moves:

1. **Nearest-existing-ancestor resolve** (replaces the lexical-only fallback at
   lib.rs:1232-1247): walk up `target`'s ancestors to the nearest existing
   component, canonicalize it, and require the canonicalized ancestor to stay
   within the canonicalized base. This closes the deterministic escape even
   when `auto_create=false` (validate-only path).
2. **Full-chain symlink rejection below base**: derive the relative components
   of `target` against the ORIGINAL (non-canonicalized) configured base — do
   not require the lexical target to prefix the canonical base. Any component
   below base that is a symlink (`symlink_metadata`) is rejected. The base
   itself may be a symlink (canonicalized normally, per existing operator
   contract); even in-base symlinks are mutable retargeting points, so none
   below base are accepted on producer write paths.
3. **Post-create re-verify**: in `FileProducer::call`, after the `auto_create`
   `create_dir_all(parent)` step and BEFORE the strategy match, re-run
   containment validation on the now-existing parent chain. A symlink followed
   during directory creation yields an outside-base canonical parent and the
   write is refused before any open/rename. Fail/Append/Ignore/TryRename/
   Override and the done-file path all sit downstream of (or share) these
   validators, so one hardening covers every write path (all five `fileExist`
   strategies and the separate done-file write); the done-file validation call
   site (already shared) picks this up unchanged.

Residual, explicitly documented in CONTEXT.md: a concurrently planted symlink
between validation and mkdir/open can still race (post-create re-verify blocks
the WRITE but cannot undo directories created outside during the mkdir race);
fully closing it requires the openat rewrite. The note changes from
"intermediate components are not symlink-protected" to "deterministic
symlinked-ancestor escape closed; concurrent local planting remains the
documented residual".

Tests (`#[cfg(unix)]` for symlink cases; tempfile; existing test harness
shape): a `#[cfg(unix)]` matrix test per strategy — `base/link -> outside`,
fileName `link/new/file.txt` — assert producer error AND
`!outside/new/file.txt.exists()` AND no `outside/new` dir. Done-file tested
via a SAFE body fileName plus an independently symlinked `doneFileName`
(a malicious body path never reaches done-file validation). Ignore probe:
fileName resolving through the symlink to an EXISTING outside marker must
error (not silently early-return on `target_path.exists()`). Happy paths:
nested `a/b/c.txt` with no symlinks succeeds; symlinked BASE directory still
accepted.

## Affected crates

- `camel-component-file` (crates/components): `src/lib.rs` (validator +
  producer post-create re-verify), tests, `CONTEXT.md` residual note.

## Architecture boundaries

Components layer only; the producer's atomic-write contract surface
(ADR-0016 strictness) gains a rejection row (symlinked intermediate
components below base) — consistent with the existing strict-rejection posture
for leaves. Mirrors the confinement discipline of the ADR-0047 template
openat walk as a considered alternative; ADR-0006/0032 (trusted operator base
config) frame the LOW deployment risk. No Runtime/DSL/Services impact; no new
dependencies.

## Alternatives considered

- Openat-family descriptor-relative traversal (camel-template
  `path_util.rs` pattern): rejected — correct but invasive (rename, temp
  files, done-file, durable fsync all need dirfd plumbing) for a P3 hardening.
- Hybrid (openat for the mkdir step only): rejected per pre-flight — blocks
  mkdir traversal but leaves path-based opens/rename raceable, implying
  stronger guarantees than delivered.
- Rejecting only EXISTING symlink components (no full-chain scan): rejected —
  mutable retargeting points; deterministic case demands the full chain.

Single-phase change (one coherent slice; no milestone split needed).
