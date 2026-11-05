# Design: http-content-type-provenance

## Approach

Decision: **A, provenance marker** (mission-preferred; expert-approved over B in the first spec-bless round). Value equality cannot distinguish "the route wrote the value" from "the request brought the value"; a marker can. The invariant this change implements:

> An untouched inbound request `Content-Type` is never echoed; a route-set value is always emitted verbatim, including when it equals the request value. Header names stay case-insensitive (`header_ic`).

### Marker

- `camel-api` gains one shared constant, `PROPERTY_HTTP_CONTENT_TYPE_INHERITED = "CamelHttpContentTypeInherited"`, next to `Exchange` in `crates/camel-api/src/exchange.rs`. Every crate in the writer set below already depends on `camel-api`, so no new dependency edge appears.
- Marker value is `Value::Bool(true)`, set at ingress only when the request carried a `Content-Type` header (same guard as the existing value record).
- `camel-api` gains `Exchange::remove_property` (public; the error-clearing code already removes properties internally). Writers call it; removal is idempotent.
- The existing `CamelHttpRequestContentType` value property keeps its name, type, and ingress write. Routes may read it. The reply finaliser keeps reading it for exactly one purpose: the stream-metadata value filter (below). The message-header and body-inference channels never consult it.

### Writer set (complete, established by grep, expert review, and two verification rounds)

Every production path that writes, removes, or replaces `Content-Type` on an exchange input message clears the marker when a key matches ASCII-case-insensitively; the one restoring path (site 13) reinstates the snapshotted presence:

| # | Writer | Site | Clearing rule |
|---|---|---|---|
| 1 | `SetHeader` | `crates/camel-processor/src/set_header.rs:68` | key is `Content-Type` |
| 2 | `SetHeaderIfAbsent` | `crates/camel-processor/src/set_header.rs:112` | same, only when the write happens |
| 3 | `DynamicSetHeader` | `crates/camel-processor/src/dynamic_set_header.rs:80` | key is `Content-Type` |
| 4 | `DynamicSetHeaderIfAbsent` | `crates/camel-processor/src/dynamic_set_header.rs:136` | same, only when the write happens |
| 5 | `RemoveHeader` | `crates/camel-processor/src/remove_header.rs:60` | removed key is `Content-Type` |
| 6 | `ScriptMutator` | `crates/camel-processor/src/script_mutator.rs`, `call` | detector: sorted list of every `Content-Type`-keyed entry (exact key spelling + value) before vs after the expression; any difference clears |
| 7 | Producer response-header mapping (HTTP) | `crates/components/camel-http/src/lib.rs:~3513` | a mapped upstream response header keyed `Content-Type` |
| 8 | Producer response-header mapping (CXF) | `crates/components/camel-cxf/src/producer.rs:370-372` | same |
| 9 | WASM host input replacement | `camel-component-wasm` `serde_bridge.rs`, `wasm_to_exchange` | entry-list diff over the `Content-Type` entries before vs after the guest replacement; any landed difference — added, changed, respelled, or **removed** — clears the marker |
| 10 | Function patch apply | `crates/camel-core/src/step/function_step.rs:153-156` | `patch.headers_set` / `patch.headers_removed` keyed `Content-Type` |
| 11 | Claim-check header restore | `crates/camel-processor/src/claim_check.rs:140-172` | a headers action that inserts or drops a `Content-Type` key (include/exclude/remove patterns and `HeadersAction::All(Remove)`) |
| 12 | Archive/stream fragment stripping | `zip_splitter.rs:331-332`, `tar_splitter.rs:530-531`, `stream_codec.rs:128-129` (`fragment_stream_exchange`) | the fragment's deliberate `Content-Type` removal clears the fragment's marker, so fragment replies describe their own bodies instead of inheriting a request-dependent suppression |
| 13 | Original-message restoration | snapshot: `crates/camel-core/src/lifecycle/adapters/route_compiler.rs:938-942`; restore: `crates/camel-processor/src/error_handler.rs:139-146` (`restore_original_message_if_enabled`) | the marker presence is snapshotted with `CamelOriginalMessage` at route entry and restored from that snapshot after `exchange.input = orig.clone()`: capture always replaces the provenance extension (insert when present, remove when absent), restoration follows it. Never inferred from header presence |

Sites 7-9 and 12 came out of the blessing rounds: the upstream response mappings (HTTP and CXF), the WASM guest input replacement, and the fragment `Content-Type` strips are all production mutations of reply-path input messages that a header-writers-only grep misses. For site 12 the strip is a route-path action on `Content-Type`: today the fragment reply's content type depends on the request value (value equality against the copied property); after the change the fragment's marker is cleared, body inference describes the fragment body, and the reply is request-independent.

