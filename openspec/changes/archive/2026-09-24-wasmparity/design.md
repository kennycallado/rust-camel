# Design: wasmparity

## Approach

Two independent fixes, both confined to the camel-test harness. `camel run`
code paths are untouched.

### Fix 1 — scenario boot root normalization (rc-l3zrr)

Reproduced on main @ fec12d64 (evidence for reviewers):

- `cd <project-root> && camel test wdoc.test.yaml` (bare filename) fails:
  `full-boot-failure: scenario boot failed: ... Endpoint creation failed:
  failed to resolve base directory: ` (empty path), exit 2.
- `camel test /abs/path/wdoc.test.yaml` on the same project: 4/4 actions
  pass; the wasm guest compiles and executes.
- `camel run` is immune: `try_canonical_project_root` (run.rs:76-101) maps
  an empty config parent to `.` before use.

Mechanism: `Path::parent()` of a bare filename is `Some("")`;
`find_camel_toml_root("")` (runner.rs:142) walks `"".ancestors()` = `[""]`,
and `"".join("Camel.toml")` resolves against the process CWD, so the empty
path is accepted as the boot root. The empty root works for every join-based
consumer (relative joins fall back to CWD) but reaches
`camel_bundles::boot` → `WasmBundle::new(_, PathBuf::from(""))` →
`WasmComponent.base_dir = ""`, and `"".canonicalize()` fails
(camel-component-wasm/src/lib.rs:105-110).

Fix seam: `boot_scenario` (camel-integration-test/src/boot_scenario.rs),
at the top of the function, before `root.join("Camel.toml")`:

```rust
let root: &Path = if root.as_os_str().is_empty() { Path::new(".") } else { root };
```

Rationale for this placement: the root is born empty in the CLI
(`find_camel_toml_root`), but every consumer downstream of `boot_scenario`
(config load, `routeFilesFromRoot`, wasm base dir) shares this entry point.
Normalizing here fixes CLI and library callers in one seam, keeps
`find_camel_toml_root`'s strict-walk contract and its unit-tier consumers
byte-identical, and mirrors the `camel run` empty-parent→`.` rule without
duplicating canonicalization (the wasm component canonicalizes the base dir
itself). No full canonicalization: that would rewrite displayed
`routeFilesFromRoot` paths in error messages for no behavioral gain.

### Fix 2 — tier-label honesty + actionable lean miss (rc-4sb1x)

Constraints: lean registry PINNED by ADR-0064 ({direct, log, mock, seda,
timer}); mock-testkit spec mandates unit documents boot the lean
registration. So the fix is advertisement honesty, not registry growth.

1. **Label** (test.rs:307-313, 480, 535): `tier_label` becomes
   context-aware. Scenario documents annotate `[full]` (unchanged); unit
   documents deriving FULL annotate `[full*]` — derived full, executed on
   the lean boot; lean documents annotate `[lean]` (unchanged). The label
   flows to the collision message and the JUnit `tier` property unchanged
   (both consume the same string), so JUnit reports `full*` for these
   documents too.
2. **Advisory** (test.rs, precedent: `R-REPOSITORY-STUB`, line 527): when a
   unit document derives FULL, `camel test` writes one stderr line before
   execution: `R-UNIT-FULL: derived full, executed on the lean registry
   (direct, log, mock, seda, timer — ADR-0064); components outside the
   lean set need a scenario document`.
3. **Actionable failure** (test.rs unit branch, after
   `run_test_doc_with_defs` returns a `doc_error` for a FULL-derived
   document): append ` — unit documents execute on the lean registry
   (direct, log, mock, seda, timer); use a scenario document for wasm and
   other full-boot components`. Condition is `(derived tier == Full &&
   doc_error present)` — no error-message sniffing and no CamelError
   variant-chain walking (the failure surfaces as a nested
   `RouteError(InvalidState(ComponentNotFound))` Display string at
   runner.rs:610; classification by variant would have to unwind three
   wrapper layers for one hint). A FULL-derived unit document failing for
   any boot-class reason gets the same correct guidance.

## Affected Crates / Boundaries

- `crates/camel-integration-test/src/boot_scenario.rs` — root
  normalization (scenario boot seam; ADR-0069 sections 4, 10).
- `crates/camel-integration-test/Cargo.toml` — new `wasm` feature
  forwarding `camel-bundles/wasm` (lint-gate-forwarding Rule 1; without
  it the wasm bundle never compiles into the scenario-boot test build).
- `crates/camel-cli/src/commands/test.rs` — label shape, advisory,
  failure hint (mock-testkit reporting surface).
- Tests: `crates/camel-integration-test/tests/wasm_boot_test.rs` (boot
  test; the empty-root case holds the defect's CWD premise under
  `RUN_LOCK` with a chdir drop guard),
  `crates/camel-cli/src/commands/test/driver_tests.rs` (label + hint).
- NOT touched: `camel run` (run.rs), `find_camel_toml_root`, lean runner
  registry, wasm component internals, test-summary path (mission 244),
  compile/embed seams (mission 243).

## Design Decisions

- **`.`-mapping over full canonicalization** in `boot_scenario`: minimal
  parity — `WasmComponent` canonicalizes the base dir itself; canonical
  roots from existing callers are unaffected.
- **`full*` over a separate degraded field**: one string, backward-shaped
  (`full` prefix preserved), self-explaining once the advisory is seen;
  the order names this shape as acceptable.
- **Hint at doc level, not variant level**: project convention is
  classification by variant, but the miss is buried under two opaque
  wrappers; the tier derivation already supplies the classification for
  free.

## Test Plan

1. boot_scenario-level (camel-integration-test, harness modeled on
   `tests/common/mod.rs::run_logs_document`): scenario document with a
   `direct:start → wasm:<guest> → log:<name>` route, `echo.wasm` guest
   fixture copied to `tests/fixtures/wasm/`, `logs.contains` assertion on
   the body marker,
   `boot_scenario(&doc, Path::new(""), &env)` — boots, base dir resolves
   non-empty, the send action executes the wasm step (the asserted log
   line exists only if the exchange traversed it). Absolute-root twin
   asserts unchanged behavior.
2. camel-cli driver test: unit document with a `wasm:` step — output line
   carries `[full*]`, stderr carries `R-UNIT-FULL`, failure text names the
   lean registry and the scenario alternative; never a bare
   `Component not found: wasm`.
3. Regression: absolute-path scenario invocation and existing
   camel-component-wasm battery unchanged (gate:
   `cargo test -p camel-component-wasm`).

## Risks

- Annotation consumers parsing `[full]` exactly will see `[full*]` for
  unit-derived-full documents — accepted; the old signal was false.
- `boot_scenario` signature unchanged; normalization is behavior-additive
  only for the empty-root case, which previously could not boot wasm at
  all.
