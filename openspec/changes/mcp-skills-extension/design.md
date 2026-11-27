# Design: mcp-skills-extension

## Approach

### Configuration

`McpServerConfig` gains `skills_root: Option<String>` with a non-empty
deserializer. `deny_unknown_fields` stays. `get_or_spawn` compares `skills_root`
with the live listener config, like caps and TLS: a second named server on the
same bind with a different root is rejected. The DSL `mcp:` block does not carry
the key in this slice.

Probes recorded 2026-11-27, before this change:

- Deserializing `{"bind": "127.0.0.1:9123", "skills_root": "/tmp/skills"}` fails
  with `unknown field \`skills_root\``. The extension is impossible today.
- `Cargo.lock` pins rmcp `3.9.0`, which owns the typed SEP-2640 surface:
  `ServerHandler::list_skills` / `get_skill`, `ListSkillsResult`,
  `GetSkillResult`, `Skill`, `SkillFrontmatter`, `SkillResources`,
  `SkillResource`, `SkillsCapability`. None of it is used today. When the
  extension is undeclared, rmcp answers `skills/*` with `-32601` before any
  handler runs.

### Loading

New `src/skills.rs`, with no rmcp imports (ADR-0020). `McpSkillCatalog::load`
takes the root path and returns a typed error:

1. Canonicalize the root. A missing or non-directory root is
   `McpError::SkillLoad`.
2. Each direct child directory that contains `SKILL.md` is a skill. Other
   direct children are ignored.
3. `SKILL.md` must start with YAML frontmatter (`---` … `---`). Parse the
   frontmatter into `noyalib::Value` with
   `ParserConfig::serde_yaml_compat().duplicate_key_policy(DuplicateKeyPolicy::Error)`,
   then reject tagged values and non-finite floats before converting with
   `serde_json::to_value`; any failure is `McpError::SkillLoad`. `noyalib`
   mapping keys are already strings (scalar keys are stringified, non-scalar
   keys fail at parse), so no key check is needed. Probe
   2026-11-27 (noyalib 0.0.54): the default compat path keeps the last duplicate
   key and maps `.inf`/`.nan` to `null`, so the strict config and the explicit
   walk are required. Require string `name` and `description`. Apply the
   extension's naming rules: name is 1–64 characters of `[a-z0-9-]`, no leading
   or trailing `-`, no `--`; description is 1–1024 characters. The skill
   directory name must equal `name`.
4. Walk the skill directory iteratively in sorted order. A symlinked direct
   child of the root follows the same rule: a canonical target outside the
   canonical root is `McpError::SkillPathEscape`, never ignored. Canonicalize
   every candidate path and require the canonical target to be a regular file.
   Count entries and bytes from file metadata before reading; more than 512
   files or more than 16,777,216 bytes per skill is `McpError::SkillLoad`. Read
   accepted files into memory. Digest = `sha256:<hex>` over the served bytes;
   size = raw byte count.
5. URI = `skill://<skill-dir>/<relative-path>`, percent-encoded per segment.
   Manifests and skills are sorted, so replies are deterministic.

Serving stored bytes makes the manifest authoritative: the digest and size a
client verifies always match the bytes the server returns.

### Protocol surface

`McpServerAdapter` gains `skills: Option<Arc<McpSkillCatalog>>`.
`capabilities()` takes the presence flag and, when set, inserts a raw
`JsonObject` `{"directoryRead": false}` under
`extensions["io.modelcontextprotocol/skills"]`, next to `resources` and
`tools`. The raw object satisfies the mission's declaration and the extension's
default; rmcp parses it as `SkillsCapability { directory_read: false }`.

The adapter overrides rmcp's typed handlers:

- `list_skills(request: Option<PaginatedRequestParams>, context)`. The cursor
  is an opaque decimal offset; page size is 50 skills and an entry is never
  split. A non-numeric or out-of-range cursor answers `-32602`. The result is
  `ListSkillsResult::with_all_items(page).with_ttl_ms(0)
  .with_cache_scope(CacheScope::Private)`, with `next_cursor` set when another
  page exists.
- `get_skill(request: GetSkillRequestParams, context)`. The URI is the skill
  entry URI (`…/SKILL.md`). The result is `GetSkillResult::new(entry)` with the
  same cache metadata. Unknown URI answers `-32602`; rmcp already rejects a URI
  that does not end in `/SKILL.md`.

`on_custom_request` stays unimplemented. With no catalog, the extension is
absent from capabilities and rmcp's `validate_skills_capability` answers both
methods with `-32601`.

The adapter converts project-owned catalog entries into rmcp `Skill` values:
`SkillFrontmatter` from the stored JSON, and `SkillResources::Files` from the
stored manifest. All rmcp types stay in `src/adapter/`.

`resources/read`: while a catalog exists, it owns the `skill://` namespace.
A URI with a `..` segment (raw or percent-encoded) answers `-32602` with an
escape message. A manifest URI is served with its stored MIME type and bytes
(text when valid UTF-8, blob otherwise) and a `ReadResourceResult` carrying
`ttlMs: 0` and `cacheScope: "private"`. An unknown `skill://` URI answers
`-32602`. Non-`skill://` URIs keep the existing registry dispatch unchanged.
`resources/list` does not include skill files: the complete manifest is the
extension's listing channel, and injecting files would consume the resource cap
and mix route-owned entries with files.

MIME type: `.md` → `text/markdown`, `.json` → `application/json`, `.txt` →
`text/plain`, otherwise `application/octet-stream`.

### Resource URI precedence

The canonical `Resource URI addressing` requirement resolves reads through the
registry. This change modifies it: while a skills root is configured, manifest
URIs are served by the catalog first. A resource route declaring a `skill://`
URI on a listener with a skills root fails consumer start with a `CamelError`,
so one URI has exactly one owner.

