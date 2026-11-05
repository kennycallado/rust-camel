# Tasks: http-content-type-provenance

Single-phase change (no `## Phases` in design.md). Crate groups are implementation-order only.
Global conventions for every task: marker property `PROPERTY_HTTP_CONTENT_TYPE_INHERITED`
(`"CamelHttpContentTypeInherited"`), key match is ASCII-case-insensitive (`eq_ignore_ascii_case("content-type")`),
clearing is `exchange.remove_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED)` (idempotent).
TDD discipline per task: each behavioral task names the POSITIVE regression test(s) that must
fail before its implementation lands (write those first, verify the command shows them failing,
then implement until they pass); negative-control tests may already pass and are not required to
be red; new API items may fail compilation before they exist, which counts as red for the tasks
that introduce them. Task 6.3 is green-only (docs/hygiene, no new tests). Task 6.2 verifies by
the behavior-specific mutation spot-checks in its step 4. Do not invent additional test design;
if a needed test is not specified here, STOP and report `test-design-gap: <what>`.

## camel-api

### Task 1.1: Marker constant, remove_property, and original-content-type snapshot extension

**Files:**
- `crates/camel-api/src/exchange.rs` (modified)
- `crates/camel-api/src/lib.rs` (modified)

**Steps:**
1. Add `pub const PROPERTY_HTTP_CONTENT_TYPE_INHERITED: &str = "CamelHttpContentTypeInherited";` next to `ORIGINAL_MESSAGE_EXTENSION`, with a doc comment stating it is set by the HTTP consumer at ingress and cleared by any route step that writes/removes `Content-Type`.
2. Add `pub fn remove_property(&mut self, key: &str)` on `Exchange` that does `self.properties.remove(key);` (doc: idempotent clear used by provenance writers).
3. Add `pub const ORIGINAL_CONTENT_TYPE_INHERITED_EXTENSION: &str = "CamelOriginalContentTypeInherited";` (doc: snapshot of marker presence taken with `CamelOriginalMessage` at route entry when `use_original_message` is enabled; capture replaces unconditionally) and the snapshot newtype `pub struct OriginalContentTypeInherited(pub bool);`.
4. In `crates/camel-api/src/lib.rs`, extend the existing `pub use exchange::{ ... }` re-export list (around lines 96-98) with `PROPERTY_HTTP_CONTENT_TYPE_INHERITED`, `ORIGINAL_CONTENT_TYPE_INHERITED_EXTENSION`, and `OriginalContentTypeInherited`, so downstream crate-root imports (`use camel_api::PROPERTY_HTTP_CONTENT_TYPE_INHERITED;`) compile.
5. Add unit tests in the existing `mod tests` of `exchange.rs`.

**Tests:** (command: `cargo test -p camel-api --lib`)
- `remove_property_removes_and_is_idempotent`: Exchange with property set → `remove_property` twice → `property(key)` is `None` after both calls.
- `remove_property_absent_is_noop`: Exchange without the property → `remove_property` → no panic, `property(key)` still `None`.
- `original_content_type_snapshot_roundtrips`: set_extension with `Arc::new(OriginalContentTypeInherited(true))` → `get_extension::<OriginalContentTypeInherited>` returns `Some` with `true`; absent key returns `None`.

**Acceptance:**
- `cargo test -p camel-api --lib` exits 0.
- `cargo clippy -p camel-api -- -D warnings` exits 0.
- `cargo fmt --check -p camel-api` exits 0.

- [x] 1.1

## camel-processor

### Task 2.1: Marker clearing in the five header processors (sites 1-5)

**Files:**
- `crates/camel-processor/src/set_header.rs` (modified)
- `crates/camel-processor/src/dynamic_set_header.rs` (modified)
- `crates/camel-processor/src/remove_header.rs` (modified)