**Original-message restoration soundness (site 13).** `RouteChannelService` stashes the route-entry message as the `CamelOriginalMessage` extension (`route_compiler.rs:938-942`) and the error handler can restore it wholesale (`exchange.input = orig.clone()`, `error_handler.rs:144`) before a `Handled` policy returns it as the reply. The snapshot is route-entry state, not HTTP-ingress state — `camel-api::ORIGINAL_MESSAGE_EXTENSION` documents "the pre-route Message" — and a previous route may already have written `Content-Type`. Header presence therefore cannot reproduce provenance: a nested route reached through `direct:` can snapshot a route-set value whose marker was correctly cleared. The marker presence is snapshotted as its own typed extension (`CamelOriginalContentTypeInherited`) next to the message stash, and only when `use_original_message` is enabled. Capture replaces state unconditionally: the extension is inserted when `PROPERTY_HTTP_CONTENT_TYPE_INHERITED` is present at snapshot time, and any pre-existing provenance extension is removed when it is absent. Both halves are required — `Exchange::set_extension` replaces only the named entry and a nested route receives the parent's extension map, so a route that inherits a stale `true` snapshot (parent route with `useOriginalMessage` captured an inherited original, then set `Content-Type`) must not leave that snapshot paired with its own route-set message. Restoration reads the snapshot: set when present, removed when absent. Counterexample this closes: HTTP route A sets `Content-Type: application/json` (marker cleared) -> `direct:B` -> B enables `useOriginalMessage`, snapshots the route-set header, fails -> restoration reinstates JSON; a presence-based rule would suppress the route's deliberate value, the snapshot rule leaves it emitted. The ingress case is unchanged in behaviour: an untouched request carries the marker into the snapshot, restoration reinstates it, and the finaliser suppresses the echo. A production sweep for wholesale input replacement (`\.input = `) found exactly two sites: this restore and the WASM bridge; both are covered.

**Script detector soundness (site 6).** Both language surfaces commit changed-only per exact key: rhai builds `EntryDelta::Set`/`EntryDelta::Remove` from `map_deltas` and applies only those deltas (`camel-language-rhai/src/lib.rs:958-980`, committed at `:1351`); JS documents `apply_map_changes` as changed-only write-back (`camel-language-js/src/expression.rs:~409-430`). Two consequences make the before/after entry-list detector exact:

- A same-value assignment on an existing exact key produces no delta, does not land, and the entry list is unchanged — nothing landed is suppressed, and today's behaviour is preserved bit-for-bit.
- A case-spelling rename (`Content-Type` -> `content-type`, same value) lands as a remove plus an insert: the resolved `header_ic` value is unchanged, but the entry list (exact spellings) changes, so the detector clears the marker where a value-only comparison would wrongly keep it.

A script that only rewrites the body never changes the header entry list and never clears the marker. Error paths (eval failure, timeout, panic) roll back untouched and clear nothing. Write-intent is therefore defined as: a landed mutation of the `Content-Type` entry set.

**Verified non-writers.** The splitter family's other header writes are Camel-namespaced (`CAMEL_MIME_PART_*`, `CamelSplit*`, tar/zip/stream origin keys), which `select_response_headers` drops as Camel-namespace anyway; the MIME inherited-header cleanup at `mime_splitter.rs:199` removes only `CAMEL_MIME_PART_*` keys. `set_error` writes `CamelHttpResponseCode`/`Text` (Camel namespace). The aggregator and content enricher perform no `Content-Type` header writes. The declarative DSL compiles to the processors above (`camel-dsl/src/compile.rs:1077-1093`, `camel-core/src/lifecycle/adapters/step_compilers/core.rs:366-414`), so DSL routes inherit clearing with no DSL change.

**Documented escape hatch.** Custom `.process` closures and direct `Message::headers` mutation bypass provenance tracking, exactly as they bypass header normalisation today (precedent: the archived `2026-11-02-http-header-case-semantics` design keeps the raw `headers` field as a documented escape hatch). The delta spec states the invariant over route steps and the escape-hatch boundary.

### Reply finaliser (`camel-http`)

At the spawn-site reply block (`lib.rs` ~2340-2390):

