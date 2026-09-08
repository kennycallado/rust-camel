# Tasks: validate-partner-self-declare

## camel-integration-test

### Task 1: self-declaration grammar in document.rs (bindings + cross-check relaxation)

**Files:**
- `crates/camel-integration-test/src/document.rs` (modified)
- `crates/camel-integration-test/src/doc_parse_test.rs` (modified)

**Steps:**
1. In `ScenarioAction::bindings()` (~line 182): change the
   `ScenarioTarget::Partner(_)` arm from `Vec::new()` to return
   `endpoint_bindings(endpoint)` when
   `endpoint.provisioning == Some(Provisioning::Harness)`, and
   `Vec::new()` for plain-string (no-provisioning) targets. This makes
   a validate `bindVar` reserve its env key via the sole caller, the
   reserved-env-key check (~line 783).
2. In the partner-target cross-check (~lines 793-820, inside the
   document build fn where the local `partners` binding from
   `partners_from_raw` is in scope, document.rs:764): track TWO sets —
   `harness_uris` (send/receive refs with `provisioning: Harness`, as
   today) and a new `self_declared` set. The collector loop gains a
   `ScenarioAction::Validate { target: ScenarioTarget::Partner(endpoint), .. }`
   arm that pushes `endpoint.endpoint.as_str()` into `self_declared`
   if and only if ALL of: (a) `endpoint.provisioning == Some(Provisioning::Harness)`,
   (b) the endpoint's scheme (the part before `://`) is `http`, and
   (c) the `partners` map contains a key equal to `endpoint.endpoint`.
   Post-loop check: for each collected partner target, the URI passes
   iff (the target was object-form AND its URI is in `self_declared`)
   OR (its URI is in `harness_uris`); a plain-string validate URI
   passes only via `harness_uris`. Any other partner URI returns
   `DocError::Validation` naming the URI. Track object-form-ness per
   collected target (e.g. collect `(index, endpoint, is_object_form)`).
3. Rewrite the failure message of that post-loop check to teach all
   three escapes, keeping the URI: name the URI, then state the
   accepted forms — a harness `http` ref declared by this scenario's
   `send`/`receive` actions, an object form with
   `provisioning: harness` plus a `partners:` entry naming the URI.
   (Exact wording free; must contain the literal fragments
   `partners:`, `provisioning`, `send`.)
