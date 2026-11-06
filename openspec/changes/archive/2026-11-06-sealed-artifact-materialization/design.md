# Design: sealed-artifact-materialization

## Approach

Extend the mission-350 pattern (`camel-embedded:` refs + in-process resolution) to the
three byte-consuming classes and give the disk class a writable-location fallback.

### Inventory (evidence, local main `df1533b7c`)

Single production materialization site: `camel-cli/src/compile/runtime.rs`
`prepare_embedded_assets` (boot, before route dispatch) →
`compile/materialize.rs:113` `Materialization::create()` →
`tempfile::Builder::new().prefix("camel-assets-").tempdir_in(std::env::temp_dir())`;
per-asset writes via `materialize.rs:146` `write` (`create_new`, 0600). Consumers read
the confined paths substituted into the route documents:

| Class | Write site (boot) | What it writes | Who reads it | Read time |
|---|---|---|---|---|
| `certificate` / `private key` / `client CA` | runtime.rs:748-760 + materialize.rs:113/146 | PEM cert/key/CA files under `camel-assets-*` (0700 dir, 0600 files) | `camel-component-api/src/tls_source.rs:41,47,82` (gRPC/HTTP/WS servers); `camel-http/src/tls.rs` (7 sites) + `camel-http/src/lib.rs:1641`; `camel-ws/src/lib.rs:2326`; gRPC/WS `tls_reload.rs` re-reads | endpoint creation (boot) + TLS hot-reload |
| `xslt stylesheet` | same materialization path | stylesheet file | `camel-xslt/src/component.rs:327` `read_stylesheet` (`File::open`, 16 MiB cap) → bytes to the xml bridge (`client.compile(bytes)`) | endpoint creation |
| `xsd schema` | same | root schema file (+ its import/include files, which are NOT collected and therefore missing in sealed artifacts today) | `camel-validator/src/compiled.rs` `compile_xsd` → `discover_schema_set` (`schema_set.rs`): canonicalizes the root, walks import/include under budgets `MAX_SCHEMA_BYTES` 8 MiB total transmitted schema-set bytes (alias and relative-path entries both count), `MAX_SCHEMA_RESOURCES` 256, `MAX_SCHEMA_ALIASES` 128, `MAX_IMPORT_DEPTH` 32 → bytes to the xsd bridge. (`FilesystemResolver` is JSON/YAML-only, not part of this class.) | endpoint creation |
| `sql file` | same | query file | `camel-sql/src/config.rs:738` `resolve_file_query` (`tokio::fs::read_to_string`) → inline query string | async endpoint init |

All three non-TLS consumers read **bytes** in-process — no external library needs a
path — so (b) is a one-seam change per component. The `AssetRegistry` in
`camel-bundles` cannot host the seam (it depends on the components; a component →
bundles dep would cycle), so the registry mirrors `camel-proto-compiler::embedded`
(mission-350 precedent: grpc depends on that leaf) and lives in **`camel-component-api`**
— the shared leaf all three components already depend on, same layer as
`tls_source.rs`.

### Reference spelling round trip (blessing fix 2)

`URI_SITE_SET` percent-encodes `:` (runtime.rs:370: `NON_ALPHANUMERIC` minus
`/ - . _ ~`), so a substituted reference lands in the URI text as
`camel-embedded%3A<store path>`. Each consumer's parse posture differs and the seams
must respect it:

- `camel-validator` percent-decodes the URI path exactly once at `from_uri`
  (`config.rs:81`) → the seam sees the canonical `camel-embedded:` prefix; no extra
  decode at the seam.
- `camel-xslt` parses the URI manually (`config.rs:113-143`) and performs NO decoding →
  `read_stylesheet` accepts both spellings: an ENCODED reference is decoded IN FULL
  exactly once (spaces, Unicode, colons, literal `%` sequences — the whole reference,
  not just the `%3A` separator, then UTF-8-validated and looked up); a CANONICAL
  reference's suffix is a literal registry key, never decoded; plain-path behavior
  unchanged.
- `camel-sql` uses camel-endpoint's `parse_uri`, which percent-decodes the path
  exactly once (`uri.rs:147`) → the canonical spelling arrives at
  `resolve_file_query` through `from_uri`; `resolve_ref` still accepts both spellings
  so directly-placed `source_path` values resolve either way.
- (mission-350 precedent: grpc consumes the value via `url::Url` query pairs, which
  decode, so `proto_path_is_acceptable` sees the canonical prefix.)
- **Fail-closed prefix rule** (plan-review fix): at every seam, a value carrying
  either prefix spelling that resolves to NO registered asset fails with a typed
  error naming the reference — the host filesystem is never probed for prefixed
  values (the reservation shadows real files named with the prefix; a probe would
  let a decoy file inject bytes into a sealed artifact). Only prefix-less values
  reach the filesystem path.

