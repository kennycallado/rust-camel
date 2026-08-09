# Tasks: consolidate-uri-metadata

> **Migration delegation convention (applies to ALL migration tasks 1.6, 2.1–2.6, 3a.1–3a.4, 3b.1–3b.3):**
> The macro generates inherent `fn uri_options()` and (when opted in) `fn metadata()` on
> the *config* struct. The *component* struct implements `Component`, whose `metadata()`
> default returns `minimal(scheme)` (empty options). Every migration task MUST wire the
> component's `Component::metadata()` to delegate:
> `fn metadata(&self) -> ComponentMetadata { ConfigType::metadata() }`
> — or, if the component has no opt-in but the config does:
> `ComponentMetadata::minimal(scheme).with_uri_options(ConfigType::uri_options())`.
> Without this delegation step, the catalog returns empty options.

## Phase 1: Macro extension + reference migration (sql)

### camel-endpoint-macros

#### Task 1.1: Extend UriParamAttr to parse semantic keys

**Files:**
- `crates/camel-endpoint-macros/src/uri_config.rs` (modified)

**Steps:**
1. Add fields to `UriParamAttr` struct (L9-15): `desc: Option<String>`, `required: bool`, `secret: bool`, `deprecated: Option<String>`, `aliases: Vec<String>`, `kind_override: Option<String>`.
2. Restructure the parser: the current `Punctuated<KeyValue, Token![,]>` forces every element to be `key = value`. Replace with a custom parse loop that accepts EITHER a bare ident (flag form: `required`, `secret`) OR a `key = value` pair. For `aliases`, the value is an `ExprArray` of string literals (not a `Lit`), so the value type must accommodate both `Lit` and `ExprArray` — use an enum `AttrValue { Lit(Lit), Array(Vec<String>) }` or a separate parse path.
3. In the `match key_str` block, add: `"desc"` (Lit::Str), `"required"` (flag or `Lit::Bool`), `"secret"` (flag or `Lit::Bool`), `"deprecated"` (Lit::Str), `"aliases"` (ExprArray → `Vec<String>`), `"kind"` (Lit::Str). Keep the catch-all `_ =>` arm returning `syn::Error::new_spanned(pair.key, format!("unknown attribute key: {}", key_str))` for truly-unknown keys.
4. When `input.is_empty()` (bare `#[uri_param]`), all flags default to `false` and all options to `None`.

**Tests:**
- `parse_secret_flag`: `#[uri_param(secret)]` → `UriParamAttr.secret == true`
- `parse_secret_with_other_keys`: `#[uri_param(secret, default = "x")]` → `secret == true && default == Some("x")` (mixed flag+keyvalue)
- `parse_deprecated_key`: `#[uri_param(deprecated = "old")]` → `deprecated == Some("old")`
- `parse_aliases_array`: `#[uri_param(aliases = ["a","b"])]` → `aliases == vec!["a","b"]`
- `parse_unknown_key_still_errors`: `#[uri_param(bogus = 1)]` → `syn::Error` containing "unknown attribute key"
- **command**: `cargo test -p camel-endpoint-macros -- parse_`
- **expected**: all pass after implementation; unknown-key test unchanged (regression guard)

**Acceptance:**
- `cargo test -p camel-endpoint-macros` green
- `cargo clippy -p camel-endpoint-macros -- -D warnings` clean

- [ ] 1.1

#### Task 1.2: Implement OptionKind type inference

**Files:**
- `crates/camel-endpoint-macros/src/uri_config.rs` (modified)

**Steps:**
1. Add a `fn infer_option_kind(ty: &syn::Type) -> TokenStream` helper that maps Rust types to `OptionKind` constructor calls, using existing `get_type_name` (L166), `is_duration_type` (L177), `is_option_type` (L190):
   - `Duration` → `OptionKind::Duration`
   - `bool` → `OptionKind::Bool`
   - `u8`..`u64`/`usize`/`i8`..`i64`/`isize` → `OptionKind::Int`
   - `f32`/`f64` → `OptionKind::Float`
   - `String`/`&str` → `OptionKind::String`
   - `Vec<T>` → `OptionKind::List(Box::new(infer_option_kind(inner)))`
   - anything else → `OptionKind::String` (NEVER `Enum`)
