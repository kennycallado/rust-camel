# Tasks — mcp-skills-extension

Single-phase change. Six tasks: catalog loader, listener wiring, protocol
surface (list/get), auth latch, resource reads/path safety, docs.

Mission test map: `skills_extension_not_advertised_without_root`,
`skills_list_returns_complete_manifest_with_digests`,
`skills_get_by_uri_matches_list_entry`,
`skill_file_is_served_through_resources_read`,
`skill_path_escape_is_rejected`,
`skills_list_follows_catalog_authentication`,
`skills_list_paginates_with_next_cursor`.

## camel-component-mcp — catalog

### Task 1.1: skill catalog loader and `skills_root` config

**Files:**
- `crates/components/camel-component-mcp/Cargo.toml` (modified)
- `crates/components/camel-component-mcp/src/error.rs` (modified)
- `crates/components/camel-component-mcp/src/config.rs` (modified)
- `crates/components/camel-component-mcp/src/skills.rs` (new)
- `crates/components/camel-component-mcp/src/lib.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_config_test.rs` (modified)

**Steps:**
1. Cargo.toml: add `noyalib = { workspace = true }` and
   `sha2 = { workspace = true }` under `[dependencies]`. Move
   `percent-encoding = { workspace = true }` from `[dev-dependencies]` to
   `[dependencies]` (the test suite imports it directly; a normal dependency is
   visible to integration tests). Do not touch `Cargo.lock` by hand; the only
   lock change must be the `camel-component-mcp` dependency list.
2. error.rs: add `SkillLoad { skill: Option<String>, reason: String }` with
   `#[error("skill load error: {reason}")]` (the reason names the skill when
   one is identifiable) and `SkillPathEscape { path: String }` with
   `#[error("skill path escapes the skills root: {path}")]`. Map both to
   `CamelError::Config(e.to_string())` in `From<McpError> for CamelError`.
3. config.rs: add `#[serde(default, deserialize_with = "deserialize_skills_root")]`
   `pub skills_root: Option<String>` to `McpServerConfig`. The deserializer
   trims the value, maps an empty result to
   `serde::de::Error::custom("skills_root must not be empty")`, and returns
   `None` for an absent field.
4. skills.rs: add the module with these public items:
   `SKILLS_PAGE_SIZE: usize = 50`, `MAX_SKILL_FILES: usize = 512`,
   `MAX_SKILL_BYTES: u64 = 16_777_216`;
   `McpSkillCatalog::load(root: &Path) -> Result<Self, McpError>`,
   `skills() -> &[McpSkillEntry]`, `get(uri) -> Option<&McpSkillEntry>`,
   `file(uri) -> Option<&McpSkillFile>`,
   `page(cursor: Option<&str>) -> Result<SkillPage<'_>, SkillCursorError>`,
   `len()`, `is_empty()`. The cursor is an opaque decimal offset into the
   sorted `skills()` slice. `None` is always valid and returns the first page
   (empty `skills`, `next_cursor` `None` on an empty catalog). A supplied
   cursor that is non-numeric, or whose offset is at or beyond `len()`,
   returns `SkillCursorError::Invalid` for every catalog size; `next_cursor`
   is `Some((offset + 50).to_string())` only when more entries remain;
   `McpSkillEntry { pub uri: String, pub frontmatter: serde_json::Value,
   pub files: Vec<McpSkillFile> }`;
   `McpSkillFile { pub uri: String, pub digest: String, pub size: u64,
   pub mime_type: &'static str, pub bytes: Arc<[u8]> }`;
   `SkillPage<'a> { pub skills: &'a [McpSkillEntry], pub next_cursor:
   Option<String> }`; `SkillCursorError::Invalid`.