- Marker: `content_type_inherited = out.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED).is_some()`.
- Message-header channel: `response_content_type(msg, inherited: bool)` returns `None` while the marker is set, otherwise the `header_ic("Content-Type")` value verbatim. No value comparison.
- Body-kind inference (`Body::Text`/`Xml`/`Json`): suppressed while the marker is set; unchanged when the marker is absent — including the request-without-`Content-Type` case, where no marker ever exists.
- Stream-metadata channel: **unchanged and marker-independent, and strictly a fallback.** `response_stream_content_type` keeps filtering a metadata value equal to the recorded request value, in both marker states. Rationale: at ingress the request `Content-Type` is copied into the request stream's metadata, so an equal value is the ingress copy, never a route choice — suppressing it unconditionally is what kills the resurrection hole (marker cleared by `remove_header` must not unmask the ingress copy). A route-replaced stream with a *differing* metadata type is emitted — but only as the fallback when no route-set message header resolved, exactly as `select_response_headers` already orders the two channels; the reply never carries two `Content-Type` values. A route-replaced stream whose metadata type coincides with the request value stays suppressed, as today; the message-header channel remains the first-class route-set path (`set_header("Content-Type", ...)` clears the marker and wins over metadata). Full provenance for metadata alone would require tracking every body-replacing step (`set_body`, producers, splitters, marshallers) — a far larger surface with no acceptance requirement behind it.
- `select_response_headers` and `header_policy::excluded_response` are untouched.

### Corner cases, decided

- Route writes a value equal to the request: marker cleared, value emitted verbatim. This is the fix.
- `remove_header("Content-Type")` clears the marker; the header is gone; body-kind inference then applies exactly as it does today for a differing request, making the reply request-independent. A retained request stream is still filtered by the marker-independent metadata rule, so nothing resurrects. The acceptance test uses a non-inferable (`Bytes`) body and asserts the header is absent.
- Route rewrites only the body (no `Content-Type` write) while the request carried one: marker set; the message header and body-kind inference are suppressed. For a non-stream body the reply carries no `Content-Type` — today it emits a value that depends on the request; after the change it depends only on route actions. A `Body::Stream` still follows the marker-independent metadata rule: a differing metadata value is emitted as the fallback, an equal one stays suppressed. Documented in `CHANGELOG.md`.
- Archive/stream splitters strip `Content-Type` from fragments by design (`Content-Length` alongside): the strip clears the fragment's marker, so fragment replies infer from their own bodies and no longer depend on the request value. Today that reply is request-dependent (equal request value -> nothing emitted, differing -> inference emitted) — the same defect class this change fixes.
- `useOriginalMessage` error handling restores the route-entry message on failure: the marker is restored from the snapshot taken with it (site 13), where capture always replaces the previous snapshot. Two cases follow. An untouched original whose marker was set at ingress is suppressed again (today it is echoed whenever a route step wrote any `Content-Type` on the way). A route-set original inherited from a previous route through `direct:` keeps its value emitted, because its snapshot recorded no marker and the stale `true` snapshot of the parent is removed — presence-based reconstruction would wrongly suppress it.
- WASM guests can remove or respell `Content-Type` on the replacement input: the entry-list diff (site 9) treats a removal like `RemoveHeader` — the marker clears, body inference describes the guest-produced body — instead of leaving inheritance armed over a message the guest rewrote.
- Request without `Content-Type`: no marker exists; every channel behaves exactly as today.
- Ingress header forwarding (`lib.rs:2158-2166`) copies request headers onto the message before the pipeline; it is not a route write. The marker is recorded after it.

### Tests

Acceptance tests live in `crates/components/camel-http/src/lib_tests.rs` next to the reply-header suite (near line 9100), in the existing unit-test style. Each test drives the real writer processor (the same `Service<Exchange>` call pattern the `camel-processor` tests use) against an exchange carrying the ingress marker, then asserts the finaliser helpers' outcome:

1. `route_set_content_type_equal_to_request_is_preserved` — request `application/json` recorded, `SetHeader` writes `application/json`, JSON text body -> reply `application/json` (red today: `text/plain`).
2. `route_set_content_type_with_charset_equal_to_request_is_preserved` — same with `application/json; charset=utf-8`.
3. `route_set_content_type_different_from_request_is_preserved` — differing value still emitted.
4. `untouched_request_content_type_is_not_echoed` — marker set, no writer runs, `Body::Json` body -> no `Content-Type` (inference suppressed with the header).
5. `content_type_written_by_dynamic_set_header_is_preserved` — `DynamicSetHeader` writer coverage.
6. `content_type_written_by_header_dsl_is_preserved` — the function-step writer (`set_header_fn` -> `DynamicSetHeader`) coverage.
7. `content_type_header_case_is_insensitive` — `content-type` and `Content-Type` spellings clear the marker identically.
8. `removing_content_type_leaves_no_inherited_value` — `RemoveHeader` clears marker and header; `Bytes` body -> reply absent, request value not resurrected (header and metadata channels both checked).

