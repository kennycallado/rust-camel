# Design: slimblockers

## Approach

Four interlocking facts drive the design (all verified empirically in the
worktree with `cargo tree -e features,no-dev` and by reading the
`lint-gate-forwarding` xtask source BEFORE this spec was written):

1. **camel-bundles defaults do not reach workspace consumers.** The workspace
   table consumes camel-bundles with `default-features = false`; camel-cli
   activates camel-bundles features ONLY through its own feature forwards
   (`full` forwards grpc, wasm, http-static, llm, surrealdb, mqtt, mcp,
   security). Making the eight bridges default-only features would silently
   drop them — and `camel_bundles::boot()` would stop registering
   jms/sql/redis/opensearch/ws/cxf/xslt/xj components in DEFAULT camel-cli
   builds.
2. **cargo-tree rendering rules.** Feature lines render for dep-declaration
   activation (a dependency edge's `features = [...]`/default) and for
   self-expansion of declaration-seeded features; activations seeded by
   feature-forwarding render NOTHING. Therefore the only mechanism that keeps
   the bridges active in camel-cli default builds while leaving the golden
   deptree fixture byte-identical (no regen, no camel-cli edit) is
   camel-bundles-internal activation inside a feature camel-cli already
   forwards.
3. **lint-gate-forwarding forbids new camel-bundles feature keys.** The lint's
   Rule 2 requires every `boot-consumer = true` manifest (camel-cli) to
   forward EVERY camel-bundles feature key; Rule 1 requires any consumer
   feature shadowing a gate name to forward it. camel-cli was forbidden zone at design time,
   so the design adds ZERO new camel-bundles feature keys: the eight bridges
   are `optional = true` deps activated by `dep:` entries appended to the
   EXISTING `http-static` feature (already in camel-bundles `default` and
   already forwarded by camel-cli `full`). http-static is the only default-set
   member that maps to no component crate, making it the least-wrong
   transitional host. Per-bridge features (`jms`, `sql`, ...) ride the next
   camel-cli mission, which owns the consumer forwards and the lint-required
   mirroring; that mission also renames the cfg keys.
4. **Workspace inheritance forbids the template flip.** A member cannot set
   `default-features = false` on a dep whose workspace-table entry has
   defaults on. Flipping the table entry would hit camel-cli/camel-config
   (both excluded at design time; camel-config stays excluded on resume, and camel-cli's dependency wiring stays excluded — only the rename-only surface was granted). So camel-template optionalizes the ENGINE at the
   source (external `default-features = false` consumers drop the engine
   immediately), camel-bundles keeps consuming with defaults, and the
   in-workspace unwinding is deferred to the camel-cli mission that owns all
   consumer manifests.

**camel-template feature shape (cargo-semantics constrained).** `dep:`
entries in `default` enable the optional deps but set NO named feature
(`cfg(feature = "lang-minijinja")` would be false in default builds), and
both named features and implicit (bare dep-name) features in `default`
render extra lines into `cargo tree -e features` output — all three shapes
verified empirically. The only rendering-clean shape is
`default = ["dep:camel-language-minijinja", "dep:minijinja"]` with the
engine modules gated on `cfg(feature = "default")`: an all-or-nothing crate
(default = full component; `--no-default-features` = engine-free). No named
re-enable feature is defined this mission — activating one without defaults
would link the deps while compiling the modules out (a broken consumer
surface). The named `lang-minijinja` feature, its cfg-key rename, and the
golden-fixture regeneration they require ride the next camel-cli mission.

**BootHandle containment.** `jms_pool`/`cxf_pool` fields and their
`shutdown_with_deadline` drain steps are used ONLY inside camel-bundles;
external consumers touch `datasource_catalog()` and `shutdown()`. The pool
fields and drain steps become `#[cfg(feature = "http-static")]`; the
always-on surface keeps compiling for every consumer in every profile
camel-cli can express (http-static is on in default/full builds; slim builds
never see the gated fields and never name them).

**Known transitivity.** camel-xj depends on camel-xslt; enabling the bridge
set always brings both. Under the aggregate gate this is invisible (all eight
toggle together) and is recorded for the future per-bridge split.

**camel-template boundary (exact).** Gated on `cfg(feature = "default")`:
bundle, closure, component, endpoint, lifecycle, producer, reload,
template_set, and their lib.rs exports (`TemplateBundle`,
`TemplateBundleConfig`, `TemplateComponent`). Engine-free and ungated: the
public config and error modules (exported limits-config types +
`TemplateReloadError`) and the crate-private path_util and uri modules —
all verified zero engine references; `cargo check -p camel-template
--no-default-features` must pass with exactly that surface.

## Affected crates

- `camel-bundles`: manifest (eight bridges `optional = true`; `http-static`
  gains the eight `dep:` entries; `default` unchanged), `src/lib.rs` cfg-gated
  registrations (xslt/xj components + BridgeCleanup fields, ws bundle, jms/cxf
  pool registration, opensearch/redis/sql bundles), cfg-gated BootHandle
  fields and shutdown steps, feature-aware tests.
- `camel-template`: manifest (optional camel-language-minijinja + minijinja,
  `default = ["dep:camel-language-minijinja", "dep:minijinja"]` — no other
  feature keys), cfg-gated modules, engine-absent compile test.
- `camel-cli` (rename only, zone granted on resume after mission 108 landed):
  `slim-http = []` becomes `slim-benchmarks = []` with `slim-http =
  ["slim-benchmarks"]` as a one-release alias; the three slim-http literals
  in tests/feature_profiles.rs switch to the new name plus one alias
  resolution check, and the two slim-http mentions in
  crates/camel-cli/CONTEXT.md move to the canonical name with a one-line
  alias note. The only other camel-cli prose touched is the post-holistic
  truthfulness rewrite of the stale deferral paragraph in the same
  CONTEXT.md. No camel-cli dependency wiring changes this mission.

## Architecture boundaries

Components zone only (camel-bundles registration cascade, camel-template
engine). No Runtime/DSL/Services/API changes; no public type changes beyond
cfg-narrowing under non-default feature combinations. The runtime-boot
cascade contract (ADR-0069 §10) is preserved: default builds register the
identical component set; feature-off builds register the reduced set by
explicit opt-out.

## Phases

### Phase 1: camel-bundles bridge optionality (rc-9720m)

- **Goal:** eight bridges optional; default closure and boot behavior
  byte-identical; slim (no-default-features) drops them.
- **Dependencies:** none (validated mechanism).
- **Externally-visible types/interfaces:** none added; BootHandle pool fields
  cfg-gated (invisible to in-tree consumers).
- **Deliverable:** commits on feat/slimblockers; camel-bundles tests.
- **Exit-criteria:** feature_profiles ×3 green without regen;
  lint-gate-forwarding green; camel-bundles compile matrix green; cargo tree
  proves the eight bridges absent from camel-bundles --no-default-features
  closure (redis exception documented).

### Phase 2: camel-template minijinja optionality (rc-wcs3v)

- **Goal:** engine optional at the source; defaults byte-identical.
- **Dependencies:** Phase 1 (shared manifest discipline; no code coupling).
- **Externally-visible types/interfaces:** camel-template engine gating
  (cfg-narrowed exports under `--no-default-features`; no new feature keys).
- **Deliverable:** commits; camel-template --no-default-features compile +
  tests.
- **Exit-criteria:** golden still green without regen; camel-template
  default/no-default compile matrix green.

### Phase 3: evidence, measurement, docs, rename

- **Goal:** slim-http binary size before/after recorded with numbers;
  CONTEXT docs aligned; deferral ledger updated; slim-benchmarks rename
  applied with one-release alias.
- **Dependencies:** Phases 1-2 complete.
- **Deliverable:** evidence files + rename commits + park report inputs.
- **Exit-criteria:** size numbers reported (BEFORE rebuilt at rebased base
  37ec0cb6); camel-bundles/camel-template CONTEXT.md feature notes current;
  `--features slim-benchmarks` and the `slim-http` alias both resolve and
  pass the forbidden-prefix exclusions; bd comments drafted.

## Rejected alternatives

- **Per-bridge camel-bundles features now:** fails mandatory
  lint-gate-forwarding Rule 2 (boot-consumer camel-cli must forward every
  gate; camel-cli forbidden at design time, bridge-forward diet not granted
   on resume). Deferred, not abandoned.
- **Workspace-table feature carry:** renders `camel-bundles feature "..."`
  lines into every consumer's deptree — breaks the golden no-regen pin. Also
  un-droppable for slim.
- **camel-integration-test feature carrier:** drags a third crate into the
  transition and couples the scenario harness to bridge presence; rejected
  for zone hygiene (and lint Rule 1 shadow hazards).
- **Member-level `default-features = false` on camel-template from
  camel-bundles:** rejected by cargo (inheritance rule); the coordinated flip
  belongs to the mission that owns all consumer manifests.
- **Golden fixture regeneration:** explicitly forbidden by mission 115.
