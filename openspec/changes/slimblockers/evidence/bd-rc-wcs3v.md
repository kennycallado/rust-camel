# rc-wcs3v — camel-language-minijinja exclusion edge (mission 115 slimblockers)

## Implementation

`feat(template): optional minijinja engine gate` (c31f9954):
camel-language-minijinja + minijinja are optional dependencies behind
`default = ["dep:camel-language-minijinja", "dep:minijinja"]`. `dep:`
entries set no named feature, so engine modules (bundle, closure, component,
endpoint, lifecycle, producer, reload, template_set + the
TemplateBundle/TemplateComponent re-exports) gate on `cfg(feature =
"default")`: an all-or-nothing crate. Engine-free surface: config, error
(public), path_util, uri (crate-private). The hard edge is removed AT THE
SOURCE — external `default-features = false` consumers drop both engine
crates from their closure today (verified: closure grep 0, engine_free
integration test green).

## Verification

- `cargo check/test -p camel-template` default (all 68 tests, zero assertion
  edits) and `--no-default-features` (engine_free test) green; clippy/fmt
  both polarities clean.
- Golden deptree fixture green WITHOUT regeneration (dep:-in-default renders
  no cargo-tree feature lines — empirically validated at v0.48 and v0.49).
- Consumer matrix green (camel-bundles, camel-config, camel-integration-test,
  camel-cli default + slim).
- Cargo.lock zero drift.

## Remaining deferral (in-workspace exclusion)

The workspace table consumes camel-template with default features ON, and
cargo forbids a member-level `default-features = false` flip against that
entry; camel-config/camel-cli co-inherit it. The coordinated flip (table
entry + camel-bundles/camel-cli/camel-config edges + named lang-minijinja
re-enable feature + cfg-key rename off `cfg(feature = "default")`) rides the
camel-cli bridge-forward mission, which owns all consumer manifests and the
golden regeneration those changes require. camel-language-minijinja
therefore stays linked in every in-workspace camel-cli profile this change.

## Reviews

r_glm per-task T2.1 APPROVE (two informational minors, no change), r_glm
inter-phase APPROVE; holistic + e_glm stage-4 verdicts in the parked report.