2. Unwrap `Option<T>` to inner `T` before inference (reuse `is_option_type`).
3. When `kind_override` is present, parse it: `"duration"` → `Duration`, `"bool"` → `Bool`, `"int"` → `Int`, `"string"` → `String`, `"float"` → `Float`, `"enum:A,B,C"` → `Enum(vec!["A","B","C"])`. Reject unrecognized kind strings with a spanned `syn::Error`.

**Tests:**
- `infer_bool`: field `active: bool` → `OptionKind::Bool`
- `infer_duration`: field `timeout: Duration` → `OptionKind::Duration`
- `infer_string`: field `name: String` → `OptionKind::String`
- `infer_option_inner_kind`: field `val: Option<u32>` → `OptionKind::Int` (unwrapped)
- `infer_option_required_false`: field `val: Option<String>` with no `required` attr → generated `UriOption.required == false`
- `infer_vec_string`: field `tags: Vec<String>` → `OptionKind::List(Box::new(OptionKind::String))`
- `infer_enum_is_string`: field `mode: SomeEnum` → `OptionKind::String` (NOT Enum)
- `kind_override_enum`: `#[uri_param(kind = "enum:A,B")]` → `OptionKind::Enum(vec!["A","B"])`
- `kind_typo_errors`: `#[uri_param(kind = "duraton")]` → compile error
- **command**: `cargo test -p camel-endpoint-macros -- infer_ kind_override`
- **expected**: all pass after implementation

**Acceptance:**
- `cargo test -p camel-endpoint-macros -- infer_ kind_override` green
- Inference never produces `OptionKind::Enum` without explicit `kind` override

- [ ] 1.2

#### Task 1.3: Generate fn uri_options() helper + trybuild infra

**Files:**
- `crates/camel-endpoint-macros/src/uri_config.rs` (modified)
- `crates/camel-endpoint-macros/tests/ui/` (new directory)
- `crates/camel-endpoint-macros/Cargo.toml` (modified — add trybuild dev-dep)

**Steps:**
1. Add `trybuild = "1"` to `[dev-dependencies]` in `Cargo.toml`.
2. Create `tests/ui/` directory for compile-fail tests.
3. In `impl_uri_config` (L494), after the existing `from_uri`/`parse_uri_components` generation, add a generated inherent fn `pub fn uri_options() -> Vec<UriOption>`.
4. **Path resolution (C-NEW-1 fix)**: generated code must reference types via fully-qualified `::camel_api::component_metadata::UriOption` (NOT `#endpoint_crate::UriOption`), because `camel-component-api` does not re-export these types. Alternatively, add re-exports (`pub use camel_api::component_metadata::{ComponentMetadata, ComponentCapabilities, UriOption, OptionKind};`) to `camel-component-api/src/lib.rs` and keep the `#endpoint_crate::` path. Pick ONE approach and apply consistently.
5. Build the body as a `vec!` literal of `UriOption` constructors, one per `#[uri_param]`-annotated field. Each entry is `UriOption::new(name, description, kind)` chained with builder calls (`.required()`, `.with_default(v)`, `.secret()`, `.deprecated(reason)`, `.with_alias(a)`) per the parsed `UriParamAttr`. The path field (non-`#[uri_param]`) is excluded.
6. Emit a compile error (via `syn::Error`) if a field has both `secret` flag and a non-empty `default` value.

