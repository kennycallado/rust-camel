# Design: scriptfile

## Approach

**Load-time resolution; the declarative contract stays inline.**
`LanguageExpressionDef` remains `{ language, source }`. `source_file` exists
only in the DSL layer and resolves to inline source during parse/load, so all
~100 existing struct-literal construction sites keep compiling and the
canonical schema for the def is unchanged.

Resolution helpers live in **camel-api** (single source of truth, used by
camel-dsl and the Rust DSL alike):

- `infer_script_language(path) -> Result<String, CamelError>` — extension map
  `.rhai→rhai`, `.js→js`, `.jsonpath→jsonpath`, `.xpath→xpath`,
  `.jinja`/`.jinja2→minijinja`.
- `read_script_source(path) -> Result<String, CamelError>` — one
  `read_to_string`; IO failures and trimmed-empty payloads are
  `CamelError::RouteError`/`Io(String)` messages naming the **resolved** path.
- `impl LanguageExpressionDef { fn file(path) -> Result<Self, CamelError>;
  fn file_with_language(path, language) -> ... }` — relative paths use normal
  std CWD semantics.

**camel-dsl.** The route AST structs gain `source_file: Option<String>`
(`ScriptData`, `PredicateBlock`, `LoopWhileExpr`, the value-source AST
struct); `ScriptData.language`/`.source` narrow to `Option<String>` so the
file form can omit them. `parse_language_expression` and
`parse_value_source` count `source_file` as an alternative (it pairs with
optional `language`; `source` + `source_file` together is a named error —
including the resolved path — raised before any file IO) and perform
resolution through a small `ScriptLoadContext { base_dir, sink }` threaded
from the parse entry points: `base_dir` is the route file's parent for
`load_from_file`/`load_json_from_file`/discovery, `None` (CWD semantics)
for string parsing. The sink records each resolved `(path, content)` so the
document's `source_hash` — currently computed from raw YAML before parse —
is finalized **after** parse as `raw_content + script contents`, and the
paths feed the watch registry. Template materialization is in scope: the
expression's originating file context is carried through
`template/materializer.rs`, `source_file` resolves **after** parameter
substitution against the template document's directory, the resulting
dependencies register like normal ones, and their contents fold into the
instance hash (`discovery.rs` instance-hash seam). Script file contents are
read verbatim: env interpolation already happened on the raw document (so
the *path value* may use `${env:}`), but file *contents* are never
interpolated — this matches the existing minijinja byte-identical
passthrough rule (`template/materializer.rs` checks
`language == "minijinja"`, which inference produces unchanged).

**camel-dsl watch registry.** New `ScriptWatchRegistry` (global, in
camel-dsl): `route_id → BTreeSet<PathBuf>`. Discovery **stages** each
route's script dependencies; publication follows the runtime outcome of the
reload pass, not discovery success: successfully added, restarted, or
swapped routes publish their staged replacements, failed or skipped routes
keep their previous dependencies, and routes removed by a committed reload
have their dependencies deleted. `load_from_file`/`load_json_from_file`
merge their paths in.

**camel-core watcher.** `DevTargetMap` sources become kind-tagged
(`Template` | `Script`). `classify_event` gains `EventClass::ScriptSource`
for script paths; a batch containing a script event runs the existing full
`reload_pass` (re-discovery → hash diff), so a script-only edit swaps the
route instead of dispatching `ReloadTemplates` (which does nothing for a
route with no template target). Batched script+route edits collapse into one
pass as today.

**camel-cli.** `dev_target_sources()` merges `TemplateReloadRegistry` and
`ScriptWatchRegistry` sources with kinds. The `basic` template gains
`scripts/hello.rhai` at project root, referenced from `routes/hello.yaml`
via `source_file: ../scripts/hello.rhai` (relative to the route file's
directory) with inferred language (`lang-rhai` ships in `flavor-slim`, which
`flavor-regular` — the CLI default — chains).

**Contract + docs.** `cargo xtask schema` regenerates the DSL schema and TS
types (canonical spec unchanged); parity harness gains an optional fixture
hook (`{scripts_dir}` substitution + fixture files) so a `ParityCase` covers
`source_file` in both formats with absolute paths; docs script pages gain the
file form + IDE sentence.

## Affected crates

- camel-api: inference/reading helpers, `file()` constructors.
- camel-dsl: AST fields, expression parsers, resolution context, hash
  folding, `ScriptWatchRegistry`.
- camel-core: watcher event classification + reload trigger for script
  sources.
- camel-cli: `dev_target_sources` merge, `basic` template script file.
- schemas/ts: regenerated.
- docs: script guide pages.

## Architecture boundaries

Runtime (camel-core) learns only "these paths are script sources" — it never
reads script files or infers languages. DSL owns resolution and the registry.
Languages are untouched: resolved scripts enter the existing
`LanguageExpressionDef` → `language_registry` pipeline. Components are
untouched.

## API decision

`LanguageExpressionDef` unchanged → the stable declarative contract is
**non-breaking**. The public `camel_dsl::route_ast` structs DO change shape
(`ScriptData` fields narrow to `Option`; new `source_file` fields) —
breaking for exhaustive struct literals, covered by ADR-0090 (pre-1.0);
migration is mechanical per literal: wrap existing `language`/`source`
values in `Some(...)` AND supply `source_file: None` explicitly (or wrap
values and extend the literal with `..Default::default()` once `Default` is
derived). The squash commit subject uses `feat(dsl)!:`.