5. skills.rs loader: canonicalize the root (missing/non-directory →
   `SkillLoad`). For each direct child, canonicalize; a failed canonicalize is
   `SkillLoad` (never ignored) and a target outside the canonical root →
   `SkillPathEscape`; a directory containing `SKILL.md` is a skill, other
   children are ignored. Walk each skill iteratively with an explicit stack and
   a `HashSet<PathBuf>` of visited canonical directories (repeat →
   `SkillLoad { skill: Some(skill.clone()), reason: "symlink cycle".into() }`). For every entry: canonicalize; outside root
   → `SkillPathEscape`; require a regular file or directory; non-regular
   entries → `SkillLoad`; a non-UTF-8 file name → `SkillLoad`. Count files and
   bytes from metadata before reading; more than `MAX_SKILL_FILES` or
   `MAX_SKILL_BYTES` → `SkillLoad`. Read each accepted file through
   `File::open(&path).map_err(|e| skill_io_error(&skill, &path, e))?` then
   `.take(remaining + 1).read_to_end(&mut bytes)
   .map_err(|e| skill_io_error(&skill, &path, e))?`, where `skill_io_error`
   returns `SkillLoad { skill: Some(skill), reason: format!("cannot read '{}': {e}", path.display()) }`;
   reject a read that exceeds the remaining byte budget. Digest the bytes with
   `sha2::Sha256` formatted as `sha256:<64 lowercase hex>`; size is
   `bytes.len()`.
6. skills.rs frontmatter: require the file to start with `---\n` or
   `---\r\n` and find the closing line `---` or the YAML document-end marker
   (three dots); no frontmatter → `SkillLoad`. Parse the YAML with
   `noyalib::from_str_with_config::<noyalib::Value>(yaml,
   &noyalib::ParserConfig::serde_yaml_compat()
   .duplicate_key_policy(noyalib::DuplicateKeyPolicy::Error))`. Walk the value
   recursively through `Mapping` values and `Sequence` items: reject
   `Value::Number` whose `as_f64()` is not finite, and reject
   `Value::Tagged` → `SkillLoad`. `noyalib::Mapping` keys are already
   `String` (scalar keys are stringified, non-scalar keys fail at parse), so
   no key check is needed. Convert with
   `serde_json::to_value`; an error → `SkillLoad`.
   Require a JSON object with string `name` and `description`. Name rule:
   1–64 chars, bytes in `[a-z0-9-]`, no leading/trailing `-`, no `--`.
   Description rule: 1–1024 chars (`.chars().count()`). The directory name
   must equal `name`; otherwise `SkillLoad { skill: Some(dir) }`.
7. skills.rs URI: `skill://` followed by the skill directory and the
   relative file path, with each segment encoded by
   `percent_encoding::utf8_percent_encode` and the set
   `NON_ALPHANUMERIC` minus `-`, `.`, `_`, `~`. Sort files by URI and skills by
   directory name. MIME: `.md` → `text/markdown`, `.json` →
   `application/json`, `.txt` → `text/plain`, otherwise
   `application/octet-stream`.
8. lib.rs: add `pub mod skills;`.
9. Add the unit tests below inside `src/skills.rs` under `#[cfg(test)] mod
   tests` BEFORE steps 4-7 land: write them first, run them to see them fail,
   then implement.

**Tests:** (write first; see them fail, then implement)
- `catalog_builds_complete_manifest_with_digests`: arrange: temp root with
  `code-review/SKILL.md` (`---\nname: code-review\ndescription: Review code.\nlicense: MIT\n---\n# Code review\n`)
  and `references/checklist.md` (`# checklist\n`); act: `load`; assert: one
  skill, URI `skill://code-review/SKILL.md`, frontmatter carries
  `name`/`description`/`license`, two files, `SKILL.md` first, each digest
  equals `sha256:` of the file bytes, each size equals the byte length.
- `catalog_percent_encodes_uri_segments`: arrange: a supporting file named
  `my notes.md` under `references`; act: `load`; assert its URI is
  `skill://code-review/references/my%20notes.md`.
- `catalog_rejects_invalid_skill_identity`: arrange seven separate roots:
  (1) `SKILL.md` with no frontmatter; (2) directory `code-review` whose
  `name: other` differs from the directory; (3) directory `Bad_Name` with
  `name: Bad_Name`; (4) `code-review` with an empty description; (5)
  `code-review` with a 65-character name; (6) `code-review` with
  `name: a--b` whose directory is `a--b`; (7) `code-review` with no
  `name`; act: `load` each; assert each is `Err(McpError::SkillLoad { .. })`
  and `skill.as_deref() == Some("code-review")` or `Some("Bad_Name")` /
  `Some("a--b")` matching that case's directory.
- `catalog_rejects_duplicate_and_nonfinite_frontmatter`: arrange:
  `name: code-review` twice, and separately `x: .inf`; act: `load`; assert
  both are `Err(McpError::SkillLoad { .. })` with
  `skill.as_deref() == Some("code-review")`.
