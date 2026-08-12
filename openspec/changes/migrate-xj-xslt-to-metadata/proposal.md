# Proposal: migrate-xj-xslt-to-metadata

## Why

`camel-xj` and `camel-xslt` are the only two components in the repo that accept
an open-ended `param.<name>=<value>` URI namespace. They were recorded as
`schema-blocked-deferred` in the canonical `endpoint-metadata-derivation` spec
because the exact-match `UriOption` model could not express an open namespace.

Change `open-uri-param-namespace` (squash-merged `e27a77bb`, specs archived
`3751cac7`) landed the `#[uri_param(pattern = "param.")]` capability. The
`schema-blocked-deferred` disposition is now stale. This change makes xj/xslt
publish rich URI metadata so the lint, catalog, schema-gen, and LSP can see
their options — closing the loop on the two-phase split.

Bd: `rc-xf7y` (discovered-from: `rc-2s7g`).

## What Changes

**Included:**
- New `metadata.rs` module in `camel-xj` and `camel-xslt` with a
  `skip_impl` metadata-only descriptor (`#[derive(UriConfig)]`), declaring all
  URI query options including `#[uri_param(pattern = "param.")]` for the open
  stylesheet-parameter namespace.
- `Component::metadata()` override on `XjComponent` and `XsltComponent` so the
  descriptor reaches the catalog (the metadata-only struct publishes nothing by
  itself — the override is the wiring step).
- Parity tests per descriptor (option names + `pattern` shape) and a catalog
  integration test asserting `get_metadata("xj")` / `get_metadata("xslt")`
  returns non-empty `uri_options` through the override.
- Example/corpus audit: `examples/xj-example/` and `examples/xslt-example/` +
  `lint_corpus` gate confirming zero new diagnostics after metadata lands.
- Schema-snapshot regeneration (`cargo xtask schema --check`) so the
  `schema-check` quality gate stays green.
- Canonical spec update: MODIFIED "xj/xslt recorded as schema-blocked-deferred"
  scenario → "schema-published", plus a new scenario asserting the `param.*`
  pattern option resolves via prefix.
- Drive-by fix: `camel-lint/src/route_view.rs::endpoints()` now parses `from:`
  URI query options (was `Vec::new()`), making `from:`/`to:` symmetric. This
  was necessary because the xj fixture's required `direction` param lives in
  the `from:` URI. The fix newly surfaces `missing-required-option` diagnostics
  on 3 pre-existing routes (jms/soap/master) — baselined as unresolved under
  investigation (Bd: rc-9qjq).

**Excluded:**
- Runtime parser changes — `XjEndpointConfig::from_uri` and
  `XsltEndpointConfig::from_uri` stay 100% hand-rolled and unchanged.
- Enum-value validation for `direction` — the metadata model has no
  value-set constraint; `OptionKind::String` is the ceiling. Out of scope.
- `transformDirection` / `resourceUri` rejection modeling — these are absent
  from the catalog, so the lint emits `UnknownOption`, which already aligns
  with the runtime hard-rejection. No new concept needed.
- Any change to `openspec/specs/xj-runtime-affinity/` (tokio runtime affinity,
  unrelated to metadata).

## Acceptance criteria

- `cargo test -p camel-xj -p camel-xslt` passes including new parity + catalog
  integration tests. Parity tests assert that `usize`/`u32`/`u64` fields derive
  `OptionKind::Int` (not `String`), preventing silent kind-inference regressions.
- `get_metadata("xj")` and `get_metadata("xslt")` return non-empty
  `uri_options` containing a `param` option with
  `pattern = Some(Prefix { separator: "param." })`.
- `cargo test -p camel-cli --test lint_corpus` passes. The 3 diagnostics on
  jms/soap/master routes are newly surfaced by the `from:`-option fix (not
  pre-existing), baselined as unresolved under investigation (Bd: rc-9qjq).
- `cargo xtask schema --check` passes with a regenerated snapshot.
- `openspec validate migrate-xj-xslt-to-metadata --type change --json` exits
  with `"valid": true` and zero issues.
- No LSP golden/snapshot test hardcodes xj or xslt as "minimal scheme, no
  options" — verified by `rg 'xj|xslt' crates/camel-lsp/` (if any golden test
  exists, update it; if none, this criterion is vacuously satisfied).

## Risk budget

**Acceptable:** lint behavior change on xj/xslt routes — previously silent
(minimal scheme), now validates known options. This is the intended behavior;
if a real route uses an option the descriptor does not list, that is a
legitimate finding to fix in the descriptor, not a regression.

**Out of bounds:** any modification to the runtime parser, the macro, or the
resolver. Any change that touches `xj-runtime-affinity` semantics.