**Steps:**
1. In `SetHeader::call` (set_header.rs:68) after `exchange.input.set_header(...)`: when `self.key.eq_ignore_ascii_case("content-type")`, call `exchange.remove_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED)`.
2. In `SetHeaderIfAbsent::call` (set_header.rs:112): same check inside the branch where the write actually happens (no write -> no clear).
3. In `DynamicSetHeader::call` (dynamic_set_header.rs:80): after the write, same case-insensitive key check clears the marker.
4. In `DynamicSetHeaderIfAbsent::call` (dynamic_set_header.rs:136): same, only when the write happens.
5. In `RemoveHeader::call` (remove_header.rs:60): after `exchange.input.remove_header(&self.key)`, when `self.key.eq_ignore_ascii_case("content-type")`, clear the marker.
6. Import `PROPERTY_HTTP_CONTENT_TYPE_INHERITED` from `camel_api` in all three files.
7. Add the tests below to each file's existing `mod tests`, using the file's existing `Service::call` test pattern (e.g. `SetHeader::new(IdentityProcessor, key, value)` + `futures::executor::block_on(svc.call(ex))` or the file's async test helper).

**Tests:** (command: `cargo test -p camel-processor --lib`)
- POSITIVE regressions (must fail before implementation):
- `set_header_clears_marker_on_content_type`: exchange with marker property set + message header absent → `SetHeader::new(IdentityProcessor, "Content-Type", "application/json")` call → marker `property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED)` is `None`, header present.
- `dynamic_set_header_clears_marker_on_content_type`: marker set → `DynamicSetHeader::new(IdentityProcessor, "Content-Type", evaluating_fn)` (use the file's existing test expression helper) → marker `None`.
- `remove_header_clears_marker_on_content_type`: marker set + header `"Content-Type"` present → `RemoveHeader` → marker `None`, header gone.
- NEGATIVE controls (may already pass):
- `set_header_keeps_marker_on_other_header`: exchange with marker set → `SetHeader` with key `"X-Flag"` → marker still `Some`.
- `set_header_if_absent_clears_marker_only_when_written`: (a) header absent → `SetHeaderIfAbsent` `"Content-Type"` clears marker (this half is positive); (b) header already present → same call leaves marker `Some` (negative half).
- `dynamic_set_header_if_absent_clears_marker_only_when_written`: same pattern as set_header_if_absent, both branches.
- `remove_header_keeps_marker_on_other_header`: marker set → `RemoveHeader("X-Flag")` → marker `Some`.

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0 (all pre-existing tests stay green).
- `cargo clippy -p camel-processor -- -D warnings` exits 0.

- [x] 2.1

### Task 2.2: ScriptMutator entry-list detector (site 6)

**Files:**
- `crates/camel-processor/src/script_mutator.rs` (modified)

**Steps:**
1. In `ScriptMutator`'s `Service::call`, before evaluating the expression, snapshot the `Content-Type` entry set: collect `(String, Value)` pairs from `exchange.input.headers` where `key.eq_ignore_ascii_case("content-type")`, sorted by exact key (deterministic order; `Value` is `Clone` + `PartialEq`).
2. After a successful `evaluate` (only on the `Ok` path), collect the same list and compare with `==`. Any difference — entry added, removed, value changed, or exact key spelling changed — calls `exchange.remove_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED)`.
3. Do NOT compare `header_ic` resolved values only: the exact key spelling must be part of the comparison so a case rename (`Content-Type` -> `content-type`) is detected.
4. Add the tests below to the existing `mod tests`, reusing the file's existing `MutatingExpression` test doubles (extend `TestMutatingExpression`-style doubles with new ones that mutate `Content-Type`).

**Tests:** (command: `cargo test -p camel-processor --lib`)
- POSITIVE regressions (must fail before implementation):
- `script_mutator_clears_marker_on_ct_update`: marker set, header `Content-Type: application/json` → expression sets `Content-Type: text/html` → marker `None`, header updated.
- `script_mutator_clears_marker_on_case_rename`: marker set, header `Content-Type: application/json` → expression removes it and sets `content-type: application/json` → marker `None`.
- `script_mutator_clears_marker_on_ct_removal`: marker set, header `Content-Type: application/json` present → expression removes the header (removal of a PRESENT key) → marker `None`.
- NEGATIVE controls (may already pass — the changed-only commit semantics preserve today's behaviour):
- `script_mutator_keeps_marker_on_same_value_assignment`: marker set, header `Content-Type: application/json` → expression assigns the same value → marker `Some` (unchanged-only commit means nothing landed).
- `script_mutator_keeps_marker_on_body_only_script`: marker set → expression assigns only the body → marker `Some`.
- `script_mutator_keeps_marker_on_eval_error`: marker set → expression returns `Err` (reuse `ParseErrorMutatingExpression`-style double) → marker `Some`, headers untouched.

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.

- [x] 2.2

### Task 2.3: Claim-check header restore clearing (site 11)

**Files:**
- `crates/camel-processor/src/claim_check.rs` (modified)

**Steps:**
1. In the headers-restore match at claim_check.rs:140-172, clear the marker when the applied action lands or drops a `Content-Type` key:
   - `HeadersAction::All(FilterAction::Include)`: clear when any stashed key `eq_ignore_ascii_case("content-type")` (it will be inserted).
   - `HeadersAction::All(FilterAction::Remove)` (`current.headers.clear()`): always clear.
   - `HeadersAction::All(FilterAction::Exclude)`: no header lands and none is dropped -> never clears on its own; the marker passes through unchanged.
   - `ByPattern { include, .. }`: the include arm inserts stashed keys matching an include pattern — clear when `include` is non-empty and any stashed `content-type`-keyed header matches some include pattern.
   - `ByPattern { exclude, .. }`: the exclude arm inserts stashed keys matching NO exclude pattern (there is no retain in this arm — insertion, claim_check.rs:158-165) — clear when `exclude` is non-empty and any stashed `content-type`-keyed header matches no exclude pattern.
   - `ByPattern { remove, .. }`: the remove arm retains current keys not matching any remove pattern (claim_check.rs:166-170) and `HeaderPattern::matches` is CASE-SENSITIVE (claim_check.rs:284-291) — clear iff some CURRENT header key that ASCII-ci-matches "content-type" is matched by some remove pattern (that exact key will actually be dropped). A canonical-literal shortcut would diverge in both directions: `Exact("Content-Type")` against a current lowercase `content-type` survives the restore, so clearing there would let the untouched inbound value be emitted verbatim.
2. Implement the predicate as a small helper in the same file (e.g. `fn action_touches_content_type(include: &[..], exclude: &[..], remove: &[..], stashed_headers: &HashMap<String, Value>, current_headers: &HashMap<String, Value>) -> bool`) so the branches stay readable; keep evaluation side-effect-free (inspect patterns/keys, no mutation). The remove arm needs the CURRENT headers (not only the stash) because that is what it retains.
3. Add tests to the file's existing test module, following its existing stash/restore test setup.

**Tests:** (command: `cargo test -p camel-processor --lib`)
- POSITIVE regressions (must fail before implementation):
- `claim_check_clears_marker_when_ct_included`: stash headers containing `Content-Type`, marker set on current → restore with `All(Include)` → marker `None`, header restored.
- `claim_check_clears_marker_on_all_remove`: marker set → `All(Remove)` (headers cleared) → marker `None`.
- `claim_check_clears_marker_on_bypattern_include_matching_ct`: marker set → `ByPattern { include: [pattern matching "Content-Type"], exclude: [], remove: [] }` → marker `None`, header inserted.
- `claim_check_clears_marker_on_bypattern_exclude_missing_ct`: marker set, stash contains `Content-Type` → `ByPattern { include: [], exclude: [pattern matching only "X-Other"], remove: [] }` → the ct key matches no exclude pattern, is inserted → marker `None`.
- `claim_check_clears_marker_on_pattern_removing_ct`: marker set, current headers contain `Content-Type` → `ByPattern { include: [], exclude: [], remove: [Exact pattern "Content-Type"] }` → marker `None`.
- NEGATIVE controls (may already pass):
- `claim_check_keeps_marker_when_remove_pattern_misses_case_variant`: marker set, current headers contain lowercase `content-type` only → remove pattern `Exact("Content-Type")` (case-sensitive, no match, key survives) → marker `Some`.
- `claim_check_keeps_marker_when_headers_untouched`: marker set → restore with `All(Exclude)` (stash without ct keys) → marker `Some`.

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.

- [x] 2.3

### Task 2.4: Fragment stripping clears marker (site 12)

**Files:**
- `crates/camel-processor/src/zip_splitter.rs` (modified)
- `crates/camel-processor/src/tar_splitter.rs` (modified)
- `crates/camel-processor/src/stream_codec.rs` (modified)

**Steps:**
1. In the zip fragment construction: after `ex.input.headers.remove("Content-Type")` AND after the `ex.properties = parent.properties.clone()` that follows it (zip_splitter.rs:333 — the clone happens after the removes and would otherwise re-arm the fragment's marker), call `ex.remove_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED)`.
2. Same in the tar fragment construction: clear AFTER the parent-properties clone (tar_splitter.rs:532), same reasoning.
3. Same in `fragment_stream_exchange` (stream_codec.rs:126-130) after the two `remove` calls (the clone there happens inside `fragment_exchange` before the removes, so ordering is already correct).
4. Parent exchanges are NOT modified; only the fragment exchange loses the marker.
5. Add tests to each file's existing test module (zip/tar have fragment-building tests to copy the setup from; stream_codec has `fragment_stream_exchange` callers in `stream_codec/chunks.rs` tests to copy).

**Tests:** (command: `cargo test -p camel-processor --lib`)
- POSITIVE regressions (must fail before implementation):
- `zip_fragment_clears_marker`: parent exchange with marker set + header `Content-Type` → build one zip entry fragment via the existing splitter test setup → fragment marker `None`; parent marker still `Some`.
- `tar_fragment_clears_marker`: same pattern with the tar splitter setup → fragment marker `None`.
- `stream_fragment_clears_marker`: parent with marker set → `fragment_stream_exchange(&parent, Body::Bytes(...))` → returned fragment marker `None`, parent untouched.

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.

- [x] 2.4

### Task 2.5: Original-message restoration restores marker (site 13, restore half)

**Files:**
- `crates/camel-processor/src/error_handler.rs` (modified)

**Steps:**
1. In `restore_original_message_if_enabled` (error_handler.rs:139-146), after `exchange.input = orig.clone()`: read `exchange.get_extension::<OriginalContentTypeInherited>(ORIGINAL_CONTENT_TYPE_INHERITED_EXTENSION)`. Semantics WHENEVER restoration runs (i.e. `use_original_message` is enabled AND the original-message extension is present): snapshot `Some(OriginalContentTypeInherited(true))` → `exchange.set_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED, Value::Bool(true))`; snapshot `Some(OriginalContentTypeInherited(false))` → clear the live marker (a stored false snapshot removes a live marker left by later route writes); snapshot ABSENT (message stashed without provenance capture) → clear the live marker as well. Capture always replaces, so any captured state is authoritative over the live state.
2. When restoration does NOT run (flag disabled, or no original-message extension on the exchange), do nothing — the marker state is whatever the route left.
3. Import `OriginalContentTypeInherited`, `ORIGINAL_CONTENT_TYPE_INHERITED_EXTENSION`, `PROPERTY_HTTP_CONTENT_TYPE_INHERITED` from `camel_api`.
4. Add the regression tests to the file's existing `mod tests`, which already simulate the stash (`ex.set_extension(camel_api::ORIGINAL_MESSAGE_EXTENSION, original)` at :1755/:1803/:1855) — extend that setup to also set the provenance extension per case.

**Tests:** (command: `cargo test -p camel-processor --lib`)
- POSITIVE regressions (must fail before implementation):
  - `restore_reinstates_marker_for_untouched_original` (echo-counterexample regression): stash original message whose headers include `Content-Type: application/xml`; provenance extension `OriginalContentTypeInherited(true)`; live exchange: marker CLEARED by a simulated route write (marker absent) + header `Content-Type: application/json`; `use_original_message` enabled → `handle_step` with Handled disposition → returned exchange input has the restored original body/headers AND marker property is `Some` (fails before: restore leaves the marker absent, so the untouched original would be echoed).
  - `restore_removes_live_marker_when_snapshot_false`: stash original with NO ct header; provenance extension `OriginalContentTypeInherited(false)`; live exchange still carries the marker `Some` (later route writes never cleared it) → restore → marker `None` (fails before: a no-op restore leaves `Some`).
  - `restore_clears_marker_when_snapshot_absent`: stash original present, NO provenance extension, live marker `Some` → restore → marker `None` (fails before: no-op leaves `Some`; distinguishes the absent branch from a no-op implementation).
- NEGATIVE control (may already pass):
  - `restore_without_use_original_message_leaves_marker_untouched`: handler without `use_original_message`, marker cleared on the exchange → `handle_step` → marker still absent, input not restored.

**Acceptance:**
- `cargo test -p camel-processor --lib` exits 0.
- `cargo clippy -p camel-processor -- -D warnings` exits 0.

- [x] 2.5

## camel-core

### Task 3.1: Function patch apply clears marker (site 10)

**Files:**
- `crates/camel-core/src/step/function_step.rs` (modified)

**Steps:**
1. In the patch application (function_step.rs:153-156): BEFORE the loops consume the collections, compute the touch flags — `let set_touches_ct = patch.headers_set.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"));` and `let removed_touches_ct = patch.headers_removed.iter().any(|k| k.eq_ignore_ascii_case("content-type"));` — then run the existing loops unchanged, and clear the marker (`exchange.remove_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED)`) after the loops when `set_touches_ct || removed_touches_ct`. Do not inspect the vectors after the loops have moved their elements.
2. Import `PROPERTY_HTTP_CONTENT_TYPE_INHERITED` from `camel_api`.
3. Add tests to the file's existing `mod tests`, following its existing patch-apply test setup.

**Tests:** (command: `cargo test -p camel-core --lib`)
- POSITIVE regressions (must fail before implementation):
  - `function_patch_clears_marker_on_ct_set`: marker set → apply patch with `headers_set [("Content-Type", "application/json")]` → marker `None`.
  - `function_patch_clears_marker_on_ct_remove`: marker set → apply patch with `headers_removed ["content-type"]` → marker `None`.
- NEGATIVE control (may already pass):
  - `function_patch_keeps_marker_on_other_headers`: marker set → patch touching only `"X-Flag"` → marker `Some`.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 3.1

### Task 3.2: Route-entry marker snapshot capture (site 13, capture half)

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_compiler.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_compiler_tests.rs` (modified — the existing external test module; the route-channel stash fixtures live at lines 733-819)

**Steps:**
1. In the `use_original_message` stash block (route_compiler.rs:938-942), immediately after `ex.set_extension(ORIGINAL_MESSAGE_EXTENSION, original)`: capture marker presence — `let inherited = ex.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED).is_some();` — then ALWAYS replace the snapshot: `ex.set_extension(ORIGINAL_CONTENT_TYPE_INHERITED_EXTENSION, Arc::new(OriginalContentTypeInherited(inherited)))`. Unconditional replacement removes any stale extension from a parent route because `set_extension` overwrites the named entry — capture with the marker absent therefore stores `OriginalContentTypeInherited(false)`, replacing a stale `true` from an inherited parent exchange.
2. Do NOT gate capture on header presence; do not read the original message's headers for this.
3. Add the tests below to `route_compiler_tests.rs`, building on the existing route-channel fixtures there.

**Tests:** (command: `cargo test -p camel-core --lib`)
- POSITIVE regressions (must fail before implementation):
  - `route_entry_capture_sets_snapshot_when_marker_present`: exchange with marker property set → run the channel call with `use_original_message` enabled → extension reads `OriginalContentTypeInherited(true)`.
  - `route_entry_capture_replaces_stale_snapshot_when_marker_absent`: exchange carrying a stale `OriginalContentTypeInherited(true)` extension but no marker property (simulating a nested route after the parent cleared the marker) → run the channel call → extension reads `OriginalContentTypeInherited(false)`.
  - `nested_use_original_message_route_restores_route_set_header_without_resuppression` (the capture+restoration end-to-end regression per design.md:85): using the existing full-route channel fixture — parent route enables `use_original_message` and captures an inherited original (typed snapshot `true`); the parent then writes `Content-Type: application/json`, clearing the live marker; the pipeline enters another channel with `use_original_message` enabled (nested capture overwrites the stale `true` snapshot with `false`); the nested step overwrites the live input with its own message carrying `Content-Type: application/json` and fails; the error handler restores with Handled disposition → the restored route-set header is present, the live marker is absent, and the captured typed snapshot read from the exchange is `OriginalContentTypeInherited(false)`. (Fails before: presence-based inference or a stale `true` snapshot would re-suppress the route value.)

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 3.2

## camel-component-wasm

### Task 4.1: WASM host input replacement entry-list diff (site 9)

**Files:**
- `crates/components/camel-component-wasm/src/serde_bridge.rs` (modified)

**Steps:**
1. In `wasm_to_exchange` (serde_bridge.rs:195-201): BEFORE `original.input = wasm_to_message(wasm.input)`, snapshot the `Content-Type` entry set of the OLD input headers (same detector as ScriptMutator: `(String, Value)` pairs with `eq_ignore_ascii_case("content-type")`, sorted by exact key).
2. The final marker clear must happen AFTER the guest-properties merge loop (the merge at :199-201 re-inserts guest-echoed host properties and would otherwise re-arm a marker cleared before it): perform the input replacement, let the existing guest-properties merge run, THEN diff the old entry list against the NEW input's entry list; on any difference — added, changed, respelled, or REMOVED — call `original.remove_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED)`.
3. Add tests to the file's existing test module, built on this fixture pattern: construct a marked original (marker property set + header `Content-Type` present), serialize it with `exchange_to_wasm`, mutate ONLY the guest input headers per case, keep guest properties UNCHANGED (so the merge echoes the host properties back, including the marker property — the echo must not re-arm the marker), then call `wasm_to_exchange`.

**Tests:** (command: `cargo test -p camel-component-wasm --lib`)
- POSITIVE regressions (must fail before implementation):
  - `wasm_replacement_clears_marker_on_ct_removed`: original with marker set + header `Content-Type: application/json` → guest input headers drop the header (guest properties still echo the marker — assert the marker is present in the guest properties before `wasm_to_exchange`) → after `wasm_to_exchange`: header absent AND marker property absent (fails before: a clear placed before the property merge, or no clear, leaves the echoed marker re-arming inheritance).
  - `wasm_replacement_clears_marker_on_ct_changed`: guest replacement carries `Content-Type: text/html` (echoed properties kept) → marker `None`, header updated.
- NEGATIVE controls (may already pass):
  - `wasm_replacement_clears_marker_on_case_respell`: guest replacement carries `content-type: application/json` (lowercase, same value, echoed properties kept) → marker `None`.
  - `wasm_replacement_keeps_marker_when_ct_untouched`: replacement preserves the identical entry (echoed properties kept) → marker `Some`.

**Acceptance:**
- `cargo test -p camel-component-wasm --lib` exits 0.
- `cargo clippy -p camel-component-wasm -- -D warnings` exits 0.

- [x] 4.1

## camel-component-cxf

### Task 5.1: CXF producer response mapping clears marker (site 8)

**Files:**
- `crates/components/camel-cxf/src/producer.rs` (modified)

**Steps:**
1. Extract the existing response-header mapping loop (producer.rs:369-372) into a small private helper in the same file: `fn apply_response_header_mapping(exchange: &mut Exchange, headers: impl IntoIterator<Item = (String, String)>)` — the production call site passes the collected response headers; behaviour is unchanged apart from the clearing rule below. (The existing producer unit tests cover readiness, body conversion, and errors only — there is no successful-response fixture to reuse, so tests drive the helper directly.)
2. Inside the helper, when a mapped key `k.eq_ignore_ascii_case("content-type")`, call `exchange.remove_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED)` after the `set_header`.
3. Import the constant from `camel_api`.
4. Add tests to the file's existing `mod tests`, driving `apply_response_header_mapping` directly with explicit header pairs (no server fixture).

**Tests:** (command: `cargo test -p camel-component-cxf --lib`)
- POSITIVE regression (must fail before implementation):
  - `cxf_response_mapping_clears_marker_on_ct`: exchange with marker set → `apply_response_header_mapping(ex, [("Content-Type", "text/xml"), ("X-Other", "v")])` → marker `None`, header present.
- NEGATIVE control (may already pass):
  - `cxf_response_mapping_keeps_marker_on_other_headers`: mapping over headers without `Content-Type` → marker `Some`.

**Acceptance:**
- `cargo test -p camel-component-cxf --lib` exits 0.
- `cargo clippy -p camel-component-cxf -- -D warnings` exits 0.

- [x] 5.1

## camel-component-http

### Task 6.1: Ingress marker, finaliser channel rewrite, producer mapping clearing

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)
- `crates/components/camel-http/src/lib_tests.rs` (modified)

**Steps:**
1. In `record_request_content_type` (lib.rs:3781-3791): inside the existing `if let Some(value)` guard, additionally `exchange.set_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED, Value::Bool(true));`.
2. Change `response_content_type` (lib.rs:3795-3800) to `fn response_content_type(msg: &Message, inherited: bool) -> Option<String>`: when `inherited` is true return `None`; otherwise return `msg.header_ic("Content-Type").and_then(Value::as_str).map(str::to_string)` with no comparison.
3. At the reply spawn-site (lib.rs:~2340): ADD the marker read alongside the existing value read — `let inherited = out.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED).is_some();` — the value read (`PROPERTY_REQUEST_CONTENT_TYPE`) remains and is used only by the stream-metadata filter; call `response_content_type(&out.input, inherited)`. The stream-metadata call site is NOT marker-gated: keep the existing unconditional `response_stream_content_type(metadata, recorded_value)` call in both marker states (blessed decision: the metadata filter is marker-independent; gating it on the marker would resurrect the ingress copy after a route clears the marker). Extract that spawn-site wiring into a testable pure helper `fn stream_metadata_content_type(metadata: Option<String>, inherited: bool, recorded_value: Option<&str>) -> Option<String>` that ignores `inherited` and returns `response_stream_content_type(metadata, recorded_value)` — same extraction precedent as `select_response_headers`; the spawn-site calls the helper. `response_stream_content_type` itself is not modified.
4. Extract the non-stream body-kind inference into a shared pure helper `fn inferred_content_type(body: &Body, inherited: bool) -> Option<String>`: `Text` -> `Some("text/plain; charset=utf-8")`, `Xml` -> `Some("application/xml")`, `Json` -> `Some("application/json")` ONLY when `inherited` is false; `Bytes`/`Empty` -> `None` always; every other variant -> `None` (streams stay on the separate `stream_metadata_content_type` path from step 3). The production reply branch (lib.rs:2348-2385) CALLS this helper — the `if !inherited` rule must exist in production code, not only in tests. For `Body::Stream`, the fallback content type is `stream_metadata_content_type(...)`.
5. In the producer response-header mapping loop (lib.rs:~3513): extract the loop into a private testable helper `fn apply_response_header_mapping(exchange: &mut Exchange, headers: &http::HeaderMap, skip_response_headers: &[String])` (same extraction precedent as `select_response_headers`: extracted from the inline filter for unit testability), and inside it, when the mapped header key `eq_ignore_ascii_case("content-type")`, clear the marker on the exchange after the `set_header`.
6. Update every legacy unit test that pins the old value-equality signature — SEVEN call sites: the four at lib_tests.rs:9114-9141 (`response_content_type_reads_lowercase_header`, `response_content_type_reads_mixed_case_header` — keep names, pass `inherited: false`; `response_content_type_drops_inherited_equal_value` — REPLACE with `response_content_type_suppressed_while_marker_set`: marker true -> `None` even when the header equals the recorded value; `response_content_type_keeps_changed_value` — keep, pass `inherited: false`) AND the three round-trip tests: `reply_content_type_round_trip_no_echo` (:9214) and `reply_content_type_round_trip_identical_is_inherited` (:9260) pass `inherited: true` (ingress now sets the marker, so their suppression assertions hold as written), and `reply_content_type_round_trip_lowercase_override` (:9238) emulates the route write it performs with the direct `set_header`: clear the marker in the test setup, then pass `inherited: false`, keeping the `Some("application/json")` assertion. Add `response_content_type_emits_route_value_equal_to_request` (marker false, header equals recorded value -> `Some(value)`). Keep all `select_response_headers` tests unchanged.
7. Add stream-metadata helper tests as specified in the Tests block below (the helper itself is from step 3).
8. Add ingress tests: `record_request_content_type_sets_marker` (headers with `Content-Type` -> marker `Some` + value property unchanged) and `record_request_content_type_without_header_leaves_no_marker`.

**Tests:** (command: `cargo test -p camel-component-http --lib`)
- POSITIVE regressions (must fail before implementation):
  - `record_request_content_type_sets_marker`: as in step 8 (fails before: no marker is recorded).
  - `response_content_type_suppressed_while_marker_set`: message with header `Content-Type: application/xml`, `inherited: true` -> `None` (fails before: signature takes `Option<&str>`; after signature change the value-equality drop only fires on equal values — this test pins the marker override).
  - `response_content_type_emits_route_value_equal_to_request`: header `Content-Type: application/xml`, `inherited: false` -> `Some("application/xml")` (fails before: value-equality drops the equal value).
  - `inferred_content_type_suppressed_while_marker_set`: `inferred_content_type(&Body::Json(...), true)` -> `None` and `inferred_content_type(&Body::Text("x"), true)` -> `None` (fails before: helper does not exist; after it exists, a broken production gate shows here first).
  - `http_producer_mapping_clears_marker_on_ct`: exchange with marker set; `apply_response_header_mapping` over response headers containing `Content-Type: application/json` and no skip entry -> marker `None`, header mapped (fails before: no clearing in the mapping).
- NEGATIVE controls (may already pass):
  - `record_request_content_type_without_header_leaves_no_marker`: as in step 8.
  - `inferred_content_type_unmarked_still_infers`: `inferred_content_type(&Body::Json(...), false)` -> `Some("application/json")`, Bytes/Empty -> `None` both marker states.
  - `stream_metadata_equal_to_request_suppressed`: `stream_metadata_content_type(Some("application/xml"), true, Some("application/xml"))` -> `None`.
  - `stream_metadata_filter_is_marker_independent`: `stream_metadata_content_type(Some("application/xml"), false, Some("application/xml"))` -> `None` (marker cleared must not resurrect the ingress copy), and `stream_metadata_content_type(Some("video/mp4"), false, Some("application/xml"))` -> `Some("video/mp4")`.
  - `http_producer_mapping_keeps_marker_on_other_headers`: mapping over headers without `Content-Type` (with a `skipResponseHeaders` entry for one non-ct header) -> marker `Some`.
- All `select_response_headers` tests, lowercase/mixed-case tests, and inference integration tests stay green; the three `reply_content_type_round_trip_*` tests pass after their specified rewrites.

**Acceptance:**
- `cargo test -p camel-component-http --lib` exits 0.
- `cargo clippy -p camel-component-http -- -D warnings` exits 0.

- [x] 6.1

### Task 6.2: The eight acceptance tests

**Files:**
- `crates/components/camel-http/Cargo.toml` (modified)
- `crates/components/camel-http/src/lib_tests.rs` (modified)

**Steps:**
1. Add `camel-processor = { workspace = true }` and `camel-builder = { workspace = true }` to `[dev-dependencies]` in Cargo.toml (cycle-safe: neither depends on camel-component-http; `cargo xtask lint-publish-cycles` verifies). camel-builder is required by test 6: `set_header_fn` is a builder method (camel-builder/src/lib.rs:153) that compiles to `DynamicSetHeader`.
2. In `lib_tests.rs` next to the reply-header suite (near line 9100), add a local helper `fn inherited_exchange(header: Option<(&str, &str)>, body: Body) -> Exchange` that builds an `Exchange`: sets the request content-type header + `CamelHttpRequestContentType` value property + marker property when `header` is `Some`, otherwise leaves all three unset; body comes from the `body` argument. Each test then arranges in one call.
3. Write the eight tests below. Writers 1-2, 5-6, 8 drive the real processors via `tower::Service::call` (`SetHeader::new(IdentityProcessor, ...)`, `RemoveHeader::new(IdentityProcessor, key)`); test 6 uses the existing builder EXECUTION pattern (camel-builder/src/lib.rs:2712-2748): build a route with `set_header_fn`, extract its processor steps, compose with `PipelineRuntimeCtx::compile_time`, and execute it — merely constructing `DynamicSetHeader` again would not cover the fluent builder surface. Every test asserts the finaliser outcome through the REAL production helpers (`response_content_type(&out.input, marker_after)` and `inferred_content_type(&body, marker_after)` combined via `select_response_headers` where inference participates) — never by re-implementing the `if !inherited` rule inside the test. Follow the existing unit-test style of the file (`#[test]` / `tokio::test` as the processor calls require).
4. Mutation spot-checks (behavior-specific, replace the old blanket revert rule): (a) temporarily disable the SetHeader clearing call -> tests 1, 3, and 7 must fail; (b) temporarily disable the DynamicSetHeader clearing call -> test 5 must fail; (c) temporarily disable the RemoveHeader clearing call -> test 8 must fail; (d) temporarily remove the PRODUCTION inference gate (make `inferred_content_type` ignore `inherited`) -> test 4 must fail. Restore all mutations; the full suite must be GREEN at task end.