4. Update the `ScenarioTarget::Partner` doc-comment (~lines 208-211,
   currently "The URI must equal a harness endpoint reference declared
   by the scenario's own `send`/`receive` actions") to state the
   relaxed rule: equal such a reference, OR self-declare via object
   form with `provisioning: harness` and a `partners:` entry naming
   the URI.

**Tests:** (executable spec — name, arrange, act, assert)
- `object_form_partner_target_parses_with_bind_var`: parse_case YAML
  with a validate whose partner target is the object map
  `{endpoint: http://upstream/tiles, provisioning: harness, bindVar: upstream}`
  → parses to `ScenarioAction::Validate` carrying
  `ScenarioTarget::Partner(EndpointRef)` with
  `provisioning == Some(Provisioning::Harness)` and
  `bind_var == Some("upstream")`. Expected: PASSES before this task
  (the parser already accepts object form on `http:` refs) — it locks
  the parse shape. Command:
  `cargo test -p camel-integration-test --lib object_form_partner_target_parses_with_bind_var`.
- `object_form_partner_self_declares_with_partners_entry`: build a doc
  whose scenario has a send to `direct:start` plus a validate with the
  object-form partner target from above, and whose `partners:` map has
  key `http://upstream/tiles` (one trivial `PartnerScript`); NO
  send/receive references the partner → call the document validation
  used by `parse_case`/`undeclared_partner_target_is_load_error`
  (doc_parse_test.rs:714) → result is `Ok`. Expected: FAILS before
  (cross-check rejects — no send/receive harness URI), passes after.
  Command:
  `cargo test -p camel-integration-test --lib object_form_partner_self_declares_with_partners_entry`.
- `object_form_partner_without_partners_entry_requires_send_receive`:
  same doc minus the `partners:` map (partners omitted) → validation
  errs with `DocError::Validation` whose message contains the URI AND
  the fragments `partners:` and `provisioning` and `send`. Expected:
  FAILS before (message lacks the teaching fragments), passes after.
  Command:
  `cargo test -p camel-integration-test --lib object_form_partner_without_partners_entry_requires_send_receive`.
- `validate_partner_bind_var_reserves_env_key`: doc carrying a
  document-level `env:` map with key `upstream` (any string value), a
  send to the object-form `{endpoint: http://upstream/tiles, provisioning: harness}`
  (no bindVar — this keeps cross-check (i) satisfied in the
  before-state), and a validate object-form partner target with
  `bindVar: upstream` → validation errs with the reserved-env-key
  error naming `upstream` (check (h) at document.rs:775-790 compares
  every action's `bindings()` against the env-map keys and precedes
  cross-check (i)). Expected: FAILS before (Partner bindings are
  empty, the doc parses Ok), passes after. Command:
  `cargo test -p camel-integration-test --lib validate_partner_bind_var_reserves_env_key`.
- Every validate action in every test arrange above carries the
  mandatory `expectation:` node (e.g. `count: 1`) — lift the exact
  YAML shape from the pattern at doc_parse_test.rs:714.
- Existing `undeclared_partner_target_is_load_error`
  (doc_parse_test.rs:714) must remain green unchanged (plain-string
  unmatched URI still exits via `DocError::Validation`). Command:
  `cargo test -p camel-integration-test --lib undeclared_partner_target_is_load_error`.

**Acceptance:**
- `cargo test -p camel-integration-test --lib` exits 0.
- `cargo clippy -p camel-integration-test -- -D warnings` exits 0.
- `cargo fmt --check -p camel-integration-test` exits 0.

- [x] 1

## camel-cli

### Task 2: driver wiring arm + end-to-end repro

**Files:**
- `crates/camel-cli/src/commands/test/scenario.rs` (modified)
- `crates/camel-cli/src/commands/test/scenario_tests.rs` (modified)
- `crates/camel-cli/src/commands/test/driver_tests.rs` (modified)

**Steps:**
1. In `wire_endpoint_refs()` (~line 128-147): extend the match with an
   arm `A::Validate { target: ScenarioTarget::Partner(ep), .. } if ep.provisioning == Some(Provisioning::Harness) => ep`
   (import `ScenarioTarget` from camel_integration_test as needed).
   The existing `seen` endpoint-string dedup already prevents
   double-wiring when a send/receive also names the URI. No other
   function changes: this routes the ref into `bind_partners`
   (scripted via the `partners:` entry), the `harness_provisioned`
   env fold (~line 403-419), and `fill_bind_vars`.
2. `scenario_tests.rs`: add a unit test following the pattern at
   scenario_tests.rs:196-205 (`wire_endpoint_refs(&doc)` then
   `bind_partners`).

**Tests:** (executable spec — name, arrange, act, assert)
- `wiring_includes_object_form_validate_partner` (scenario_tests.rs):
  doc with a send to `direct:start` and a validate object-form
  partner target (`provisioning: harness`, `bindVar: upstream`), plus
  a `partners:` map keying the URI → `wire_endpoint_refs(&doc)`
  returns exactly two refs — `direct:start` (every send/receive ref
  wires, no scheme filter, scenario.rs:134-145) and the partner URI —
  and `bind_partners` yields `harness_provisioned` containing
  `upstream -> http://<bound>`. Expected: FAILS before (wired holds
  only `direct:start`), passes after. Command:
  `cargo test -p camel-cli --lib wiring_includes_object_form_validate_partner`.
- `wiring_excludes_plain_string_validate_partner` (scenario_tests.rs):
  doc whose validate partner target is a plain string URI
  (`partner: http://upstream/tiles`, no object form) alongside a send
  to `direct:start` → `wire_endpoint_refs` returns only the
  `direct:start` ref (plain-string validate stays inert). Expected:
  passes before and after (shape lock). Command:
  `cargo test -p camel-cli --lib wiring_excludes_plain_string_validate_partner`.
- `validate_only_partner_proxies_varying_query` (driver_tests.rs, async,
  following the write-doc-then-run pattern of `all_pass_exits_zero` at
  driver_tests.rs:111): full driver run with (a) a route file whose
  route is `from: direct:start` `to: ${env:UPSTREAM}/tiles?bbox=1.2`
  (env-tier interpolation resolved at boot — harness-provisioned
  bindings win over document env, scenario.rs:416-419), (b) a scenario
  doc whose ONLY partner reference is the validate object-form target
  `{endpoint: http://upstream/tiles, provisioning: harness, bindVar: UPSTREAM}`
  with `expectation: {count: 1}` and a short `deadline`, plus a
  `partners:` entry scripting one 200 response for the URI, and a send
  to `direct:start`; NO receive action anywhere → run exits 0, action
  results show the validate passing. Expected: FAILS before (Task 1
  cross-check rejects the doc: exit 2 `doc-validation`), passes after
  Tasks 1+2. Command:
  `cargo test -p camel-cli --lib validate_only_partner_proxies_varying_query`.

**Acceptance:**
- `cargo test -p camel-cli` exits 0 (lib + integration targets,
  including lint-corpus and tier-filter fixtures).
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `cargo fmt --check -p camel-cli` exits 0.

- [x] 2

## Documentation

### Task 3: prose docs for the self-declaration rule

**Files:**
- `docs/src/testing/index.md` (modified)
- `docs/adr/0069-integration-tier-testing-contract.md` (modified)
- `crates/camel-integration-test/CONTEXT.md` (modified)

**Steps:**
1. `docs/src/testing/index.md`: in the scenario-tier section (~line
   278 area, partner validate grammar, which today teaches only
   count/filters): ADD the declaration rule sentence — a partner
   validate URI must match a send/receive harness ref OR self-declare
   via object form with
   `provisioning: harness` plus a `partners:` entry naming the URI;
   add a 3-6 line example block mirroring the driver repro (validate
   object-form target with bindVar, partners entry, no receive).
2. `docs/adr/0069-integration-tier-testing-contract.md`: in §5
   (partner-side normative proof) and §9 ("Partner provisioning
   sources", ~line 258), add one
   sentence each noting the validate self-declaration channel and why
   it exists (proxy routes with per-request-varying queries have no
   literal arrival lane; the sacrificial-receive workaround is
   forbidden by design).
3. `crates/camel-integration-test/CONTEXT.md`: update the
   `ScenarioTarget::Partner` entry (mirrors the code doc-comment from
   Task 1 step 4) to the relaxed rule.

**Tests:** (executable spec — name, arrange, act, assert)
- Docs are prose; verification is mechanical. `name`: rule present in
  all three docs. `setup`: Tasks 1-2 merged. `action`:
  `rg -n "self-declare|provisioning: harness" docs/src/testing/index.md docs/adr/0069-integration-tier-testing-contract.md crates/camel-integration-test/CONTEXT.md`.
  `assert`: at least one hit per file; the guide hit shows the
  example block. Command: `cargo xtask lint-context-citations` exits 0.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- `rg -c "provisioning: harness"` returns ≥1 for each of the three
  files above.
- No other doc file touched.

- [x] 3
