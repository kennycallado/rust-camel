# Design: modconfinement

## Approach

Create `crates/components/camel-file/src/path_guard.rs` and move the five
related private helpers into it: `validate_relative_filename`,
`validate_path_is_within_base`, `is_valid_temp_prefix`,
`open_options_no_follow`, and `path_contains_traversal`. Declare the module in
`lib.rs`, import the helpers there, and preserve function bodies and cfg
attributes byte-for-byte where practical. Keep unit tests in `lib.rs`; they
continue to exercise the same helpers through crate-private imports.

## Affected crates

- `camel-component-file`: extract internal path-confinement helpers and update
  stale `CONTEXT.md` source citations.

## Architecture boundaries

This is internal Components-layer organization only. It does not alter Runtime,
DSL, Services, Languages, or Functions boundaries, and it does not change the
file component's endpoint or producer contracts. The extraction preserves the
existing exchange-data confinement behavior described by the file component
context and ADR-0032 trust-boundary terminology.

## Alternatives considered

Leaving helpers in `lib.rs` does not address the size and cohesion issue.
Moving tests with the helpers would expand the refactor surface without
benefit, so tests remain colocated in the existing test module. No new shared
crate is justified for one component's private helpers.