**Tests:**
- `uri_options_excludes_path_field`: struct with 1 path field + 1 `#[uri_param]` → `uri_options()` returns 1 entry
- `uri_options_has_secret_flag`: `#[uri_param(secret)]` → entry `.secret == true`
- `uri_options_has_default`: `#[uri_param(default = "100")]` → entry `.default_value == Some("100")`
- trybuild `tests/ui/secret_with_default_fail.rs`: `#[uri_param(secret, default = "x")]` → compile error (trybuild `compile_fail`)
- trybuild `tests/ui/kind_typo_fail.rs`: `#[uri_param(kind = "duraton")]` → compile error (trybuild `compile_fail`)
- trybuild `tests/ui/unknown_key_fail.rs`: `#[uri_param(bogus = 1)]` → compile error (trybuild `compile_fail`)
- **command**: `cargo test -p camel-endpoint-macros` (includes trybuild UI tests)
- **expected**: unit tests pass; trybuild cases confirm compile failures

**Acceptance:**
- `cargo test -p camel-endpoint-macros` green (unit + trybuild)
- `cargo clippy -p camel-endpoint-macros -- -D warnings` clean

- [ ] 1.3

#### Task 1.4: Add metadata() generation via opt-in attribute

**Files:**
- `crates/camel-endpoint-macros/src/uri_config.rs` (modified)

**Steps:**
1. Extend `UriConfigAttr` (L118-160) to parse a `metadata(..)` sub-attribute via `parse_nested_meta`: `#[uri_config(metadata(scheme = "sql", description = "..", producer, consumer, polling_consumer, streaming))]`.
2. **Nested-meta parser (C-NEW-2 fix)**: the `metadata(..)` group mixes bare flags (`producer`, `consumer`) with kv pairs (`scheme = ".."`). The current struct-attr parser (L134) does flat `is_ident` checks only. Restructure to use recursive `parse_nested_meta` with branching: if the nested meta is a bare ident (`producer`), set the corresponding capability flag; if it has a value (`scheme = ".."`), parse the string literal. Add tests for both forms.
3. Store: `metadata_scheme: Option<String>`, `metadata_description: Option<String>`, `supports_producer: bool`, `supports_consumer: bool`, `supports_polling_consumer: bool`, `supports_streaming: bool`.
3. When `metadata(..)` is present, generate an inherent `fn metadata() -> ComponentMetadata` on the config struct that returns `ComponentMetadata::minimal(scheme).with_description(desc).with_capabilities(ComponentCapabilities { supports_producer, supports_consumer, supports_polling_consumer, supports_streaming }).with_uri_options(Self::uri_options())`. Map `producer` attr flag → `supports_producer: true`, `consumer` → `supports_consumer: true`, etc.
4. When `metadata(..)` is absent, do NOT generate `metadata()`.
5. Update macro rustdoc in `lib.rs` (L1-100) to document `#[uri_config(metadata(..))]` and all new keys.

**Tests:**
- `metadata_optin_generates_uri_options`: struct with `#[uri_config(metadata(scheme = "test", description = "d"))]` + 2 `#[uri_param]` → `metadata().uri_options.len() == 2`
- `metadata_optin_capabilities`: `#[uri_config(metadata(scheme = "x", producer, consumer))]` → `metadata().capabilities.supports_producer == true && supports_consumer == true`
- trybuild `tests/ui/no_optin_no_metadata_fn.rs`: struct without `metadata(..)` → calling `Self::metadata()` as inherent fn fails (trybuild `compile_fail`)
- **command**: `cargo test -p camel-endpoint-macros`
- **expected**: all pass

**Acceptance:**
- `cargo test -p camel-endpoint-macros` green

- [ ] 1.4

### camel-api

#### Task 1.5: Add ComponentMetadata builder methods

**Files:**
- `crates/camel-api/src/component_metadata.rs` (modified)

**Steps:**
1. Add `pub fn with_description(mut self, desc: &str) -> Self` (sets `self.description`).
2. Add `pub fn with_capabilities(mut self, caps: ComponentCapabilities) -> Self`.
3. Add `pub fn with_uri_options(mut self, opts: Vec<UriOption>) -> Self`.