Processor-level unit tests accompany each new clearing site (writers 1-13): marker set + `Content-Type` mutation -> marker gone; marker set + non-`Content-Type` mutation -> marker intact; for the script detector additionally: same-value exact-key assignment -> marker intact, case-spelling rename -> marker cleared; for WASM: guest removal of the header -> marker cleared; for the error handler two regressions: (a) untouched ingress original restored -> marker restored, `Handled` reply suppressed (the echo counterexample), and (b) both routes enabling `useOriginalMessage`: the parent captures an inherited original, sets `Content-Type` (marker cleared), and the nested route snapshots that route-set message -> the stale `true` snapshot is replaced, the marker stays absent, and the route value is emitted (the capture-must-invalidate regression, so neither presence-based inference nor a stale extension can re-suppress it). Existing integration tests (`test_content_type_inferred_for_*`, `test_content_type_from_stream_metadata`, `test_user_content_type_*`) send GET requests without a `Content-Type` header, so no marker exists and their expectations are unchanged. Existing unit tests that pin the value-equality signature (`response_content_type_drops_inherited_equal_value` and neighbours) are updated to the marker semantics as part of the delta.

## Affected crates

- `camel-api`: marker property constant, `Exchange::remove_property`, and the `CamelOriginalContentTypeInherited` extension key (+ tests).
- `camel-processor`: marker clearing in `SetHeader`, `SetHeaderIfAbsent`, `DynamicSetHeader`, `DynamicSetHeaderIfAbsent`, `RemoveHeader`, `ScriptMutator`, claim-check header restore, the zip/tar/stream fragment stripping sites, and marker restoration from the original-message snapshot (+ tests per site).
- `camel-core`: function patch apply clearing, and the marker snapshot captured next to `CamelOriginalMessage` in `route_compiler.rs` (+ test).
- `camel-http`: ingress marker record, producer response-mapping clearing, finaliser channel rewrite, the eight acceptance tests, updated value-equality unit tests.
- `camel-cxf`: producer response-mapping clearing (+ test).
- `camel-component-wasm`: host input-replacement clearing in `wasm_to_exchange` (+ test).
- `camel-language-rhai` / `camel-language-js`: none (changed-only commit semantics already guarantee the detector's exactness).
- `camel-dsl`: none.
- Root `CHANGELOG.md`: one entry under Unreleased/Fixes.

## Architecture boundaries

- Runtime/processor layer owns marker clearing: the clearing lives where the header writes live. No new component-to-core coupling beyond the shared `camel-api` constant.
- `camel-http` owns ingress recording, producer response-mapping clearing, and reply finalisation, as today.
- The DSL and language crates are untouched; declarative steps and scripts reach the writers through the existing compile and commit paths.
- Non-goals honoured: no change to header case folding, to `header_policy::excluded_response`, or to the `CamelHttpRequestContentType` property name/type.

## Alternatives considered

- **B. Do not copy the request `Content-Type` into message headers at ingress; keep it only in the property.** Rejected. The ingress forward loop copies all request headers generically (`lib.rs:2158-2166`), and production code reads the forwarded value back: `ContentNegotiationProcessor` resolves the inbound `Content-Type` with `exchange.input.header_ic("Content-Type")` for request matching (`crates/camel-processor/src/content_negotiation.rs:49-54`). Carving the header out of ingress forwarding breaks that contract (415/406 negotiation would see no type) and any other route reading the request type from the message. It also does nothing for the stream-metadata channel. The marker approach keeps the message surface intact and targets suppression exactly.
- **Generation counter on the message header map.** Rejected: touches `camel-api::Message` plumbing for every header write in the framework to solve a content-type-only problem; the property marker is narrower and fully sufficient given the enumerated writer set.
- **Marker-gated stream filtering.** Rejected in the first blessing round: a cleared marker (e.g. after `remove_header`) would pass an ingress-copied metadata value verbatim and resurrect the request value, while a route-replaced stream with an equal metadata type would be wrongly suppressed. The metadata filter stays value-based and marker-independent (unchanged behaviour), which closes the resurrection hole without a body-write tracking surface.
