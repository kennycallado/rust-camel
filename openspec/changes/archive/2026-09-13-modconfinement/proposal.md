# Proposal: modconfinement

## Why

Extract path-confinement helpers from `camel-file`'s large `src/lib.rs` into a
dedicated private module. This addresses bd `rc-4pm9j` and improves module
cohesion without changing behavior or public API.

## What Changes

Move the confinement and no-follow helper functions, update imports and
Context documentation, and keep existing tests and behavior unchanged. No new
dependencies, security policy changes, or public symbols are included.

## Acceptance criteria

- `camel-file` builds with helpers in a dedicated private module.
- Existing `camel-file` tests pass unchanged, proving behavior preservation.
- Public API and dependency graph remain unchanged.

## Risk budget

Acceptable risk is limited to mechanical import and module-visibility changes.
Any function-body change, new behavior, public API change, or dependency
change is out of bounds.