**Tests:**
- `with_uri_options_appends`: `minimal("x").with_uri_options(vec![UriOption::new("p","d",OptionKind::String)])` → `.uri_options.len() == 1`
- `with_capabilities_sets_flags`: `.with_capabilities(ComponentCapabilities { supports_producer: true, ..Default::default() })` → `.capabilities.supports_producer == true`
- `with_description_sets`: `.with_description("test")` → `.description == "test"`
- **command**: `cargo test -p camel-api -- with_uri_options with_capabilities with_description`
- **expected**: pass

**Acceptance:**
- `cargo test -p camel-api` green
- `cargo clippy -p camel-api -- -D warnings` clean

- [ ] 1.5

### camel-sql

#### Task 1.6: Migrate sql as reference component

**Files:**
- `crates/components/camel-sql/src/config.rs` (modified)
- `crates/components/camel-sql/src/lib.rs` (modified)

**Steps:**
1. Add `#[uri_config(metadata(scheme = "sql", description = "Execute SQL against a configured datasource", producer, consumer))]` to the sql config struct that derives `UriConfig`.
2. Add `desc`, `secret` (on password/credential fields if present), and other semantic attrs to existing `#[uri_param]` fields where known.
3. Wire `SqlComponent::metadata()` to delegate to the config's inherent `metadata()` (or compose via `with_uri_options`). This is the delegation step — without it, the catalog returns empty options.
4. Do NOT modify any existing `from_uri` parsing logic.

**Tests:**
- `sql_metadata_nonempty`: register SqlComponent in a test Registry → `get_metadata("sql").uri_options` non-empty, contains a known param name
- `sql_from_uri_unchanged`: all existing sql `from_uri` tests pass unchanged
- **command**: `cargo test -p camel-sql` and `cargo test -p camel-core -- sql_metadata`
- **expected**: metadata test passes after migration; from_uri tests pass unchanged

**Acceptance:**
- `cargo test -p camel-sql` green
- `get_metadata("sql").uri_options` non-empty

- [ ] 1.6

#### Task 1.7: Macro rustdoc + README update

**Files:**
- `crates/camel-endpoint-macros/src/lib.rs` (modified)

**Steps:**
1. Update module rustdoc (L1-100): document new `#[uri_param]` keys (`desc`, `required`, `secret`, `deprecated`, `aliases`, `kind`), `#[uri_config(metadata(..))]` form, OptionKind inference table, secret+default compile-error rule, and the component delegation convention.
2. Add a worked example showing a config struct with semantic attrs and the generated `uri_options()` + `metadata()`.

**Tests:**
- `rustdoc_builds`: `cargo doc -p camel-endpoint-macros --no-deps` exits 0
- **command**: `cargo doc -p camel-endpoint-macros --no-deps`
- **expected**: passes

**Acceptance:**
- `cargo doc -p camel-endpoint-macros --no-deps` clean
- `cargo fmt --check` clean

- [ ] 1.7

## Phase 2: Migrate MACRO-ONLY (5) + timer

### camel-file

#### Task 2.1: Migrate file component

**Files:**
- `crates/components/camel-file/src/lib.rs` (modified)

**Steps:**
1. Add `#[uri_config(metadata(scheme = "file", description = "Read/write files from a directory", consumer, producer))]` to the config struct.
2. Add `desc` attrs to existing `#[uri_param]` fields.
3. Wire `FileComponent::metadata()` to delegate to config's `metadata()` (delegation convention).
4. Do NOT modify `from_uri` parsing.

**Tests:**
- `file_metadata_nonempty`: `get_metadata("file").uri_options` non-empty
- `file_from_uri_unchanged`: existing parse tests pass
- **command**: `cargo test -p camel-file`
- **expected**: pass

**Acceptance:**
- `cargo test -p camel-file` green; fmt/clippy clean

- [ ] 2.1

### camel-cron

#### Task 2.2: Migrate cron component

**Files:**
- `crates/components/camel-cron/src/lib.rs` (modified)

**Steps:**
1. Add `#[uri_config(metadata(scheme = "cron", description = "Schedule route execution via cron expressions", consumer))]`.
2. Add `desc` to `schedule`, `timeZone`, `includeMetadata` fields.
3. Wire `CronComponent::metadata()` delegation.

