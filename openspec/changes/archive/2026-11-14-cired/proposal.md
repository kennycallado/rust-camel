# Proposal: cired

## Why

CI run `43560699207` on `89c52e82d` left `Full Tests` red. Eight of nine jobs are
green; all three failures trace to `49cb8b896` (`feat(dsl)!: source_file`) and
are repairable without product changes:

- `rc-lhd0y` — `dev_watch_script_test.rs` registers `rhai` unconditionally.
  Under `--workspace` feature unification a member enables
  `camel-core/lang-rhai`, so `CamelContextBuilder::built_in_languages()` already
  seeded `rhai` and the explicit registration panics with `AlreadyRegistered`.
  Under `-p camel-test` the feature is off, which is why the mission gate run
  saw green.
- `rc-850w7` — the two watched tests in `dev_watch_proto_test.rs` assert the
  committed descriptor generation immediately after the validation outcome
  succeeds. `ProtobufReloadTarget::commit` swaps the descriptor state and bumps
  the generation counter in two separate statements (verified in code); a
  concurrent poll can observe the swapped descriptor before the counter lands,
  and the post-hoc `generation()` read then fails the assert even though
  exactly one reload committed. The race did not reproduce locally; increased
  CI load from the larger workspace test graph is the inferred trigger.
- `rc-wigut` — the new fenced example in `docs/src/eip/script.md` uses
  `source_file: scripts/enrich.rhai`. The sweep's script loader
  (`ScriptLoadContext::new(None)`, `camel-dsl/src/yaml.rs`) joins relative
  references onto the test process current working directory — under
  `cargo test` that is the `camel-test` package directory — so the sweep looks
  for `crates/camel-test/scripts/enrich.rhai`, which does not exist, and fails
  closed on the unresolvable reference (verified empirically via the sweep
  ledger).

## What Changes

- Test-only guard in `crates/camel-test/tests/dev_watch_script_test.rs`: register
  `rhai` only when absent (`resolve_language` check). Any registration error
  other than the unification artifact still fails loudly.
- Test-only determinism fix in `crates/camel-test/tests/dev_watch_proto_test.rs`:
  after the validation outcome wait, wait on the generation advance under the
  existing bounded poll, then keep the exact generation assertions.
- New fixture `crates/camel-test/scripts/enrich.rhai` at the location the sweep
  actually resolves, so the documented example parses under the sweep without a
  baseline entry or sweep weakening. The documentation remains unchanged; this
  fixture supplies the sweep's CWD-based parser context (the documented
  contract is route-file-relative, but the sweep parses fences without a route
  file, so its loader falls back to the process CWD).
- Follow-up bd issues (filed from this mission, not part of the diff): a gate
  lane that runs `camel-test` integration suites under `--workspace`
  unification; a public parse-with-base-dir API for the DSL script loader
  (sweep-anchoring `source_file` to the route file directory needs a product
  change and is out of scope here); anything found in the watcher diagnosis
  that is a real product defect.

Out of scope: `rc-w0znn` (JMS flake), any change to
`crates/camel-core/src/hot_reload/**` product code, changes to the sweep's
key-set assertion or baseline policy, `AGENTS.md` edits.

## Acceptance criteria

- `cargo test -p camel-test --test dev_watch_script_test` passes (feature off —
  the registration branch runs) and the workspace run passes (feature unified —
  the already-registered branch runs).
- `cargo test --workspace --test dev_watch_script_test --test dev_watch_proto_test --test route_example_parser_sweep` passes (run twice).
- Mission gate table green: fmt, `cargo build --workspace`, clippy
  `-p camel-test --all-targets -- -D warnings`, `cd docs && mdbook test`,
  `bash scripts/gates-local.sh`.
- The documented example in `docs/src/eip/script.md` parses under the sweep:
  the `source_file` reference resolves from the sweep's test process working
  directory (`crates/camel-test/scripts/enrich.rhai` exists).

## Risk budget

Test-and-docs only; no product behavior change. Acceptable: adjusting the
fixture script content to keep the sweep's Mutating-capability requirement.
Out of bounds: weakening the sweep assertion, blanket skips, watcher redesign
(stop and report instead), weakening the script test's registration failure to
`.ok()`.

Bd: rc-lhd0y, rc-850w7, rc-wigut
