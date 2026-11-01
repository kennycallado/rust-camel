# Proposal: 476-template-proto-dev-reload

## Why

`camel run --watch` reloads route files but not the assets those routes compile
against. `RuntimeCommand::ReloadTemplates { route_id }` exists and the
`RuntimeBus` intercepts it (`TemplateReloadRegistry::reload_route`), yet no
user-facing path triggers it. An operator who edits a template or a `.proto`
descriptor keeps the old rendered output or the old descriptor until a full
restart. Dev-mode iteration on `camel-astro` 0.62.0 exposed this gap (bd
`rc-hasr3`).

## What Changes

- `camel run --watch` only: derive watch targets from committed per-route
  reload-target metadata — template dependency-closure files (entry and
  includes, nested) and validator protobuf descriptor sources (the schema
  file plus its resolved local imports). No arbitrary files or directories.
- On a template or proto change, after the existing debounce, the watcher
  dispatches `RuntimeCommand::ReloadTemplates { route_id }` for each affected
  route through the runtime execution handle. Routes are not restarted,
  `RouteStatus` does not change, no journal events are appended, and no full
  route reload runs for a template-only or proto-only change.
- If route files and template/proto files change in the same debounce batch,
  the existing route reload runs first; template/proto reloads still dispatch
  for affected routes the route pass did not rebuild.
- Watched files that are deleted and recreated (editor atomic saves) are
  re-attached so later edits keep firing.
- Template and validator reload targets expose their source files (unique
  closure file paths, root directory excluded); the validator protobuf
  endpoint holds its descriptor behind an atomic swap and recompiles it on
  reload, with a bounded deadline and blocking-compile isolation.
- Atomic swap and fail-closed semantics are preserved: a failed recompilation
  keeps the prior compiled set, logs a redacted error, and leaves the route
  running. Reload dispatches are bounded and cancellable with no late commit;
  the watcher loop never blocks on them.
- Docs: CLI `--watch`, template hot-reload, proto dev workflow.

Excluded: production watch mode; new admin or CLI trigger channels; network
exposure of reload; changes to route-file reload semantics; restarts on
template edits; watching arbitrary files; **gRPC live descriptor reload** —
gRPC endpoints retain descriptors resolved at startup, and live refresh for
them is out of scope (deferral ledger, follow-up ticket; non-goal per master
ruling 2026-11-01).

## Acceptance criteria

- Editing a template changes rendered output without a route restart or
  status change.
- Editing a nested include triggers the reload.
- A validator proto descriptor edit (schema or local import) is observed and
  recompiled for the affected route.
- Rapid edits coalesce under the existing debounce.
- A failed template reload keeps the previous output and the route alive.
- Non-watch mode and compiled artifacts never watch.
- Watch targets stay limited to declared closures and descriptor sources.

## Risk budget

Acceptable: additive watcher changes in camel-core reload_watcher, a trait
extension in camel-component-api, a validator endpoint retrofit with atomic
swap. Out of bounds: touching route-reload semantics, journaling, or
`RouteStatus` transitions; any watch behavior outside `camel run --watch`;
gRPC endpoint changes.

Affected crates: camel-core, camel-cli, camel-component-api, camel-template,
camel-validator, camel-proto-compiler (public import-closure helper), docs.