**Tests:**
- `cron_metadata_nonempty`: `get_metadata("cron").uri_options` non-empty, contains "schedule"
- `cron_from_uri_unchanged`: parse tests pass
- **command**: `cargo test -p camel-cron`
- **expected**: pass

**Acceptance:**
- `cargo test -p camel-cron` green

- [ ] 2.2

### camel-opensearch

#### Task 2.3: Migrate opensearch component

**Files:**
- `crates/components/camel-opensearch/src/config.rs` (modified)

**Steps:**
1. Add `#[uri_config(metadata(scheme = "opensearch", description = "Interact with OpenSearch clusters", producer, consumer))]`.
2. Add `secret` to credential/password fields, `desc` to host/index fields.
3. Wire component `metadata()` delegation.

**Tests:**
- `opensearch_metadata_nonempty`: `get_metadata("opensearch").uri_options` non-empty
- `opensearch_secret_flag`: credential field has `secret == true` in derived options
- **command**: `cargo test -p camel-opensearch`
- **expected**: pass

**Acceptance:**
- `cargo test -p camel-opensearch` green

- [ ] 2.3

### camel-ws

#### Task 2.4: Migrate ws component

**Files:**
- `crates/components/camel-ws/src/lib.rs` (modified) or `config.rs` (modified)

**Steps:**
1. Add `#[uri_config(metadata(scheme = "ws", description = "WebSocket client/server endpoint", consumer, producer))]`.
2. Add `desc` to fields. Wire component `metadata()` delegation.

**Tests:**
- `ws_metadata_nonempty`: `get_metadata("ws").uri_options` non-empty
- **command**: `cargo test -p camel-ws`
- **expected**: pass

**Acceptance:**
- `cargo test -p camel-ws` green

- [ ] 2.4

### camel-container

#### Task 2.5: Migrate container component

**Files:**
- `crates/components/camel-container/src/lib.rs` (modified)

**Steps:**
1. Add `#[uri_config(metadata(scheme = "container", description = "Container lifecycle management endpoint", consumer))]`.
2. Add `desc` to fields. Wire component `metadata()` delegation.

**Tests:**
- `container_metadata_nonempty`: `get_metadata("container").uri_options` non-empty
- **command**: `cargo test -p camel-container`
- **expected**: pass

**Acceptance:**
- `cargo test -p camel-container` green

- [ ] 2.5

### camel-timer

#### Task 2.6: Convert timer — remove hand-written metadata, delegate to config

**Files:**
- `crates/components/camel-timer/src/lib.rs` (modified)

**Steps:**
1. Add `#[uri_config(metadata(scheme = "timer", description = "Generate timer-based events", consumer))]` to `TimerConfig`.
2. Rewrite `TimerComponent::metadata()` (L138) to delegate: `fn metadata(&self) -> ComponentMetadata { TimerConfig::metadata() }`. Remove the hand-written `UriOption` list from this method.
3. Do NOT modify `from_uri` parsing.

**Tests:**
- `timer_metadata_nonempty`: `get_metadata("timer").uri_options` non-empty, contains "period"
- `timer_no_handwritten_urioptions`: 0 `UriOption::new` in camel-timer/src outside `#[cfg(test)]`
- `timer_from_uri_unchanged`: parse tests pass
- **command**: `cargo test -p camel-timer`
- **expected**: pass

**Acceptance:**
- `cargo test -p camel-timer` green
- No `UriOption::new` in camel-timer/src outside test modules

- [ ] 2.6

### Workspace

#### Task 2.7: Phase 2 integration gate

**Files:**
- `crates/camel-core/src/component_metadata_catalog.rs` (modified — test only)

**Steps:**
1. Add a parametrized test registering all Phase-2 components and asserting non-empty `uri_options`.

**Tests:**
- `all_phase2_schemes_have_options`: for sql, file, cron, opensearch, ws, container, timer → `get_metadata(scheme).uri_options` non-empty
- `no_duplicate_option_names`: for each scheme → no two options share a name
- **command**: `cargo test -p camel-core -- all_phase2_schemes no_duplicate`
- **expected**: pass