**Tests:** (command: `cargo test -p camel-component-http --lib`)
- `route_set_content_type_equal_to_request_is_preserved`: ingress recorded `application/json` (marker set), JSON text body → `SetHeader` writes `Content-Type: application/json` → marker cleared → reply `application/json` (red today: value-equality drops it and `text/plain` is emitted).
- `route_set_content_type_with_charset_equal_to_request_is_preserved`: same with `application/json; charset=utf-8` → preserved verbatim.
- `route_set_content_type_different_from_request_is_preserved`: recorded `application/json`, route writes `text/csv` → reply `text/csv`.
- `untouched_request_content_type_is_not_echoed`: marker set, no writer runs, `Body::Json` body → resolve user ct via `response_content_type(&msg, true)` (None) and inference via the PRODUCTION helper `inferred_content_type(&body, true)` (None); feed `(None, None)` through the real `select_response_headers` → the reply carries NO `Content-Type` (header and inference both suppressed; no duplicated gate logic in the test).
- `content_type_written_by_dynamic_set_header_is_preserved`: `DynamicSetHeader` writes `application/json` equal to the recorded request → marker cleared → reply `application/json`.
- `content_type_written_by_header_dsl_is_preserved`: the fluent function-step writer (`set_header_fn` via the builder execution pattern of step 3) writes `application/json` → preserved.
- `content_type_header_case_is_insensitive`: two exchanges, writers keyed `content-type` and `Content-Type` respectively → both clear the marker and both replies emit the written value.
- `removing_content_type_leaves_no_inherited_value`: recorded `application/json`, `RemoveHeader("Content-Type")` runs → assert the MARKER is absent after the call, header gone, `Bytes` body → via the production helpers (`response_content_type(msg, false)` -> `None`, `inferred_content_type(&Body::Bytes, false)` -> `None`) the reply has NO `Content-Type`; additionally assert `stream_metadata_content_type(Some("application/json"), false, Some("application/json"))` -> `None` — no resurrection through the fallback channel either. (The marker-absence assertion is essential: without the clearing call the visible outputs would be unchanged for a Bytes body.)

