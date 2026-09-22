# Proposal: namebloat

## Why

`camel-lint`'s route-view walk (`document.rs`) decides where to look for
endpoint URIs using GLOBAL name sets derived from the whole embedded
`route-schema.json`: `CONTAINER_KEYS` (every object / array-of-object
property name anywhere in the schema) and a `URI_KEYS` allowlist. REST
response `headers` is a free-form map (`type: object`,
`additionalProperties: true`), so `headers` lands in the global container
set and the walk descends into ARBITRARY user-keyed header maps. A header
named `to`, `uri`, or `endpoints` then matches `URI_KEYS` and its ordinary
string value (e.g. `timer:foo?frequency=1s`) is emitted as an endpoint
URI — R-URI-known flags false `UnknownOption`/`UnverifiedScheme`
diagnostics on valid REST DSL (bd rc-ni8qu, retro520 finding; territory:
rest-block ROUTE_SCHEMA modeling, 54a1277c + 7c5ae43e). The same
class one property over — `security_policy.config`, a free-form string
map — escapes capture today only by accident (its
`type: ["object", "null"]` list slips past the container check's
string equality); a context-scoped walk makes that opacity structural
instead of incidental.

The canonical `route-lint` spec already says traversal SHALL be driven by
the schema; the global-name implementation violates that in spirit — an
arbitrary set standing in for schema context (retro520 blind-spot
checklist: "arbitrary maps/collections standing in for schema context").

## What Changes

- `document.rs`: the CST walk carries a schema CONTEXT (the candidate
  subschemas applicable to the current node — `$ref`-resolved,
  composition expanded). Key interpretation is scoped to that context:
  a key DECLARED by the current node's schema dispatches on its own
  subschema (URI key → endpoint emission; structured container → branch
  recursion); a free-form subschema (object with NO declared properties —
  arbitrary user-keyed map) is an OPAQUE LEAF and is never walked.
  UNDECLARED keys that the active schema PERMITS (`additionalProperties`
  absent/true/typed) are legitimate user data — opaque or dispatched on
  the typed `additionalProperties` schema, never globally interpreted.
  Only undeclared keys REJECTED by every active candidate
  (`additionalProperties: false`) keep a legacy global-name fallback
  for schema-invalid shape tolerance (existing behavior for malformed
  docs), with free-form maps excluded from the fallback container set.
- Root context detection mirrors R-SCHEMA's envelope logic
  (envelope / bare-route / legacy-array forms).
- Regression fixtures: rest response headers named `uri`, `to`,
  `endpoints` carrying URI-shaped values emit no endpoints; the same
  documents' operation-level `to:` and `steps[].to:` URIs remain
  validated (bd acceptance). Cross-document isolation fixture (a
  poisoned-headers document must not affect another document's lint).
- `camel-lint/CONTEXT.md` walk section updated to describe
  context-scoped traversal.

Affected crates: camel-lint only. Explicitly excluded: R-SCHEMA rule
logic (already path-scoped via jsonschema), schema regeneration
(route-schema.json unchanged), runtime lowering, `URI_KEYS` membership.

## Acceptance criteria

- A rest document whose `response.headers` contains `uri`, `to`, or
  `endpoints` keys with URI-shaped values produces ZERO R-URI-known
  diagnostics from those header values.
- The same document's operation-level `to:` and nested `steps[].to:`
  URIs still receive full URI validation (unknown option etc.).
- `security_policy.config` free-form maps are opaque to endpoint
  detection.
- All existing camel-lint tests and the camel-cli `lint_corpus` baseline
  pass unchanged (no legitimate endpoint is lost).
- No per-document mutable global state: linting a poisoned document
  then a clean document yields identical diagnostics to linting the
  clean document alone.

## Risk budget

Risk: endpoint LOSS from over-strict context matching (false negatives).
Mitigation: undeclared-key legacy fallback preserves today's tolerance
for schema-invalid shapes; full camel-lint suite + corpus baseline gate
the change. Acceptable: none of the existing corpus diagnostics change.
Out of bounds: any change to emitted diagnostics for schema-valid
documents other than the reported header/config false positives.