- `catalog_rejects_escaping_symlink` (`#[cfg(unix)]`): arrange: temp outer dir
  with `outside.txt`, root with `code-review/SKILL.md`, and a symlink
  `code-review/leak.md` to `outside.txt`; act: `load`; assert
  `Err(McpError::SkillPathEscape { .. })`.
- `catalog_first_page_is_valid_on_an_empty_catalog`: arrange: a root with no
  skill directory; act: `load`, then `page(None)` and `page(Some("0"))`;
  assert: `page(None)` is `Ok` with empty `skills` and `next_cursor == None`;
  `page(Some("0"))` is `Err(SkillCursorError::Invalid)`.
- `skill_limits_are_inclusive_at_512_files_and_16_mib`: arrange: skill with
  `SKILL.md` plus 511 `file-NNN.txt` (512 files) → load succeeds; replace with
  512 plus `SKILL.md` (513) → `SkillLoad`; separately a skill whose total bytes
  equal 16,777,216 → succeeds, and one byte more → `SkillLoad`.
- `skills_root_parses_and_empty_is_rejected` (server_config_test.rs): arrange:
  JSON `{"bind": "127.0.0.1:9100", "skills_root": "/srv/skills"}` and
  `{"bind": "127.0.0.1:9100", "skills_root": "  "}`; act: deserialize both;
  assert first carries `Some("/srv/skills")`, second is an error whose text
  contains `skills_root`.

**Acceptance:**
- `cargo test -p camel-component-mcp --lib skills::tests` → all new unit
  tests pass.
- `cargo test -p camel-component-mcp --test server_config_test` → passes.
- `cargo test -p camel-component-mcp` → passes (declared component lane,
  AGENTS.md `## COMPONENT LANES`).
- `cargo fmt --check --all` exit 0.
- `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exit 0.
- `git status --short -- Cargo.lock crates/components/camel-component-mcp` lists
  exactly: `M Cargo.lock`,
  `M crates/components/camel-component-mcp/Cargo.toml`,
  `M crates/components/camel-component-mcp/src/error.rs`,
  `M crates/components/camel-component-mcp/src/config.rs`,
  `M crates/components/camel-component-mcp/src/lib.rs`,
  `M crates/components/camel-component-mcp/tests/server_config_test.rs`,
  `?? crates/components/camel-component-mcp/src/skills.rs`.
- `git diff -- Cargo.lock | grep -c '^[+-]name = '` prints `0`; the only
  changed lock lines are the `camel-component-mcp` dependency list (adding
  `"noyalib"` and `"sha2 0.11.1"`; the lock already holds sha2 0.10.9 and
  0.11.1, so the version suffix is present).

- [x] 1.1

### Task 1.2: load the catalog at listener spawn and reject bind conflicts

**Files:**
- `crates/components/camel-component-mcp/src/registry.rs` (modified)
- `crates/components/camel-component-mcp/src/adapter/server.rs` (modified)
- `crates/components/camel-component-mcp/tests/server_registry_test.rs` (modified)

**Steps:**
1. registry.rs: add `pub skills: Option<Arc<McpSkillCatalog>>` to
   `McpListenerHandle`; include
   `skills: self.skills.as_ref().map(|catalog| catalog.len())` in the manual
   `Debug` impl.
2. registry.rs: `get_or_spawn` loads the catalog (per-slot single-flight,
   `load_skills_for_slot`) before the slot initializes and before any bind;
   a load error returns `Err(McpError)` and binds nothing. `spawn` takes the
   loaded `Option` and carries it into both TLS and plain `mcp_router` calls
   and into the returned handle.
3. registry.rs `check_conflict`: compare `existing.cfg.skills_root` with
   `cfg.skills_root`; a difference returns
   `McpError::Endpoint(format!("conflicting bind '{bind}': skills_root differs from the existing shared listener"))`.
4. adapter/server.rs `McpServerAdapter`: add
   `pub(crate) skills: Option<Arc<McpSkillCatalog>>`; `mcp_router` takes the
   new parameter and stores it in the adapter. No protocol method changes yet.
   Add `skills: None` to the `test_adapter()` helper in the `#[cfg(test)] mod
   tests` block of `adapter/server.rs`.
5. Add the tests below after the loader code is in place.

**Tests:**
- `skills_root_conflict_rejected` (server_registry_test.rs): arrange: two
  existing temp directories `root-a` (skill `alpha`) and `root-b` (skill
  `beta`), and two configs for bind `127.0.0.27:0`, one with each root; act:
  `get_or_spawn` the first, then the second on the same bind, then inspect
  `first.skills`; assert: first succeeds; second is
  `Err(McpError::Endpoint(message))` with `message.contains("skills_root")`;
  `first.skills` is `Some`, its `len() == 1`, and
  `get("skill://alpha/SKILL.md")` is `Some` (handle-level check; `skills/list`
  lands in Task 2.1).
- `missing_skills_root_fails_spawn` (server_registry_test.rs): arrange: one
  config for `127.0.0.28:0` whose `skills_root` names a path under the test
  temp dir that does not exist, and one config for `127.0.0.29:0` whose
  `skills_root` names an existing regular file; act: `get_or_spawn` each;
  assert: both are `Err(McpError::SkillLoad { .. })`, each error's
  `to_string()` contains the offending root path, and a later `get_or_spawn`
  with a valid config on `127.0.0.28:0` succeeds.

**Acceptance:**
- Both tests pass.
- `cargo test -p camel-component-mcp --test server_registry_test` → passes.
- `cargo test -p camel-component-mcp` → passes (declared component lane,
  AGENTS.md `## COMPONENT LANES`).
