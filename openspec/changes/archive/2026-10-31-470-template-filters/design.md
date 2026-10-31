# Design: 470-template-filters

## Approach

Register the two allowlisted filters where the component already builds its
environment: `TemplateSet::compile` in
`crates/components/camel-template/src/template_set.rs` (currently lines
131–134 create the environment and set strict-undefined, fuel, and recursion
limits). A new private helper `register_allowlisted_filters(&mut env)` adds
exactly `truncate` and `dump` via `minijinja::Environment::add_filter`.

The contract is about component-owned registrations: MiniJinja 2.x
`Environment::new()` already resolves the engine's built-in filters (including
`safe` and `tojson`), and those stay untouched. The component adds nothing
else — no additional filters, no functions, no globals. This also keeps the
existing `|safe` migration guidance meaningful.

- `truncate(value, length, end: Option<String>)`: accept the length as a
  `Value` and validate it — only a non-negative integer renders; negative,
  non-integral, boolean, and non-numeric lengths return a filter error
  (surfaces as a render error through the existing
  `TemplateReloadError::Render` path; the producer's log-error classifier is
  unchanged and needs no new class). Truncate on Unicode scalar values using
  `str::chars()` so multi-byte characters are never split. Append the end
  marker (default `"…"`) only when the string actually truncates. The filter
  returns a plain string `Value`; HTML awareness stays with autoescaping.
- `dump(value)`: serialize with `serde_json::to_string(&value)` — MiniJinja
  2.24 `Value` implements `Serialize` and the engine's own `tojson` filter
  uses the same call. (There is no `Value::to_json()` method on this locked
  version.) This adds `serde_json` to `camel-template`'s regular
  dependencies. Determinism: minijinja native maps are `BTreeMap`-backed
  without `preserve_order`, so equal inputs serialize with sorted keys
  byte-identically. Serialization failure returns an error, which the engine
  maps to a render error; genuinely unserializable values exist (e.g. a map
  with a non-string, sequence-valued key). The result is ordinary template
  text — it must NOT
  reuse `tojson`'s safe-string marking, so dump output stays subject to the
  active escape mode.
The escaping policy is a no-code-change decision: the built-in escaper stays,
including `/` → `&#x2f;`. Docs record the behavior and the migration options.

## Affected crates

- `camel-template`: filter registration + unit tests in `template_set.rs`;
  README/CONTEXT.md doc updates.
- Docs (mdBook): `docs/src/components/template.md`,
  `docs/src/languages/minijinja.md`.

## Architecture boundaries

- **Components**: the allowlist lives inside the external template component's
  compile step (control plane). Render-time filter failures surface through
  the existing data-plane render error path (`TemplateReloadError::Render`);
  no new error surface, and the producer's log-error classifier is unchanged.
- **Languages**: `camel-language-minijinja` (the inline
  `language: minijinja` engine) is a different code path and gains nothing;
  its registration surface and behavior are unchanged. The component's
  compiled environment is built solely in `camel-template`'s
  `template_set.rs`, so the two paths share no environment-construction code.
- **Functions**: no host functions are exposed; the filters close over no
  host state.
- Data/control plane split, confinement, zero-override policy, limits, and
  reload atomicity are untouched.

## Alternatives considered

- Keep the environment without additional component-owned registrations and
  only document migration — rejected: the
  mission decides for the narrow allowlist; `truncate`/`dump` are the two
  documented Nunjucks filters the port path needs, and behavior beats a docs-
  only answer.
- Register minijinja's full builtin filter set — rejected: the mission fixes
  the component-owned allowlist at two; engine built-ins stay because they
  are part of the locked engine, not a component decision.
- Custom escaper disabling `/` escaping — rejected outright: weakens
  autoescaping; the mission forbids it.
- `tojson` instead of `dump` — rejected: Eleventy/Nunjucks parity name is
  `dump`; and `dump` must not inherit `tojson`'s safe-string marking, so it
  is a separate registration, not an alias.

Bd: rc-6ckmb