**Acceptance:**
- All 7 schemes have non-empty options; dedup holds

- [ ] 2.7

## Phase 3a: Convert simple manual / no-UriConfig components (direct, mock, seda, log)

### camel-direct

#### Task 3a.1: Convert direct — adopt derive UriConfig with skip_impl

**Files:**
- `crates/components/camel-direct/src/lib.rs` (modified)

**Steps:**
1. Add `#[derive(UriConfig)]` + `#[uri_scheme = "direct"]` + `#[uri_config(skip_impl, metadata(scheme = "direct", description = "Synchronous in-process endpoint", consumer, producer))]` to `DirectConfig` (L67).
2. Annotate config fields with `#[uri_param(name = "..", desc = "..")]` matching current parse behavior: timeout (default "30000"), failIfNoConsume (default "true").
3. Use `skip_impl` to generate `parse_uri_components` while retaining the free-function `from_uri` logic (L77) as a manual `impl UriConfig`. This preserves any bespoke validation.
4. Delete hand-written `fn metadata()` (L168). Wire `DirectComponent::metadata()` to delegate to `DirectConfig::metadata()`.

**Tests:**
- `direct_metadata_derived`: `get_metadata("direct").uri_options` non-empty, contains "timeout"
- `direct_from_uri_unchanged`: all existing parse tests pass
- `direct_no_handwritten_urioptions`: 0 `UriOption::new` outside tests
- **command**: `cargo test -p camel-direct`
- **expected**: pass

**Acceptance:**
- `cargo test -p camel-direct` green; no `UriOption::new` outside test modules

- [ ] 3a.1

### camel-component-seda

#### Task 3a.2: Convert seda — adopt derive UriConfig with skip_impl

**Files:**
- `crates/components/camel-component-seda/src/lib.rs` (modified)

**Steps:**
1. Add `#[derive(UriConfig)]` + `#[uri_scheme = "seda"]` + `#[uri_config(skip_impl, metadata(scheme = "seda", description = "Asynchronous staging endpoint with queue", consumer, producer))]` to `SedaConfig` (L71).
2. Annotate config fields with `#[uri_param]` matching current parse. Use `skip_impl` because `SedaConfig::from_uri` (L84) has bespoke validation (name non-empty, `size > 0`, whitespace checks, enum parsing) that full derive would drop.
3. RETAIN the free-function `from_uri` validation logic as a manual `impl UriConfig`. Add regression tests for each bespoke check (empty name rejection, size=0 rejection).
4. Delete hand-written `fn metadata()` (L386). Wire `SedaComponent::metadata()` delegation.

**Tests:**
- `seda_metadata_derived`: `get_metadata("seda").uri_options` non-empty
- `seda_from_uri_unchanged`: existing parse tests pass
- `seda_validates_name_nonempty`: empty name → error (regression)
- `seda_validates_size_positive`: size=0 → error (regression)
- `seda_no_handwritten_urioptions`: 0 `UriOption::new` outside tests
- **command**: `cargo test -p camel-component-seda`
- **expected**: pass

**Acceptance:**
- `cargo test -p camel-component-seda` green; bespoke validation preserved

- [ ] 3a.2

### camel-log

#### Task 3a.3: Convert log — replace manual impl with skip_impl or derive

**Files:**
- `crates/components/camel-log/src/lib.rs` (modified)

**Steps:**
1. Add `#[derive(UriConfig)]` + `#[uri_scheme = "log"]` + `#[uri_config(skip_impl, metadata(scheme = "log", description = "Log exchange content", producer))]` to `LogConfig` (L69). Use `skip_impl` to retain the manual `impl UriConfig` (L92) parse while deriving metadata.
2. Annotate fields with `#[uri_param]` matching current parse. ~7 params (level, showAll, showBody, showHeaders, etc.).
3. Delete hand-written `fn metadata()` (L195). Wire `LogComponent::metadata()` delegation.