- `cargo test -p camel-component-mcp --test server_protocol_test` → unchanged
  behavior (no-root listener still advertises tools + resources only).
- `cargo fmt --check --all` and
  `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exit 0.

- [x] 1.2

## camel-component-mcp — protocol surface

### Task 2.1: advertise the extension and implement `skills/list` / `skills/get`

**Files:**
- `crates/components/camel-component-mcp/src/adapter/server.rs` (modified)
- `crates/components/camel-component-mcp/tests/skills_extension_test.rs` (new)

**Steps:**
1. Write the new test file first with `mod common;`: it starts with the five
   non-auth tests below (Task 2.2 adds the four auth tests, Task 2.3 adds six
   read/path tests, including one auth read test). Shared helpers: a `TempDir` guard built from
   `std::env::temp_dir().join(format!("camel-mcp-skills-{tag}-{pid}-{n}"))`
   with an `AtomicU64` counter and `Drop` removing the tree; a
   `write_skill(root, dir, frontmatter, files)` helper; a `spawn_with_root`
   helper using `McpServerRegistry::global().get_or_spawn` on a bind from
   `next_bind()` = `format!("127.0.0.{}:0", 81 + NEXT_BIND.fetch_add(1,
   Ordering::SeqCst))` with `static NEXT_BIND: AtomicU8 = AtomicU8::new(0)`;
   the file owns `127.0.0.81`–`127.0.0.95` (15 listeners). Reuse
   `common::raw_rmcp_client(addr)` for the
   discover-lifecycle client; a `raw_json_rpc_post` helper copied from
   `tests/server_protocol_test.rs` with a 5-second write and a 5-second
   read-to-EOF bound (`tokio::time::timeout`). Every raw POST in this file
   carries `params._meta` with both
   `io.modelcontextprotocol/protocolVersion: "2026-07-28"` and
   `io.modelcontextprotocol/clientCapabilities: {}`, `server/discover`
   included, plus `MCP-Protocol-Version: 2026-07-28` and an `Mcp-Method`
   header equal to its body method (`server/discover` uses
   `Mcp-Method: server/discover`). Token clients and security-plan helpers
   arrive in Task 2.2.
2. adapter/server.rs `capabilities(enabled: bool) -> ServerCapabilities`:
   always set tools + resources. When `enabled`, insert a
   `serde_json::Map` `{"directoryRead": false}` under
   `capabilities.extensions["io.modelcontextprotocol/skills"]`. `get_info`
   calls `capabilities(self.skills.is_some())`.
3. adapter/server.rs: add
   `fn rmcp_skill(entry: &McpSkillEntry) -> Result<Skill, ErrorData>` that
   builds `SkillFrontmatter` with `serde_json::from_value(entry.frontmatter.clone())`
   and `SkillResources::Files` from `entry.files` (`SkillResource::new`), and
   `Skill::new(entry.uri.clone(), frontmatter, resources)`. A conversion
   failure returns `ErrorData::internal_error`.
4. adapter/server.rs: override
   `async fn list_skills(&self, request: Option<PaginatedRequestParams>,
   context: RequestContext<RoleServer>) -> Result<ListSkillsResult, ErrorData>`.
   Read the cursor from `request.and_then(|p| p.cursor)`. No catalog → return
   an empty `ListSkillsResult` with `with_ttl_ms(0).with_cache_scope(
   CacheScope::Private)` (defensive; rmcp never routes here without a
   catalog). Otherwise `catalog.page(cursor)`: `SkillCursorError::Invalid` →
   `ErrorData::invalid_params("invalid skills cursor", None)`. Convert each
   entry with `rmcp_skill`, set `next_cursor` from the page, and return with
   the same cache metadata.
5. adapter/server.rs: override
   `async fn get_skill(&self, request: GetSkillRequestParams, context:
   RequestContext<RoleServer>) -> Result<GetSkillResult, ErrorData>`. Resolve
   `self.skills.get(&request.uri)`; missing →
   `ErrorData::invalid_params(format!("skill '{}' is not available", request.uri), None)`.
   Return `GetSkillResult::new(skill).with_ttl_ms(0).with_cache_scope(CacheScope::Private)`.
6. Run the new tests: tests 2-5 must fail before steps 2-5 (no extension);
   test 1 is the negative control and must stay green before and after; the
   loader/config tests from Task 1.1 stay green.

**Tests:** (write first; the mission names are exact)
- `skills_extension_not_advertised_without_root`: arrange: `let bind =
  next_bind();` and spawn a listener whose config is
  `serde_json::json!({"bind": bind})`; act: raw
  `server/discover` POST and raw `skills/list` POST (both with the `_meta` and
  header prelude above); assert: the discover reply is HTTP 200,
  `result.capabilities` is an object carrying `tools` and `resources` and no
  `extensions` key at all, and `skills/list` returns
  `error.code == -32601`.
- `skills_list_returns_complete_manifest_with_digests`: arrange: root with
  the Task 1.1 `code-review` fixture plus a second valid skill `deploy`;
  spawn; discover client; act: `peer().list_skills(None)` and one raw
  `server/discover` POST; assert: the raw discover JSON carries
  `capabilities.extensions["io.modelcontextprotocol/skills"] ==
  {"directoryRead": false}`; `result_type` is `Some(ResultType::COMPLETE)`,
  `ttl_ms == Some(0)`, `cache_scope == Some(CacheScope::Private)`, two skills,
  the `code-review` entry has URI `skill://code-review/SKILL.md`, frontmatter
  name/description/`license`, `resources` is `SkillResources::Files` with two
  entries, and every `SkillResource::verify(bytes)` passes against the
  on-disk file bytes. Also raw POST `resources/directory/read` for
  `{"uri": "skill://code-review", "_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientCapabilities": {}}}`
  with `Mcp-Method: resources/directory/read` and assert
  `error.code == -32601` (the extension declared `directoryRead: false`).
