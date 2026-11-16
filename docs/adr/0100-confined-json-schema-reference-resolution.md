# ADR-0100: Confined JSON Schema reference resolution

- Date: 2026-11-16
- Status: Accepted (2026-11-16)
- Context: change `schemaconf`; bd `rc-t5mgq`
- Amends: none
- Cross-refs: [ADR-0026](0026-json-canonical-route-authoring-format.md) (JSON authoring
  format), [ADR-0032](0032-exchange-data-trust-boundary.md) (exchange-data trust
  boundary), [ADR-0033](0033-security-defaults-fail-closed-startup-validation.md)
  (fail-closed config posture), [ADR-0041](0041-component-metadata-capabilities-schema.md)
  (capabilities schema), [ADR-0091](0091-rust-style-policy.md) (Rust style policy)

## Context

Three crates compiled JSON Schema with `jsonschema` directly:

- `camel-component-validator`: `compiled.rs` compiles `type: json-schema`
  validation config, both file-backed and `camel-embedded:` inline.
- `camel-processor`: `json_schema_validate.rs` compiles `request_schema`
  (inline only).
- `camel-component-mcp`: `adapter/server.rs` calls `validator_for` on generated
  inline tool schemas.

Each call site carried its own wrapper and its own copy of the same comments.

**The measured hole.** The workspace pin enabled the `jsonschema`
`resolve-file` feature. Under that pin, every external `file:` reference went
to the built-in `DefaultRetriever`, which does this:

```text
let file = std::fs::File::open(uri.path())?;
Ok(serde_json::from_reader(file)?)
```

The path is used exactly as the resolved URI spells it. The retriever
canonicalizes nothing, checks no root, resolves no symlink, applies no per-file
or per-compilation cap, and re-checks nothing after a `$id` rebase. A root
schema in `routes/` that references `../../../etc/passwd` or
`file:///etc/hostname` is read. A `$id` of `https://example.com/base.json`
moves every later relative reference onto the remote base, so the reference
leaves the directory the operator shipped without passing a check.

A schema file is operator-supplied config, but it is often vendored from a
third party. The reference graph inside it is not reviewed to the same standard
as the route that names the file. Confinement is therefore a property of the
compiler, not a convention at each call site.

**Relative-ref ergonomics.** Real schema sets ship as a directory tree. The
normal authoring shape is a root document with no `$id` that references
siblings:

```yaml
properties:
  order:
    $ref: "defs/order.json#/$defs/order"
```

Rejecting every external reference would force one giant file, or a fabricated
`$id` per file. Both are worse for authors. Resolution needs the real root file
as base URI plus a retriever that keeps the result inside the schema directory.
A base URI alone is not a boundary: `$id` overrides it.

## Decision

### 1. One leaf crate owns JSON Schema compilation

`crates/camel-json-schema` is the only place in the workspace that compiles
JSON Schema references. It depends on `jsonschema`, `serde_json`, `noyalib`
(YAML) and `thiserror`, and on no `camel-*` crate.

Public surface ([ADR-0091](0091-rust-style-policy.md) style: enums over bool,
named fields):

- `SchemaSource::{File(PathBuf), Inline(Value)}`
- `SchemaSyntax::{Auto, Json, Yaml}` for the root document
- `FormatMode::{DraftDefault, Enforce, Annotate}`
- `compile(source, syntax, format) -> Result<CompiledSchema, SchemaError>`
- `CompiledSchema::violations(&Value) -> Vec<Violation>`
- `SchemaError` with one variant per rejection class

A file root stays `File` through compilation. Pre-parsing it into `Inline`
would break legitimate sibling references.

### 2. A base URI plus a confined retriever

For `SchemaSource::File`, the crate canonicalizes the root path and uses its
parent directory as the resolution root. The base URI is
`file://<canonical root path>`. `file_uri()` percent-encodes each path segment,
so schema paths that contain spaces or URI-illegal characters still compile.

Compilation passes that base URI through `with_base_uri` and installs a custom
retriever through `with_retriever`. Upstream resolves each reference against
the base URI, and against any `$id` rebase, before the retriever sees it.
`ValidationOptions::with_retriever` stores the custom retriever in place of the
built-in one, so the built-in `resolve-file` path is unreachable on every
compilation this crate performs.

### 3. Checks run on the resolved URI and the real file path

The retriever classifies the resolved URI, then checks the file on disk, in
this order:

1. The scheme must be `file` or empty. Any other scheme is
   `RemoteRefRejected`, before any I/O. Input that does not parse as an
   absolute URI is classified by its scheme prefix: `file` is
   `ExternalRefRejected`, any other prefix is remote, and no prefix is treated
   as a relative path.
