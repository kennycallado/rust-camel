# Design: template-autoescape-default

bd: rc-6mwze

## Approach

### Decision 1 — mechanism: wrap the entry source, not a callback

When the entry template contains NO `{% autoescape %}`-family statement at all
AND `[components.template] default-autoescape` is configured, `TemplateSet::compile`
compiles the entry source wrapped in `{% autoescape "<mode>" %}` …
`{% endautoescape %}` with no added whitespace. Rejected alternative:
`Environment::set_auto_escape_callback` — it is name-based and environment-wide,
so it would also change include targets, breaking "include targets keep their
own semantics", and it could not express "explicit block wins" without extra
detection logic. With the wrap mechanism the compiled entry is byte-identical to
what the operator would produce by writing the block themselves, so include
behavior is unchanged by construction and a valid explicit block wins trivially
(no wrap).

### Decision 2 — eligibility and precedence: explicit statement > config > reject

`validate_autoescape_wrapper` (reused from `camel-language-minijinja`, unchanged)
distinguishes absence from misuse (`MissingWrapper`, `InvalidMode`,
`NotEnclosingWholeTemplate`, `DuplicateWrapper`, `MalformedSyntax`). The config
default is eligible ONLY for true absence. Compile order in `TemplateSet::compile`:

1. Entry present and UTF-8 (unchanged).
2. Pre-scan the entry source (new, camel-template-local): does it contain any
   `{% autoescape … %}` or `{% endautoescape %}` statement block?
   - Statement present → run `validate_autoescape_wrapper` exactly as today:
     pass = the explicit block wins (compile as-is); any failure = `Compile`
     error. The config NEVER repairs an invalid mode, a partial block, a
     duplicate, a stray `endautoescape`, or malformed syntax — wrapping such a
     source would either move the failure to render time (breaking
     compile-before-activation) or silently alter the operator's declared mode.
   - No statement at all → configured mode present: wrap with the configured
     mode and compile (a parse failure of the wrapped source still fails the
     whole set — fail-closed preserved); configured mode absent: today's
     `Compile` error (unchanged fail-closed).

The pre-scan is a conservative statement-block scan: for each `{% … %}`
statement block, strip any leading/trailing whitespace-control markers
(`-` and `+`) from the tag content, then match when the trimmed remainder
starts with `autoescape` / `endautoescape` — this catches `{%- autoescape
"htm" -%}` too, whose failure would otherwise only surface at render time.
It may over-count (e.g. an `autoescape` mention inside a `{% comment %}`
block counts as present) — that is safe: an over-count only skips the config
default and falls through to today's validator path, which stays fail-closed.
It never under-counts a real statement block. Nested `{% autoescape %}` blocks
inside the entry remain legal (inner block overrides the injected outer block,
standard MiniJinja scoping).

### Decision 3 — config type and validation

New public type in `crates/components/camel-template/src/config.rs`:
`pub enum AutoescapeMode { Html, Json, None }` with a parse function returning
a plain error string; `config.rs` owns it (the language crate is not touched).
`TemplateBundleConfig` (bundle.rs) gains `default_autoescape: Option<toml::Value>`
(`#[serde(default)]`, kebab-case handles the key name). `TemplateBundle::from_toml`
extracts `toml::Value::String` and requires an EXACT lowercase match against
`html` | `json` | `none`; any other value — unknown string, wrong type
(`true`, `1`, array) — becomes `CamelError::Config` naming the key and the
allowed values, e.g. `` `components.template.default-autoescape` must be one
of "html", "json", "none"; got "yaml" `` (non-strings report the TOML value).
A bare `Option<String>` serde field would let a wrong type die as a generic
serde error without the allowed values, hence `toml::Value` + manual parse.
Absent key → `None`.

### Decision 4 — threading: additive builder, public API unchanged

`TemplateComponent::new(…)` keeps its exact current signature — it is public
API consumed by `examples/template-basic/src/main.rs:37`. The component gains a
private `default_autoescape: Option<AutoescapeMode>` field (default `None`) and
a public `#[must_use] fn with_default_autoescape(self, mode: AutoescapeMode)
-> Self` builder. `TemplateBundle::register_all` calls the builder only when
the key is present. The value flows crate-internally: `component` → `endpoint`
→ `StartupBuildHandle` (lifecycle startup compile) and → `ReloadHandler`
(reload build compile). `TemplateSet::compile` gains a
`default_autoescape: Option<AutoescapeMode>` parameter; `producer.rs`'s test
helper passes `None`. The value is component-level config: static per component
instance, identical for startup and reload builds.

### Decision 5 — includes: environment callback untouched

The wrapper contract applies to the entry only; `validate_autoescape_wrapper`
stays entry-only. Concrete semantics: the config never changes
`Environment::new()`'s default name-based auto-escape callback
(`template_set.rs:102`), so an include target without its own block keeps
exactly the behavior it has today when included from a wrapped entry, and an
include target with its own explicit block keeps escaping by its own block.
`{% extends %}` parent-body semantics are likewise unchanged. No new include
behavior is introduced.

### Decision 6 — ADR-0047 amendment (docs)

ADR-0047 §"Escape-mode contract" currently reads "each rendering expression
must explicitly select its output context with a top-level MiniJinja
`autoescape` block". Add a dated amendment paragraph (2026-10-29, rc-6mwze):
for the external `template:` component, an operator-configured
`default-autoescape` counts as that explicit selection; it is valid for the
external component only; inline `language: minijinja` keeps the mandatory
block; the absent key still rejects. The docs page and crate README carry the
operator-facing detail (key, precedence, security note).

## Affected crates

- `camel-template`: config type + key, bundle parsing, compile-path
  eligibility scan + wrap, threading via additive builder, unit +
  integration tests.
- `camel-language-minijinja`: NO code change (`validate_autoescape_wrapper`
  reused as-is; inline semantics unchanged).
- `camel-cli` / examples: no change (`new` signature preserved).
- Docs: `docs/src/components/template.md`, crate README, ADR-0047 amendment.

## Architecture boundaries

Components-layer change only. The Languages layer keeps its mandatory-explicit
contract for inline sources; the Runtime/DSL layers are untouched; no new
cross-crate dependency. Authority: ADR-0047 §"Escape-mode contract" and
§"Phase 2: external-template Component".

## Test geometry

Unit tests on `TemplateSet::compile` (html escapes `<`; none preserves; explicit
json block overrides html config with observable quoted-literal output; absent-config
rejection kept; invalid explicit mode `{% autoescape "htm" %}` still rejects
with config set; whitespace-control variant `{%- autoescape "htm" -%}` still
rejects with config set; stray `endautoescape` still rejects) and on
`TemplateBundle::from_toml` (each value parses; unknown string, uppercase, and
non-string values all reject with the key + allowed values in the message;
absent key → `None`). One end-to-end test in
`tests/template_render_integration.rs`: config "html" + block-less entry file +
`<` payload renders escaped output (proves the bundle → builder → component →
compile wiring). Existing language-crate tests already pin inline rejection of a
missing block; verified, not modified. No sleeps anywhere.
