# Design: namebloat

## Approach

Replace the global-name dispatch in `document.rs::walk` with a
schema-CONTEXT-carrying traversal. Facts established by audit:

- `walk` currently matches every mapping key against `URI_KEYS`
  (hardcoded allowlist) and `CONTAINER_KEYS` (global set derived from
  the WHOLE embedded schema). `is_container` classifies ANY
  `type: object` as a container, including free-form maps
  (`RouteDslRestResponse.headers`, `RouteDslSecurityPolicy.config`,
  the various `parameters` maps — the latter already name-skipped).
- Walking into `headers` lets user header names collide with
  `URI_KEYS` → header values emitted as endpoints → R-URI-known FPs.
- `RouteDslStep` is an `anyOf` of 43 step defs; `RouteDslRestOperation`
  DECLARES `to` (string|null) and `steps` — so context-scoped lookup
  keeps rest `to:` validation working by declaration, not by global
  name coincidence.
- Schema permissiveness facts: every structured def carries
  `additionalProperties: false`; the ROOT envelope and the pure
  composition wrappers (`RouteDslStep`, `EnrichBody`, …) carry NO
  `additionalProperties` (JSON-Schema-default permissive).

New mechanics (all in `document.rs`):

1. **Context type**: `walk` gains a `ctx: &[&serde_json::Value]`
   parameter — the candidate subschemas describing the CURRENT node
   (`$ref`-resolved at use; `anyOf`/`oneOf`/`allOf` expanded lazily
   during property lookup). Cycle guard is an ACTIVE-CHAIN stack of
   `$ref` strings — pushed on entry, popped on exit, fresh per lookup
   — NOT a traversal-wide visited set: a def reachable through two
   sibling branches (or twice in one document path, e.g.
   `RouteDslStep → DoTryData.steps → RouteDslStep`) must expand BOTH
   times; only a ref already on the current chain (true cycle) is cut.
2. **Root context** mirrors R-SCHEMA's envelope detection: root mapping
   with `routes`/`rest`/`mcp` → envelope schema; other root mapping →
   `RouteDslRoute` (bare-route); root sequence → `RouteDslRoute` per
   item (legacy array form).
3. **Per-key dispatch** (after the existing `parameters` consumption,
   `mcp` skip, and first-wins `from` slot capture — all unchanged).
   First resolve the key against ctx:
   - DECLARED (some candidate declares `k` in `properties`) → dispatch
     on that property's own subschema:
     - `URI_KEYS` member: scalar/sequence-of-scalars child →
       `emit_endpoints` (unchanged); Mapping child → object-form
       recursion carrying inherited params with `ConfigParameters`
       origin (today's `enrich: {uri: ...}` behavior);
     - structured container (object WITH declared properties, or
       array of object-ish items) → Branch recursion, ctx from the
       property's subschema; a Sequence child walks items against the
       items-subschemas, or the same ctx when the declared shape is an
       object (tolerates `multicast:` direct-sequence form);
     - free-form (object with NO declared properties — an
       `additionalProperties` map) → OPAQUE LEAF: no recursion, no
       endpoint emission. **Core fix.**
     - scalar/other leaf → ignore.
   - UNDECLARED but PERMITTED — some candidate accepts unknown keys
     (`additionalProperties` absent, `true`, or a schema at that
     candidate) → the key is legitimate USER DATA: if
     `additionalProperties` is a typed schema, dispatch follows THAT
     schema; otherwise (absent/`true`) the key is opaque — NO global
     name interpretation. This closes the permissive-root hole: the
     root envelope has no `additionalProperties`, so e.g. a stray
     root-level `response:` key never reaches `CONTAINER_KEYS`.
   - UNDECLARED and REJECTED (every candidate carries
     `additionalProperties: false`) → schema-INVALID key → legacy
     global fallback (`URI_KEYS` emission / `CONTAINER_KEYS`
     recursion) preserving today's tolerance for malformed docs.
     `is_container` is tightened to exclude free-form maps so the
     fallback set cannot re-open the hole.
4. **`CONTAINER_KEYS` static** survives ONLY as that fallback set
   (free-form-excluded); it is no longer the primary dispatch.

R-SCHEMA needs no change (real jsonschema validation is already
path-scoped). `route-schema.json` unchanged. `URI_KEYS` unchanged.

## Affected crates

- camel-lint: `src/document.rs` (walk + schema-context helpers +
  tests), `src/rules/ruriknown_tests.rs` (rule-level regression),
  `CONTEXT.md` (walk section).

## Architecture boundaries

camel-lint stays runtime-free (hexagonal gate: no camel-core/camel-dsl
deps). The schema context reads the already-embedded `ROUTE_SCHEMA`
constant — no new dependency, no schema regen. Boundary unchanged:
lint layer only.

## Phases

Single-phase: one coherent fix + fixtures, ~3 tasks.

## Test plan

- document.rs unit tests: headers `uri`/`to`/`endpoints` emit no
  endpoints; `security_policy.config` opaque; operation `to:` +
  `steps[].to:` still captured; `multicast:` direct-sequence form
  still captured (rejected-key fallback); permissive-root stray keys
  (e.g. root-level `response:`) opaque; recursive composition
  (do_try-in-do_try, choice/multicast nesting) captures nested
  `steps[].to`; ROOT-FORM MATRIX — envelope, bare-route, and
  legacy-array documents each capture nested `steps[].to:` (not just
  `from`).
- ruriknown_tests.rs: bd repro — `response.headers.uri:
  timer:foo?frequency=1s` yields zero R-URI-known diagnostics while
  the same document's operation `to: timer:foo?frequency=1s` yields
  the `frequency` UnknownOption; cross-document isolation (lint
  poisoned doc then clean doc — identical diagnostics to clean-only).
- Gates: fmt, clippy -p camel-lint -D warnings, camel-lint tests,
  camel-cli lint_corpus (baseline expected unchanged).