### Authentication

Skills have no route plan. Visibility follows the same kernel path as
`tools/list`, applied at the bind level, with a fail-closed latch:

- `McpBindSecurity` gains `protected_seen: AtomicBool`, set by
  `commit_protection` (inside `spawn`, before either transport serves) or by a
  winning `register_plan_takeover` under the plan-book lock. A failed or
  cancelled start never commits; a stopped protected route cannot clear it; the
  security book is carried across dead-listener eviction, so the commitment
  survives respawns.
- While `protected_seen` is false, skills are public.
- Once true, a caller is admitted only when at least one live non-`Public` plan
  admits it through `kernel_authn_for` plus `plan_catalog_visible` (the same
  authentication and authorization tail that `catalog_entry_visible` applies to
  tools and resources). No live protected plan means hidden.
- A hidden caller is treated as seeing an empty catalog: `skills/list` returns
  the empty page with cache metadata, and a supplied cursor validates exactly
  as it would against an empty catalog; `skills/get` and `resources/read`
  answer `-32602`.
- Authentication outcomes are cached by the full tuple
  `(credential_sources, provider_ref, audience_binding, transport)` — every
  field `kernel_authenticate` reads — so one request runs at most one
  `kernel_authenticate` per distinct key. Policy decisions still run once per
  plan: two plans that share the key but carry different scopes are both
  evaluated.

A hidden skill is omitted from `skills/list`; `skills/get` and
`resources/read` answer `-32602` with the same message as an unknown skill, so
existence is not disclosed.

### Cache metadata

`ttlMs: 0`, `cacheScope: "private"` on `skills/list`, `skills/get`, and
skill-file reads. The extension requires the fields on all three result types
at `2026-07-28`. The manifest is fixed for one listener lifetime, but visibility
depends on the caller's authentication and on the bind's plan history, so
sharing a cached reply across authorization contexts is unsafe. This matches the
existing non-cacheable catalog rule. Route-resource reads keep their current
shape: rmcp's `ServerHandler` dispatch already fills missing hints with
`ttlMs: 0` and `cacheScope: "private"` on every complete `resources/read` result
for `2026-07-28` peers (`ServerResult::fill_missing_cache_hints`), so no wire gap
exists. bd rc-xp5da tracks only the missing canonical scenario and wire test.

Accepted risk: the number of skills under one root is unbounded. The per-skill
limits bound each skill; the tree is trusted operator configuration, like a
route catalog, and is loaded once at listener start.

### Errors and startup

Two typed variants join `McpError`: `SkillLoad { skill: Option<String>, reason:
String }` and `SkillPathEscape { path: String }`. Both map to
`CamelError::Config` at listener start. A bad root or invalid skill stops the
start with a named error; the listener never serves a partial catalog.

`get_or_spawn` loads the catalog before spawning and binding (per-slot
single-flight; a failed load binds nothing), stores it on
`McpListenerHandle`, and passes it to `mcp_router`. An absent root means no
catalog and no behavior change.

### Docs

`docs/src/components/mcp.md` gains `skills_root` in the config list and a short
Skills section. The component `CONTEXT.md` gains the config key and a Skills
note; `CONTEXT-MAP.md` gains a Skill Catalog term and updates the MCP Component
entry and the Shared Listener conflict-key list, following the `mcp-list-auth`
precedent.

## Affected crates

- `camel-component-mcp`: config, registry, adapter, new skills module, tests.
- `docs`: `src/components/mcp.md`, component `CONTEXT.md`, `CONTEXT-MAP.md`.

No other crate changes. `sha2`, `noyalib`, and `percent-encoding` are already
in the workspace and `Cargo.lock`. `sha2` and `noyalib` join the crate's
dependencies, and `percent-encoding` moves from dev-dependencies to
dependencies for `src/skills.rs`; no new package enters the lockfile.

## Architecture boundaries

Components layer. rmcp stays in `src/adapter/` (ADR-0020). `src/skills.rs` is
project-owned and exposes bytes plus `serde_json::Value`; no MCP type enters
`camel-api` or another crate (ADR-0045). Config joins the existing
`McpServerConfig` surface, registry ownership follows ADR-0068, and
authentication follows the unified kernel (ADR-0061). This change extends the v1
protocol surface with one opt-in extension; ADR-0060's exclusion list for base
primitives and transports is unchanged.

## Size check

Single crate plus docs, one subsystem. The delta adds five requirements and
modifies two, with about 22 scenarios. The loader validation cases are facets
of one load-time gate answered by two typed error variants, not separate
features.

## Alternatives considered

- **Route `skills/*` through `on_custom_request`.** Rejected: rmcp 3.9.0 already
  dispatches `ListSkillsRequest` / `GetSkillRequest` to typed handlers and
  validates declared capability and result shape. Custom handling would bypass
  that validation.
- **Register skill files in `McpResourceRegistry`.** Rejected: the registry
  holds route senders and owner tokens, enforces the resource cap, and has no
  manifest or digest semantics.
- **Read files at request time.** Rejected: digest/size could diverge from the
  served bytes, and it reopens the path at every request.
- **Add `serde_yaml`/`serde_yml`.** Rejected: a new package. `noyalib`'s
  `compat::serde_yaml` shim is already in the lockfile.
- **Compile the server `security_policy` JSON for skills.** Rejected: no such
  compile path exists. Reusing live route plans plus the latch keeps one
  authentication interpretation.
- **Add the DSL key now.** Deferred: it would touch `camel-dsl` and `camel-lint`
  and exceed the one-crate scope.
