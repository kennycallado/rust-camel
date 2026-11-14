# Design: cired

## Approach

Three independent, contained repairs — two test-only, one docs-fixture. No
product code changes.

### rc-lhd0y — script test registration guard

`dev_watch_script_test.rs:185-186` registers `rhai` unconditionally. Under
`--workspace` feature unification, `camel-core/lang-rhai` is on, and
`CamelContextBuilder::built_in_languages()` (`context_builder.rs:198`) has
already seeded `rhai` via `language_registry::from_config`, so
`register_language` returns `AlreadyRegistered` and the `.expect` panics.

Fix at the single registration site:

```rust
if ctx.resolve_language("rhai").is_none() {
    ctx.register_language("rhai", Box::new(RhaiLanguage::new()))
        .expect("rhai language registration");
}
```

`resolve_language` (`context.rs:639`) is the context's real absence check (the
mission order's `get_language` mapped to the actual API). Under `-p` the
registration still runs and still fails loudly on any real error; only the
unification artifact is skipped. Not weakened to `.ok()`. Both branches must
be exercised in acceptance: `cargo test -p camel-test --test
dev_watch_script_test` (feature off — registration branch) and the workspace
run (feature unified — already-registered branch).

### rc-850w7 — proto test generation determinism

`ProtobufReloadTarget::commit` (`crates/components/camel-validator/src/compiled.rs:171`) swaps
the descriptor state and bumps the generation counter in two sequential
statements. The watched tests poll the observable validation outcome, then
assert `generation(route_id) > gen_before` immediately and unsynchronized — a
concurrent poll can observe the swapped descriptor in the window before the
counter lands. The post-hoc assert then reads a stale counter. CI evidence:
validation-success assertion passes, generation assert fails. The race is
latent in the harness since the test was written (identical assert order at
`6d8c351da`); the larger workspace test graph from `49cb8b896` changed CI
timing enough to expose it. Local reproduction: 5 runs pinned to 2 CPUs stay
green — the fix must make the test deterministic, not rely on local luck.

Fix (test-only) in both watched tests: after `wait_for_validation`, wait for
the generation advance under the file's existing bounded poll:

```rust
assert!(
    wait_for(|| generation(route_id) > gen_before, WAIT_BOUND).await,
    "watcher must commit the reload generation for the edit"
);
```

then keep the exact assertion (`generation(route_id) == gen_before + 1`) in
`proto_edit_recompiles_validator_descriptor`. Update the module doc comment
(`//!` lines 13-14) to document the commit ordering: descriptor publication
and counter increment are two separate observations, not one atomic barrier.

Verified vs inferred: the two-step commit ordering is verified in code
(`compiled.rs:171-175`); the CI-load trigger is inferred — the race was not
reproduced locally (5 green runs pinned to 2 CPUs). Investigated and excluded:
the watcher's `DevTarget` classification/dispatch path is byte-equivalent for
proto events between `6d8c351da` and `49cb8b896`, and the script publication
hook runs only in the full-pass branch that proto edits never enter;
`dev_watch_template_test.rs` with the identical harness migration passed in
CI. The registry and validator target are untouched: this repair changes no
production contract, and a product change is out of this mission's scope.

- `rc-wigut` — ship the fixture where the sweep resolves it

The new fence in `docs/src/eip/script.md` references
`source_file: scripts/enrich.rhai`. The sweep parses fences via
`camel_dsl::parse_yaml_to_declarative`, which hardwires
`ScriptLoadContext::new(None)` (`camel-dsl/src/yaml.rs:110`); with no
base dir, `ScriptLoadContext::resolve` joins still-relative paths onto the
process current working directory (`yaml.rs` `resolve`). Under `cargo test`
the sweep binary runs with cwd = the `camel-test` package directory, so the
loader looks for `crates/camel-test/scripts/enrich.rhai`. Empirically
confirmed in this worktree's sweep ledger:
`cannot read script file .../crates/camel-test/scripts/enrich.rhai: No such
file or directory`. (The mission order's assumption — resolution relative to
the route file directory — is not how the sweep path works; anchoring to
`site.path.parent()` would need a public parse-with-base-dir API, a product
change filed as a bd follow-up instead.)

Fix: ship the fixture at the resolved location —
`crates/camel-test/scripts/enrich.rhai`, a small mutating Rhai script (the
sweep asserts every parsed script site is `Capability::Mutating`), mirroring
the rhai patterns the sweep itself tests (`headers[...] = ...`). No baseline
entry, no assertion weakening. File resolution and the Mutating capability
are verified by `route_example_parser_sweep`; `cd docs && mdbook test` runs
as the separate documentation gate.

## Affected crates

- camel-test: `tests/dev_watch_script_test.rs`, `tests/dev_watch_proto_test.rs` (test-only edits), `scripts/enrich.rhai` (new fixture)
- docs: none (documented example unchanged; `docs/src/eip/scripts/` is NOT created)

## Architecture boundaries

No Runtime / DSL / Components / Services / Languages / Functions product code
is touched. The tests keep using the public test-support watcher API
(`watch_and_reload_with_barriers`) and the public context API
(`resolve_language`, `register_language`). Boundary rules
(hexagonal_architecture_boundaries_test) are unaffected.

## Alternatives considered

- Script test: gh62-style `match` tolerating `AlreadyRegistered` — rejected;
  the register-only-when-absent guard states the intent and keeps other errors
  loud, per the mission order.
- Proto test: make `commit` atomic (store + counter in one step) — rejected
  for this mission: a product change to the reload path is explicitly out of
  scope; filed as a bd follow-up for review instead.
- Sweep: baseline TSV entry with written reason — rejected; the order prefers
  shipping the fixture so the documented example is real and parseable.

Bd: rc-lhd0y, rc-850w7, rc-wigut