### Binding per-class decisions

- **`xslt stylesheet`, `xsd schema`, `sql file` → (b) in-memory.** New
  `camel_component_api::embedded_assets` (`EMBEDDED_REF_PREFIX = "camel-embedded:"`,
  `register`/`get`/`resolve_ref` (the last branches on the prefix spelling: a
  CANONICAL value is a literal registry key after prefix removal — no suffix decode;
  an ENCODED `%3A` value is percent-decoded IN FULL exactly once, UTF-8-validated,
  then looked up — store paths may contain spaces, Unicode, colons, and literal `%`
  sequences; subsumes the proto module's `strip_ref`; no `contains` — no consumer
  needs it), shape adapted from proto-compiler). Boot
  (`prepare_embedded_assets`) registers each verified asset's bytes under its store path
  and rewrites the site to the reference (URI sites only, like proto: the literal
  charset clause fail-closes on `:`); the component seam resolves the reference before
  any filesystem attempt, applying its existing size cap. Plain-path boots are unchanged;
  unknown references fail closed with the component's normal error shape. The prefix
  string is defined in both leaf crates with a camel-cli consistency test (the crates
  cannot reference each other without inverting layers). The reservation is already
  workspace-wide (proto-compiler embedded.rs docs). Reference-spelling round trip per
  the section above (validator and sql decode at parse; xslt decodes exactly once at
  the seam).
  - **XSD specifics**: the seam is registry-backed ROOT discovery in the
    `compile_xsd`/`discover_schema_set` path (NOT `FilesystemResolver`, which serves
    JSON/YAML). The root schema resolves from the registry; import/include resolution
    keeps the existing budgets (8 MiB total transmitted schema-set bytes — alias and
    relative-path entries both count toward it — 256 resources, 128 aliases, 32-level
    import depth). An embedded root that carries an unresolvable import/include fails
    with a typed, named error naming the missing resource — sealed artifacts never
    embedded import files, so import-sets are a documented prerequisite with a clear
    failure (strictly clearer than today's missing-file error at the materialized
    path). Standalone XSDs boot fully in memory.
- **TLS family → (a), residual (c).** `Materialization::create()` tries, in order:
  `std::env::temp_dir()`, then the artifact-adjacent directory (`current_exe()` parent);
  first candidate that accepts the 0700 dir wins (same `TempDir` drop-cleanup). Test
  seam: `create_with_candidates(Vec<PathBuf>)`, wrapped by `create()`; the boot-level
  seam is `prepare_embedded_assets_with_candidates` (pub(crate)) so tests inject without
  env mutation. When every candidate fails, boot fails with the existing typed
  `PrepareError::TempUnavailable { class, source }` (c) — the Display names both
  attempted locations, the class, and the requirement with a PATH-FREE OS cause
  derived from `source.kind()`: tempfile 3.27.0 wraps failures in `PathError` whose
  Display prints the attempted path, so the raw source is never interpolated into the
  user-facing message (it stays reachable through `Error::source()`).
  (b) for TLS is out of bounds this mission: 14+ read sites
  across camel-http/ws/grpc plus hot-reload re-read semantics; bd follow-up records it.

### Docs

`compile.md`: xslt/xsd/sql resolve in memory (no extraction); TLS materializes into a
confined per-boot dir under the temp dir, falling back to the artifact directory, and a
fully read-only host fails boot with a typed named error. `CHANGELOG.md`: one fix entry.
`camel-cli/CONTEXT.md` owns the sealed-boot story and gets the per-class table.

## Affected crates

- `camel-component-api`: new `embedded_assets` module (leaf; one new workspace
  dependency: locked `percent-encoding`, already used across the workspace).
- `camel-cli`: boot partition (three classes move to in-memory), materialization
  fallback chain + candidates seam, prefix-consistency test.
- `camel-xslt`: `read_stylesheet` embedded-ref seam.
- `camel-validator`: registry-backed root discovery in the `compile_xsd` →
  `discover_schema_set` path (the XML seam; `FilesystemResolver` serves JSON/YAML and
  is untouched).
- `camel-sql`: `resolve_file_query` embedded-ref seam.
- Docs: `docs/src/cli/compile.md`, `CHANGELOG.md`, `camel-cli/CONTEXT.md`.

## Architecture boundaries

Runtime/boot logic stays in `camel-cli` (the only place that decodes artifacts).
Components keep depending only on ports/leaf infra (`camel-component-api`) — no
component gains a camel-core or camel-bundles dependency (`lint-component-deps`,
hexagonal boundaries test). The registry is a component-api port-level facility,
same layer as `tls_source.rs`. No DSL, Services, Languages, or Functions surface
changes.

## Phases

Single-phase change: one coherent slice (seam + partition + fallback + seams' consumers
+ docs), implemented and gated together.