**Tests:**
- `log_metadata_derived`: `get_metadata("log").uri_options` non-empty
- `log_from_uri_unchanged`: existing parse tests pass
- `log_no_handwritten_urioptions`: 0 `UriOption::new` outside tests
- **command**: `cargo test -p camel-log`
- **expected**: pass

**Acceptance:**
- `cargo test -p camel-log` green

- [ ] 3a.3

### camel-mock

#### Task 3a.4: Convert mock (trivial — zero params)

**Files:**
- `crates/components/camel-mock/src/lib.rs` (modified)

**Steps:**
1. Add `#[derive(UriConfig)]` + `#[uri_scheme = "mock"]` + `#[uri_config(skip_impl, metadata(scheme = "mock", description = "Mock endpoint for testing", consumer, producer))]` to `MockConfig` (L68). `#[uri_scheme]` is required (hard error via `extract_scheme` without it).
2. Mock has zero real URI params — do NOT manufacture any. Empty `uri_options` is legitimate.
3. Delete hand-written `fn metadata()` (L212). Wire `MockComponent::metadata()` delegation to `MockConfig::metadata()` (which returns valid metadata even with zero options).

**Tests:**
- `mock_metadata_valid`: `get_metadata("mock")` returns valid metadata (scheme/description set; `uri_options` may be empty)
- `mock_no_handwritten_urioptions`: 0 `UriOption::new` outside tests
- **command**: `cargo test -p camel-mock`
- **expected**: pass

**Acceptance:**
- `cargo test -p camel-mock` green; no manufactured params

- [ ] 3a.4

## Phase 3b: Convert http (skip_impl for bespoke parse structs)

### camel-http

#### Task 3b.1: HttpEndpointConfig — skip_impl + metadata derivation

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. Add `#[derive(UriConfig)]` + `#[uri_scheme = "http"]` + `#[uri_config(skip_impl, metadata(scheme = "http", description = "HTTP client endpoint", producer))]` to `HttpEndpointConfig` (L108).
2. Annotate fields with `#[uri_param]` matching current manual parsing. Add `#[uri_param(secret)]` to `authUsername`, `authPassword`, `authBearerToken`.
3. RETAIN the manual `impl UriConfig for HttpEndpointConfig` (L178) + bespoke `from_components` unchanged. `skip_impl` generates `parse_uri_components` + `uri_options()`; the manual impl stays.
4. Delete the hand-written `fn metadata()` (L1654 region). Wire the http component's `metadata()` to delegate to `HttpEndpointConfig::metadata()`.
5. Verify `from_uri_with_defaults` (used at ~21 sites) is unaffected.

**Tests:**
- `http_endpoint_metadata_derived`: `get_metadata("http").uri_options` non-empty, auth fields have `secret == true`
- `http_from_uri_unchanged`: all existing http parse tests pass (39 tests)
- `http_from_uri_with_defaults_unchanged`: `from_uri_with_defaults` unaffected
- `http_no_handwritten_urioptions`: 0 `UriOption::new` in camel-http/src outside test modules
- **command**: `cargo test -p camel-http`
- **expected**: pass

**Acceptance:**
- `cargo test -p camel-http` green; manual impl + bespoke parsing RETAINED; auth params carry `secret`
- No `parse_uri_components`/`from_components` collision (generated inherent fn vs manual trait impl must coexist — `skip_impl` guarantees this)

- [ ] 3b.1

#### Task 3b.2: HttpServerConfig — skip_impl + metadata derivation

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. Add `#[derive(UriConfig)]` + `#[uri_scheme = "http-server"]` + `#[uri_config(skip_impl, metadata(scheme = "http-server", description = "HTTP server endpoint", consumer))]` to `HttpServerConfig` (L439).
2. Annotate fields with `#[uri_param]` matching current manual parsing.
3. RETAIN manual `impl UriConfig for HttpServerConfig` (L466) unchanged.
4. Delete hand-written `fn metadata()` for this struct. Wire delegation.

