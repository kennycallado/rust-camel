# Proposal: add-template-relative-root

## Why

`template:file:///<abs-path>` requires an absolute path in every route. Operators
embedding routes in repos work around it with `${env:CAMEL_ROOT}`, which leaks
environment plumbing into route declarations. A configured project-relative root
(`[components.template] root`) lets routes say `template:file:templates/page.html`
and stay portable across checkouts.

bd: rc-6onwb (P2). Source: camel-astro report, e_opus-validated.

This is NOT a traversal widening (ADR-0047): a relative URI resolves only inside
the configured root, and every existing no-escape rule (`..` segments, symlink
escape, absolute escape, closure confinement) keeps enforced.

## What Changes

Affected crates: `camel-template` (config, uri, component, endpoint, lifecycle,
docs). No API break: the absolute `template:file:///...` form is untouched.

- Add `[components.template] root = "<path>"` (kebab-case,
  `deny_unknown_fields` preserved). Absolute or CWD-relative.
- Accept one canonical relative entry form: `template:file:<rel-path>` (no
  leading `./`, no `.`/`..` components). Documented as canonical; other
  relative spellings are config errors.
- When `root` is configured, the canonicalized root anchors every template
  endpoint: relative entries resolve against it, absolute entries must land
  inside it. Root and entry are canonicalized; missing root/entry, `..`
  segments, symlink escapes, and absolute escapes fail with clear errors.
- No `root` configured: current behavior only — absolute URIs anchor at the
  entry's parent; a relative URI is a `CamelError::Config` error.
- Docs: component README + `docs/src/components/template.md`.

Explicitly excluded (non-goals per mission 461): dev watcher/reload
(rc-hasr3), render diagnostics/filters (rc-a4xmx, rc-6ckmb), inline minijinja,
HTTP static (459), any change to the no-escape/autoescape contract.

## Acceptance criteria

- `cargo test -p camel-template` green, including the seven mission-mandated
  scenarios (relative renders; absolute still works; `..` rejected; symlink
  escape rejected; absolute escape rejected; relative-without-root rejected;
  include outside root rejected).
- `cargo fmt --check --all`; clippy `-p camel-template --all-targets
  -D warnings`; xtask lints; docs build for touched docs;
  `openspec validate --all --strict`.
- Delta spec under `openspec/changes/add-template-relative-root/specs/`
  validates strictly.

## Risk budget

Acceptable: new config field + new accepted URI form (both additive);
lifecycle root-anchoring refactor (internal `pub(crate)` surface only).
Out of bounds: weakening openat/O_NOFOLLOW confinement, render-path changes,
autoescape semantics, schema-breaking config changes.