- `skills_get_by_uri_matches_list_entry`: arrange: same fixture, one
  `list_skills` page; act: `peer().get_skill(GetSkillRequestParams::new(code_review.uri.clone()))`
  where `code_review` is the entry named in the list; assert:
  `result.skill == code_review`, `result_type`/`ttl_ms`/`cache_scope` as
  above.
- `skills_list_paginates_with_next_cursor`: arrange: 51 skill directories
  `skill-00`..`skill-50`, each with a valid `SKILL.md`; spawn; client; act:
  page 1 `list_skills(None)`, page 2 with
  `list_skills(Some(PaginatedRequestParams::default().with_cursor(page1.next_cursor.clone())))`
  (rmcp's struct is `#[non_exhaustive]`; a struct literal fails with E0639);
  assert: page 1 has 50 entries and `next_cursor.is_some()`, page 2 has 1
  entry and `next_cursor.is_none()`, the 51 URIs are unique, and every
  entry's manifest is non-empty.
- `skills_bad_cursor_and_unknown_uri_are_invalid_params`: arrange: one valid
  skill; act: `list_skills` with
  `PaginatedRequestParams::default().with_cursor(Some("not-a-number".into()))`
  and `with_cursor(Some("999".into()))` (out of range), then
  `get_skill(GetSkillRequestParams::new(
  "skill://nope/SKILL.md"))`; assert all three errors carry `-32602`.

**Acceptance:**
- All five tests above pass.
- `cargo test -p camel-component-mcp --test skills_extension_test` → passes.
- `cargo test -p camel-component-mcp` → passes (declared component lane,
  AGENTS.md `## COMPONENT LANES`).
- `cargo test -p camel-component-mcp --test server_protocol_test` and
  `--test server_tool_dispatch_test` → unchanged.
- `cargo fmt --check --all` and
  `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exit 0.

- [x] 2.1

### Task 2.2: protect the Skills catalog with the route-plan latch

**Files:**
- `crates/components/camel-component-mcp/src/registry.rs` (modified)
- `crates/components/camel-component-mcp/src/adapter/server.rs` (modified)
- `crates/components/camel-component-mcp/tests/skills_extension_test.rs` (modified)

**Steps:**
1. Add the auth test scaffolding to `tests/skills_extension_test.rs`: a local
   `rmcp_client_with_token(addr, token)` wrapper (the
   `server_resource_dispatch_test.rs` shape) and copies of `test_principal`,
   `provider_registry`, `scoped_plan`, and `public_plan` from
   `server_tool_dispatch_test.rs`.
2. registry.rs `McpBindSecurity`: add
   `protected_seen: AtomicBool` initialized `false`; store it with
   `store(true, Ordering::SeqCst)` ONLY after a successful `plans.insert` in
   both `register_plan` and `register_plan_takeover`, inside the same `plans`
   lock hold as the insert, after the keep-incumbent early return, and only
   when `!matches!(plan.access_mode, AccessMode::Public)`. Add
   `pub fn protected_seen(&self) -> bool`.
3. adapter/server.rs: refactor `catalog_entry_visible` so its tail becomes
   `async fn plan_catalog_visible(&self, plan: &RouteSecurityPlan, authn:
   &KernelAuthn, headers: &http::HeaderMap) -> bool` (Public → true; Denied →
   false; Granted + Authorized → the `catalog_policy_grants` rule; Granted
   otherwise → true). `catalog_entry_visible` keeps its current behavior by
   calling `kernel_authn_for` then `plan_catalog_visible`.
4. adapter/server.rs: add
   `async fn skills_visible(&self, headers: &http::HeaderMap) -> bool`. If
   `!self.security.protected_seen()` → true. Otherwise take
   `plans_snapshot()`, keep non-`Public` plans, and walk them in order. Cache
   the authentication outcome per
   `(credential_sources, provider_ref, audience_binding, transport)` key as
   `Result<AuthenticatedPrincipal, ()>`: a `Vec` of seen keys compared with
   `==` (`CredentialSource` implements `Eq` but not `Hash`; `AudienceBinding`
   implements `PartialEq` only, so the tuple cannot be hashed; `KernelAuthn`
   is not `Clone`). The cache is local to one `skills_visible` call (its key
   carries no credential). A cached `Err(())` replays as
   `KernelAuthn::Denied(CamelError::Unauthenticated("skills catalog: cached denial".into()))`.
   The key carries every field `kernel_authenticate` reads, so two plans that
   differ only in audience binding still get distinct authentication
   outcomes. For each protected plan run the cached authentication (or
   `kernel_authn_for` once per distinct key), then `plan_catalog_visible` for
   that plan. Return true when any plan passes; no live protected plan or no
   grant → false.
5. Wire `skills_visible` into `list_skills` and `get_skill`. When visibility
   is false, `skills/list` treats the catalog as empty: no cursor returns the
   empty result with cache metadata, and any supplied cursor answers the same
   `-32602` as an empty catalog (malformed/out-of-range is checked on the
   supplied cursor). `get_skill` false → the same `invalid_params` message as
   an unknown skill.
6. Write the four tests below first and see them fail before steps 2-5.
7. Run the tests; they must pass after the wiring.

**Tests:**
- `skills_list_follows_catalog_authentication`: arrange: one `code-review`
  skill; spawn; register `handle.security.register_plan("protected",
  scoped_plan(&["skills:read"]), Some(provider_registry(&[("sec-token",
  test_principal(&["skills:read"]))])), Arc::downgrade(&owner))` with `owner`
  kept alive; act: list with no token, then with `sec-token`; assert: the
  unauthenticated result's `skills` is empty; the token caller sees one skill.
  Also call `get_skill` with no token for the SKILL.md URI and assert the error
  carries `-32602` (the unauthenticated file-read `-32602` is asserted in Task
  2.3, after the `skill://` read branch exists).
- `skills_latch_ignores_refused_duplicate_plan`: arrange: spawn with a
  one-skill root; register a `Public` plan for route `r1` under live owner A;
  register a `scoped_plan(&["skills:read"])` for the same `r1` under live
  owner B (the keep-incumbent path does not insert); act: assert
  `!handle.security.protected_seen()` and list without a token; assert: the
  latch is false and the skill is listed.
- `skills_stay_hidden_after_protected_route_stops`: arrange: spawn; register
  a protected plan, then `handle.security.unregister_plan("protected")`; act:
  list and `get_skill` without a token; assert: list `skills` is empty and
  `get_skill` answers `-32602`.
- `skills_policy_is_evaluated_per_plan`: arrange: one `code-review` skill;
  spawn; `providers = provider_registry(&[("sec-token",
  test_principal(&["skills:read"]))])`; register `scoped_plan(&["admin"])`
  under route `b` and `scoped_plan(&["skills:read"])` under route `a`, each
  with `Some(providers.clone())` and its own live owner; act: `list_skills`
  with `sec-token`; assert: one skill is listed — the `admin` plan's denial
  must not shadow the `skills:read` grant.

**Acceptance:**
- All four tests above pass; Task 2.1 tests stay green.
- `cargo test -p camel-component-mcp --test skills_extension_test` → passes.
- `cargo test -p camel-component-mcp` → passes (declared component lane,
  AGENTS.md `## COMPONENT LANES`).
- `cargo test -p camel-component-mcp --test server_tool_dispatch_test` and
  `--test server_resource_dispatch_test` → unchanged (the visibility refactor
  must preserve the existing per-entry path).
- `cargo fmt --check --all` and
  `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exit 0.

- [x] 2.2

### Task 2.3: serve skill files through `resources/read`, reject escapes and `skill://` route conflicts

**Files:**
- `crates/components/camel-component-mcp/src/adapter/server.rs` (modified)
- `crates/components/camel-component-mcp/src/consumer.rs` (modified)
- `crates/components/camel-component-mcp/tests/skills_extension_test.rs` (modified)

**Steps:**
1. adapter/server.rs: add
   `fn skill_uri_escapes(uri: &str) -> bool`: percent-decode the URI with
   `percent_encoding::percent_decode_str(uri).decode_utf8_lossy()`, split the
   decoded string on `/` and `\`, and return true when any segment is `..`.
2. adapter/server.rs `read_resource`: compute `headers` before resolution.
   Only when `self.skills` is `Some` AND `uri.starts_with("skill://")`:
   - `skill_uri_escapes(&uri)` →
     `Err(ErrorData::invalid_params(format!("skill path escapes the skills root: {uri}"), None))`.
   - `skills.file(&uri)` is `Some`: if `!self.skills_visible(&headers).await`,
     return the same `invalid_params` error as an unknown skill; otherwise
     build one `ResourceContents` from the stored bytes and MIME, reusing the
     existing `is_textual_mime` rule plus the UTF-8 check (text when both hold,
     base64 blob otherwise), and return
     `ReadResourceResponse::Complete(ReadResourceResult::new(vec![contents])
     .with_ttl_ms(0).with_cache_scope(CacheScope::Private))`.
   - no file match →
     `Err(ErrorData::invalid_params(format!("skill resource '{uri}' is not available"), None))`.
   - Every other URI (no catalog, or not `skill://`) takes the existing
     registry path unchanged, even when it carries a `..` segment.
3. consumer.rs resource branch: after `get_or_spawn`, before registration,
   when `resource_uri.starts_with("skill://")` and `handle.skills.is_some()`,
   unregister the owner's plan conditionally and return
   `McpError::Endpoint(format!("resource '{resource_uri}' conflicts with the skills catalog on this bind"))`.
4. Add the six tests below to `tests/skills_extension_test.rs`; write them
   first and see them fail before steps 1-3.

**Tests:**
- `skill_file_is_served_through_resources_read`: arrange: the `code-review`
  fixture; spawn; client; act: `peer().read_resource(ReadResourceRequestParams::new("skill://code-review/SKILL.md"))`
  and the `references/checklist.md` URI; assert: each reply text equals the
  on-disk file, `mimeType` is `text/markdown`, and the result carries
  `ttl_ms == Some(0)` and `cache_scope == Some(CacheScope::Private)`.
- `skill_file_read_follows_catalog_authentication`: arrange: the `code-review`
  fixture; spawn; register a protected plan with
  `scoped_plan(&["skills:read"])` and the `sec-token` provider principal; act:
  read `skill://code-review/SKILL.md` with no token and then with
  `sec-token`; assert: the unauthenticated read answers `-32602`, and the
  token read returns the file text.
- `escaping_symlink_fails_spawn` (`#[cfg(unix)]`): arrange: temp outer dir
  with `outside.txt`, a root with `escape/SKILL.md`, and a symlink
  `escape/leak.md` to `outside.txt`; act: `get_or_spawn` with that root;
  assert `Err(McpError::SkillPathEscape { .. })` and no listener serves the
  symlink target.
- `skill_path_escape_is_rejected`: arrange: a valid root; spawn; act: read
  `skill://code-review/../../outside.txt` and
  `skill://code-review/%2e%2e/%2e%2e/outside.txt`; assert both errors carry
  `-32602` with a message containing `escapes`. Also read `crm://x/../y`;
  assert the message is `resource 'crm://x/../y' is not available` (a
  non-`skill://` URI keeps the registry path).
- `unknown_skill_file_and_resources_list_stay_closed`: arrange: a valid root
  and no route-registered resources; spawn; client; act: read
  `skill://code-review/unknown.md` and call `list_resources`; assert: the read
  error carries `-32602`, and no listed resource URI starts with `skill://`.
- `skill_uri_route_conflicts_with_skills_root`: arrange: a component with one
  server whose config carries a valid `skills_root` and one skill `blocked`;
  create a resource consumer for
  `mcp:crm/resource/blocked?uri=skill%3A%2F%2Fblocked`; act:
  `consumer.start(ctx)`; assert: start fails with a `CamelError` whose text
  contains `skills catalog`; `get_or_spawn` for the bind has
  `skills.as_ref().map(|c| c.len()) == Some(1)`, and `read_resource` of
  `skill://blocked/SKILL.md` returns the file text.

**Acceptance:**
- The six tests above pass, and the Task 2.1/2.2 tests stay green.
- `cargo test -p camel-component-mcp --test skills_extension_test` → passes.
- `cargo test -p camel-component-mcp` → passes (declared component lane,
  AGENTS.md `## COMPONENT LANES`).
- `cargo test -p camel-component-mcp --test server_resource_dispatch_test` →
  unchanged for non-`skill://` URIs.
- `cargo xtask lint-unwrap`, `cargo xtask lint-unbounded-wait`, and
  `cargo xtask lint-test-sleep` exit 0.
- `cargo fmt --check --all` and
  `cargo clippy -p camel-component-mcp --all-targets -- -D warnings` exit 0.

- [x] 2.3

## docs

### Task 3.1: document the Skills extension

**Files:**
- `docs/src/components/mcp.md` (modified)
- `crates/components/camel-component-mcp/CONTEXT.md` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Load the `ste-writing` skill before editing the Markdown files.
2. `docs/src/components/mcp.md`: add `skills_root` to the server config list
   in `## Configuration`; update the conflicting-config sentence at line 34
   (the listener conflict list) and the TOML-only-keys sentence at line 83 to
   name `skills_root`; add a short `## Skills` section covering: opt-in
   behavior, `skills/list`/`skills/get` via rmcp's SEP-2640 surface, the
   complete manifest with `sha256` digest and size, `resources/read` serving,
   path safety, catalog authentication parity, and `ttlMs: 0` /
   `cacheScope: "private"`.
3. `crates/components/camel-component-mcp/CONTEXT.md`: add `skills_root` to
   `## Config keys`; add a Skills paragraph: opt-in extension, loader,
   manifest, auth latch (state that it keys on registered route plans and is
   not the ADR-0061 Rule 8 bind-presence gate), and the `skill://` namespace
   conflict with resource routes.
4. `CONTEXT-MAP.md`: add a `Skill Catalog` term next to Tool Registry and
   Resource Registry; update the MCP Component entry to mention skills; add
   `skills_root` to the Shared Listener conflicting-config key list.
5. Run the docs checks.

**Tests:**
- No new Rust tests. Verification commands:
  `nix shell nixpkgs#mdbook -c mdbook build docs` and
  `nix shell nixpkgs#mdbook -c mdbook test docs` exit 0;
  `cargo xtask lint-context-citations` exits 0.

**Acceptance:**
- `mdbook build docs` exit 0.
- `mdbook test docs` exit 0.
- `cargo xtask lint-context-citations` exit 0.
- The spec/code terms match: `skills_root`, `skill://`, `skills/list`,
  `skills/get`, `ttlMs`, `cacheScope`.

- [x] 3.1
