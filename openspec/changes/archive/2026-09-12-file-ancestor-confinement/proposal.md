# Proposal: file-ancestor-confinement

## Why

Security audit F1 (docs/SECURITY-report_12-09-26.md, oracle-verified; bd
rc-0ks57): `validate_path_is_within_base`
(crates/components/camel-file/src/lib.rs) falls back to a lexical-only check
when neither the target nor its parent exists. A symlinked intermediate
ancestor inside the base (`base/link -> /outside`, fileName
`link/new/file.txt`) therefore passes validation; `create_dir_all` and the
subsequent write follow the link and escape the base. `O_NOFOLLOW` guards only
the leaf component. This is the residual TOCTOU already documented in
camel-file/CONTEXT.md; deployment risk is LOW (base is trusted operator
config) but the deterministic escape must close.

## What Changes

- Nearest-existing-ancestor confinement in `validate_path_is_within_base`:
  resolve the nearest existing ancestor of the target, canonicalize it, and
  require containment within the canonicalized base — replacing the
  lexical-only fallback.
- Reject any path component BELOW the configured base directory that is a
  symlink (full-chain `symlink_metadata` scan). The base itself MAY be a
  symlink (operator-configured); only components derived below the original
  base are constrained.
- Post-create re-verification: after `auto_create`'s `create_dir_all`, the
  producer re-verifies containment of the created parent before the first
  open/rename, so a followed symlink during directory creation cannot yield an
  outside-base write.
- Producer regressions: `link/new/file.txt` with `base/link -> outside`
  rejected under four `fileExist` strategies (Fail, Append, Override,
  TryRename) AND the separate done-file write path; assert rejection AND that
  no directory/file appears outside the canonicalized base.
  The Ignore-strategy stat-follow probe is folded into the matrix (existing
  outside target → error, not silent early return). Happy paths (nested
  symlink-free fileName; symlinked base dir) stay green.
- camel-file/CONTEXT.md residual-TOCTOU note updated: deterministic
  symlinked-ancestor escape CLOSED; narrowed concurrent-planting residual
  documented.

Explicitly excluded: an openat-family descriptor-relative rewrite of the
producer (pre-flight rejected as invasive for a P3 hardening), any change to
lexical pre-checks, and any consumer-side (read path) behavior change.

## Acceptance criteria

- Symlinked-ancestor fileName rejected on the bd-named write paths — four
  `fileExist` strategies (Fail, Append, Override, TryRename), the Ignore
  stat-probe, and the done-file write — with an error naming the confinement
  violation; in-base intermediate symlinks are rejected the same way.
- In deterministic cases (symlink present at validation time): no file or
  directory is created outside the canonicalized base in any rejection case
  (asserted on the filesystem, not just the error). Concurrent planting
  between validation and mkdir remains the documented residual: post-create
  re-verify blocks the write but cannot undo directories created during such
  a race.
- Nested symlink-free fileName and symlinked-base-dir configurations still
  write successfully (no over-blocking).
- CONTEXT.md residual note reflects the closed deterministic case and the
  narrowed residual.

## Risk budget

Acceptable: strictness increase — components below base must be real
directories (symlinked intermediate dirs inside a producer base are now
rejected). Out of bounds: breaking symlink-free or symlinked-base deployments,
consumer/read-path changes, or new dependencies. Affected crate:
`camel-component-file` (crates/components). bd: rc-0ks57.