2. The path component is percent-decoded.
3. `std::fs::canonicalize` resolves the path, which also resolves symlinks.
4. Containment: the canonical path must start with the canonical root schema
   directory, compared component-wise (`Path::starts_with`), never as a string
   prefix.
5. The target must be a regular file, checked from metadata before `open`, so a
   FIFO is rejected instead of blocking.
6. After `open`, the handle's metadata is checked again for a regular file, and
   then the file is read with a bound.

A `$id` rebase is not special. It arrives as a resolved URI and passes the same
checks as a direct `$ref`. Percent-encoded traversal (`%2e%2e%2f`) is rejected
with the decoded path in the message, because the URI form hides traversal
behind encoding.

### 4. Resource caps and bounded reads

| Limit | Value |
| --- | --- |
| Per file | 16 MiB (`MAX_FILE_BYTES`) |
| Distinct resources per compilation | 256 (`MAX_RESOURCES`) |
| Total bytes per compilation | 32 MiB (`MAX_TOTAL_BYTES`) |

The root document is resource 1, and its bytes count toward the total. Each
distinct external resource is counted once, deduplicated by canonical path, so
repeated references to the same file are free. The budget is created per
`compile` call and never shared across compilations.

Reads are bounded: the reader asks for `limit + 1` bytes and rejects the
resource once the extra byte arrives. A metadata-only length check is never
used, because a file can grow after that check. Every cap fails closed.

### 5. Inline schemas retrieve nothing

`SchemaSource::Inline` has no filesystem root. It compiles against the opaque
base URI `inline-schema:/`. Fragment references (`#/$defs/x`) resolve from the
root document. Every external reference fails closed, classified by the
resolved URI's scheme: `inline-schema` and `file` are `ExternalRefRejected`,
every other scheme is `RemoteRefRejected`.

### 6. The workspace pin drops `resolve-file`

The workspace dependency is now:

```toml
jsonschema = { version = "0.60", default-features = false }
```

`resolve-http` is never enabled. The consequence is intended: a bare
`jsonschema::validator_for` in the workspace now fails closed on an external
`$ref` instead of reading the file. Production callers go through this crate.
The remaining bare `validator_for` callers compile inline schemas, which have
no external references.

### 7. Remote references use one stable error literal

```text
remote $ref '<uri>' rejected: schemas resolve from local files only (ADR-0100); vendor the file
```

The literal is preserved verbatim through consumer error wrapping. It applies
to a direct remote `$ref` and to a remote `$id` rebase alike.

### 8. Draft and format defaults stay native

The root document's `$schema` selects the draft, and 2020-12 is the default. No
draft is forced. `FormatMode::DraftDefault` leaves `should_validate_formats`
unset, which reproduces `validator_for` exactly: draft-07 and the
format-assertion vocabulary enforce formats, while 2020-12 without that
vocabulary treats them as annotations. The MCP consumers compile with
`DraftDefault` for this reason. `Enforce` and `Annotate` mirror the validator's
existing `validate_formats` flag.

### 9. Message identity is kept

Existing validation messages stay byte-identical. `Violation` is still
formatted as `"{message} at {instance_path}"`. Root-scope I/O errors name the
configured path, exactly as the pre-change literals did. Include-scope
rejections name the decoded candidate path, which is the path the check
rejected.

## Consequences

Easier:

- A bare `validator_for` fails closed. An inline-schema caller cannot
  accidentally read a file.
- One implementation serves all three consumers. The twin wrappers and their
  duplicated comments are gone.
- Relative references work without a fabricated `$id`, because the canonical
  root file is the base URI.
- A traversal attempt, a symlink escape, a FIFO and an oversized file all
  produce a named error, and each path-based rejection names the offending
  path. The two budget rejections name the limit.

Harder:

- A schema set that needs a file must vendor that file under the root schema
  directory. A reference to a file outside the tree is now an error, not a
  fetch.
- New reference rejections are new message text. An operator who hits one sees
  a message that did not exist before. Existing validation messages are
  unchanged.
- A root schema stored at the filesystem root (`/`) makes the parent-directory
  containment vacuous, because that directory is the root schema directory.
  The other checks still apply, and the case is accepted as a limit.
- The 16 MiB / 256 / 32 MiB caps can reject a legitimate large schema set. The
  caps are constants in `camel-json-schema`, not per-call options.

**Residual TOCTOU risk (accepted).** The confinement checks run on the real
path before `open`, and the regular-file check repeats on the open handle. A
local writer that swaps a symlink at the canonical path inside that window can
still redirect the read. This is out of the threat model: the schema tree is
operator-supplied config, and a writer who can replace files in it already
controls the schema. The risk is recorded here so a later review does not read
the gap as an oversight.
