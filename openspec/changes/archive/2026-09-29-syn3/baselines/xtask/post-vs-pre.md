# syn3 Task 2.4 — xtask lint verdict parity proof (post-flip vs pre-flip)

## Header addendum: xtask compile-error inventory (task 2.1 supplement)

The task-2.1 inventory (`openspec/changes/syn3/baselines/compile-inventory.txt`)
could not reach xtask: cargo fail-fast starved it behind the camel-bean-macros
error via xtask -> camel-dsl -> camel-core -> camel-bean -> camel-bean-macros.
Task 2.2 unlocked the chain, so this task produced the xtask inventory itself.

Command: `cargo check -p xtask` (run on the syn 3 pin, after task 2.2 landed).

Result: exit 101, **2 errors**, both the same root cause — syn 3 changed
`ItemImpl::trait_` from `Option<(Option<Token![!]>, Path, Token![for])>`
(syn 2) to `Option<(Path, Token![for])>` (syn 3); the negative-impl marker
moved to the new `ItemImpl::modifiers.polarity` field.

```
error[E0308]: mismatched types
   --> scripts/xtask/src/lint_context_citations.rs:169:25
169 |             && let Some((bang, path, _for_token)) = &impl_block.trait_
    |                         ^^^^^^^^^^^^^^^^^^^^^^^^
    |                         expected a tuple with 2 elements, found one with 3 elements
    = note: expected tuple `(syn::Path, For)`; found tuple `(_, _, _)`

error[E0308]: mismatched types
   --> scripts/xtask/src/lint_metric_labels.rs:373:32
373 |                     .and_then(|(_, path, _)| path.segments.last())
    |                                ^^^^^^^^^^^^
    |                                expected a tuple with 2 elements, found one with 3 elements
    = note: expected tuple `(syn::Path, For)`; found tuple `(_, _, _)`
```

Fixes (compiler-driven, semantics-preserving, no pre-emptive rewrites):

- `lint_context_citations.rs:169` — destructure the 2-tuple; the negative-impl
  exclusion that was `bang.is_none()` is now `impl_block.modifiers.polarity.is_none()`
  (syn 3 moved the `!` marker there). Behavior identical: negative impls are
  still excluded from trait-type resolution.
- `lint_metric_labels.rs:373` — destructure the 2-tuple `(path, _)`; the
  original already ignored the bang, so no polarity check is needed.

Re-check after fixes: `cargo check -p xtask` exit 0.

## e2e gate

- `command -v openspec` -> `/nix/store/iykwmwdkffqqa5x9wlvq7g3vwblzf73l-openspec/bin/openspec` (non-empty)
- `openspec --version` -> `1.7.0`
- `cargo test -p xtask --test archive_e2e -- --ignored` -> exit 0, 4 passed
  (e2e_validate_modified_removed_conflict, e2e_rename_and_drop_archives,
  e2e_strict_refusal_preserved, e2e_idempotent_rerun)

## Per-target test-count contract (vs manifest.txt)

- `LIB: test result: ok. 32 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` — byte-identical to manifest
- `E2E_COMPILE: test result: ok. 0 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out` — byte-identical to manifest
- `FULL_TEST_EXIT: 0` (matches manifest record)

## Verdict table

Captured from the built binary (`cargo build -p xtask` then
`./target/debug/xtask <lint> > <lint>.post.txt 2>&1; echo EXIT=$? >> <lint>.post.txt`),
identical recipe to Task 1.3.

| lint | pre-sha256 | post-sha256 | delta bytes | verdict |
|------|-----------|------------|-------------|---------|
| lint-test-sleep | 45a6b7adf551da2d3544ce4abc8b4019107a6fa03f2cf5f109c54e23da8f927b | 45a6b7adf551da2d3544ce4abc8b4019107a6fa03f2cf5f109c54e23da8f927b | 0 | EMPTY-DIFF |
| lint-metric-labels | 147e7430364d46894d85e05525661c0563176d42735cf222ffac248fe97ba71a | 147e7430364d46894d85e05525661c0563176d42735cf222ffac248fe97ba71a | 0 | EMPTY-DIFF |
| lint-unbounded-wait | a96e4a3c11ba8972be1e04e1f14ee21483fa83104dfa6e448353f8045ec21c9c | a96e4a3c11ba8972be1e04e1f14ee21483fa83104dfa6e448353f8045ec21c9c | 0 | EMPTY-DIFF |
| lint-context-citations | c006186d32fb1b2fbc3735af8a6548af304089ef715b6bc3ba3cea1b07fd3c2e | c006186d32fb1b2fbc3735af8a6548af304089ef715b6bc3ba3cea1b07fd3c2e | 0 | EMPTY-DIFF |

All four verdicts EMPTY-DIFF. `diff <lint>.pre.txt <lint>.post.txt` empty for
each; sha256s match the Task 1.3 manifest records exactly.

## Ratchet stability

`git diff --exit-code $(git log --format=%H -1 -- openspec/changes/syn3/baselines/xtask/manifest.txt) -- scripts/xtask/ratchet-test-sleep.max scripts/xtask/ratchet-unbounded-wait.max scripts/xtask/ratchet-cancel-tokens.max`
-> exit 0 (ratchets untouched since the Task 1.3 baseline commit).

## Quality gates

- `cargo clippy -p xtask --all-targets -- -D warnings` -> exit 0
- `cargo fmt -p xtask` -> applied; only the two fixed files changed