**Acceptance:**
- `cargo test -p camel-component-http --lib` exits 0 with all eight named tests passing.
- `cargo xtask lint-publish-cycles` exits 0.
- `cargo clippy -p camel-component-http -- -D warnings` exits 0.

- [x] 6.2

### Task 6.3: CHANGELOG entry and local hygiene

**Files:**
- `CHANGELOG.md` (modified, repo root)

**Steps:**
1. Add one entry under `## Unreleased` / `### Fixes` following the existing entry style: client-visible fix — a route that sets `Content-Type` is now always emitted verbatim, including when it equals the request value (provenance marker replaces value equality; bd rc-tf0k4); note the secondary client-visible change: replies to requests with a `Content-Type` whose route only rewrites the body no longer emit a request-dependent inferred type, and archive/stream fragment replies no longer depend on the request value.
2. Run the local hygiene battery and fix anything this change introduced: `cargo fmt --check --all`, `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings`, `cargo xtask lint-unwrap`, `cargo xtask lint-glob-imports`, `cargo xtask lint-secrets`, `cargo xtask lint-publish-cycles`.

**Tests:** (command: `cargo test -p camel-component-http --lib && cargo test -p camel-processor --lib`)
- No new tests; the suite must stay green after hygiene fixes.

**Acceptance:**
- `cargo fmt --check --all` exits 0.
- The clippy invocation above exits 0.
- `cargo xtask lint-unwrap`, `lint-glob-imports`, `lint-secrets`, `lint-publish-cycles` exit 0.
- CHANGELOG.md contains the rc-tf0k4 entry.

- [x] 6.3
