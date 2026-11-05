# Proposal: http-content-type-provenance

## Why

A route that deliberately sets `Content-Type` loses it whenever the client happens to send the same value. The reply finaliser tests inheritance by VALUE equality against the `CamelHttpRequestContentType` property (`crates/components/camel-http/src/lib.rs:3776-3800`): a response content type equal to the recorded request value is dropped, so the reply falls back to body inference (`text/plain; charset=utf-8` for a JSON text body). The emitted reply therefore depends on the request, not on the route (bd rc-tf0k4; downstream report camel-112, 0.62 -> 0.65).

The archived change `2026-11-02-http-header-case-semantics` specified value equality as the provenance boundary on purpose ("a route that sets exactly the request value is treated as inherited"). The boundary is wrong: value equality cannot distinguish "route wrote it" from "request brought it". Provenance can.

## What Changes

- Ingress records the inherited state with a marker exchange property (`CamelHttpContentTypeInherited`), alongside the existing value property (unchanged name and type).
- Every route path that writes, removes, replaces, or restores the `Content-Type` header clears or restores the marker. The writer set, established by grep and three expert-review rounds, is: `SetHeader`, `SetHeaderIfAbsent`, `DynamicSetHeader`, `DynamicSetHeaderIfAbsent`, `RemoveHeader`, `ScriptMutator` (before/after entry-list diff over all `Content-Type`-keyed entries, exact because both language surfaces commit changed-only per key), the HTTP and CXF producer response-header mappings, WASM host input replacement (entry-list diff, guest removals included), function patch application, claim-check header restore, the zip/tar/stream fragment `Content-Type` stripping sites, and error-handler original-message restoration (the marker presence snapshotted at route entry with `CamelOriginalMessage` is restored with it; capture always replaces the snapshot so a stale presence cannot survive into a nested route, and it is never inferred from header presence).
- The reply finaliser suppresses the reply content type only while the marker is set, and never compares values for the message header or body inference. The stream-metadata value filter is unchanged and marker-independent: a metadata value equal to the recorded request value stays suppressed in both marker states, so removing the header cannot unmask the ingress copy, while a differing route-produced metadata value is still emitted.
- Delta on `http-emission-correctness` replaces the value-equality rule with the provenance invariant. A `CHANGELOG.md` entry documents the client-visible fix.

Excluded: header case folding, `header_policy::excluded_response`, the `CamelHttpRequestContentType` property name and type (all non-goals).

## Acceptance criteria

- `route_set_content_type_equal_to_request_is_preserved` — request `application/json`, route sets the same value, JSON text body -> reply carries `application/json` (today: `text/plain`).
- `route_set_content_type_with_charset_equal_to_request_is_preserved` — charset-qualified equal value preserved.
- `route_set_content_type_different_from_request_is_preserved` — differing value still emitted (unchanged-behaviour guard).
- `untouched_request_content_type_is_not_echoed` — no route write -> no `Content-Type` (existing guarantee, no regression).
- `content_type_written_by_dynamic_set_header_is_preserved` and `content_type_written_by_header_dsl_is_preserved` — function-step writers covered.
- `content_type_header_case_is_insensitive` — `content-type` vs `Content-Type` writers behave the same.
- `removing_content_type_leaves_no_inherited_value` — `remove_header` then reply -> absent, no resurrection of the request value.
- `cargo test -p camel-http` green; all eight tests written first and verified red.

## Risk budget

- Client-visible change, documented in `CHANGELOG.md`: routes that set `Content-Type` now always see it emitted; replies to requests with a `Content-Type` whose routes only rewrite the body no longer emit a request-dependent inferred type.
- Acceptable: a reply `Content-Type` that previously varied with the request value becomes request-independent.
- Out of bounds: producer-side outbound header policy, header classification lists, property renames, splitter or stream-codec header behaviour.

## Affected crates

- `camel-api`: shared marker property constant, `Exchange::remove_property`, and the snapshot extension key.
- `camel-processor`: marker clearing in the five writer processors, `ScriptMutator`, claim-check header restore, the zip/tar/stream fragment stripping sites, and marker restoration from the original-message snapshot.
- `camel-core`: function patch application clearing, and the marker snapshot captured with `CamelOriginalMessage`.
- `camel-cxf`: producer response-mapping clearing.
- `camel-component-wasm`: host input-replacement clearing.
- `camel-http`: ingress marker record, producer response-mapping clearing, reply finaliser rewrite, acceptance and regression unit tests.

Bd: rc-tf0k4