**Tests:**
- `http_server_metadata_derived`: derived metadata for HttpServerConfig valid
- `http_server_from_uri_unchanged`: existing server parse tests pass
- **command**: `cargo test -p camel-http`
- **expected**: pass

**Acceptance:**
- `cargo test -p camel-http` green

- [ ] 3b.2

#### Task 3b.3: HttpStaticConfig — skip_impl + metadata derivation

**Files:**
- `crates/components/camel-http/src/static_config.rs` (modified)
- `crates/components/camel-http/src/static_endpoint.rs` (modified)

**Steps:**
1. Add `#[derive(UriConfig)]` + `#[uri_scheme = "http-static"]` + `#[uri_config(skip_impl, metadata(scheme = "http-static", description = "Static file server endpoint", consumer))]` to `HttpStaticConfig` (static_config.rs L20).
2. Annotate fields with `#[uri_param]` matching manual parsing at L118-160 (mount_path/dir bespoke logic).
3. RETAIN manual `impl UriConfig for HttpStaticConfig` (L118) + bespoke `from_components` unchanged.
4. Delete hand-written `fn metadata()` in static_endpoint.rs (L49 region). Wire delegation.

**Tests:**
- `http_static_metadata_derived`: derived metadata for HttpStaticConfig valid
- `http_static_from_uri_unchanged`: existing static parse tests pass (mount_path, root-path rejection)
- **command**: `cargo test -p camel-http`
- **expected**: pass

**Acceptance:**
- `cargo test -p camel-http` green; bespoke parsing unchanged

- [ ] 3b.3

### Workspace

#### Task 3b.4: Full unification gate (xtask lint)

**Files:**
- `scripts/xtask/src/main.rs` (modified)
- `scripts/xtask/src/lint_single_source.rs` (new)

**Steps:**
1. Add a new xtask command `lint-single-source` that scans component crate source for `UriOption::new` calls OUTSIDE `#[cfg(test)]` modules. Implementation approach: parse each `.rs` file with `syn`, walk the AST, skip items within `#[cfg(test)]` modules, count `UriOption::new` call expressions. Report violations with file:line.
2. Add it to the `Commands` enum and wire `main()`.
3. Add a workspace test asserting all migrated components are in the catalog with valid metadata.

**Tests:**
- `lint_single_source_clean`: `cargo xtask lint-single-source` exits 0 after all migrations
- `all_components_in_catalog`: all component schemes (sql, file, cron, opensearch, ws, container, timer, direct, seda, log, mock, http) return valid metadata from catalog (mock may have empty `uri_options`; rest non-empty)
- `no_duplicate_option_names_all`: for ALL 12 schemes → no two options share a name (extended from Phase 2 gate)
- **command**: `cargo xtask lint-single-source` and `cargo test -p camel-core -- all_components_in_catalog no_duplicate_option_names_all`
- **expected**: both pass

**Acceptance:**
- `cargo xtask lint-single-source` exits 0 (zero violations outside test modules)
- All component schemes in catalog

- [ ] 3b.4

#### Task 3b.5: ADR-0041 amendment + CONTEXT-MAP glossary

**Files:**
- `docs/adr/0041-component-metadata-capabilities-schema.md` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Append amendment to ADR-0041: macro-derived `uri_options` via `#[derive(UriConfig)]`; OptionKind inference rules (never Enum); semantic fields via `#[uri_param]` keys; `skip_impl` path for bespoke-parse structs; component-to-config delegation convention; single-source-of-truth invariant enforced by `lint-single-source` xtask.
2. Add glossary entry in CONTEXT-MAP.md cross-linking `uri_options()` derivation and metadata unification.

**Tests:**
- `adr_amended`: ADR-0041 contains "uri_options", "skip_impl", "inference", "delegation"
- **command**: `rg -c 'uri_options|skip_impl|inference|delegation' docs/adr/0041-component-metadata-capabilities-schema.md`
- **expected**: count > 0

**Acceptance:**
- ADR-0041 amended; CONTEXT-MAP glossary updated; `cargo fmt --check` clean

- [ ] 3b.5
