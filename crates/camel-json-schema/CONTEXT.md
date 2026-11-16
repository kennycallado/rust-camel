# JSON Schema

Runtime-free leaf crate that compiles a JSON Schema document into a reusable validator.
It is the only place in the workspace that compiles JSON Schema `$ref`s (ADR-0100).
It depends on no `camel-*` crate, so every consumer links the same confinement rules
instead of copying a wrapper.

Dependencies: `jsonschema` (workspace pin with `default-features = false`),
`serde_json`, `noyalib`, `percent-encoding`, `thiserror`.

ADR-0100 defines the decision; this file records the shipped contract.

## Consumers

Three call sites use `compile`:

- `camel-component-validator` (`compiled.rs`): `type: json-schema` validation config,
  both file-backed and `camel-embedded:` inline.
- `camel-processor` (`json_schema_validate.rs`): inline `request_schema`.
- `camel-component-mcp` (`adapter/server.rs`): inline generated tool schemas, compiled
  with `FormatMode::DraftDefault`.

A file root stays `SchemaSource::File` through compilation. Pre-parsing it to `Inline`
would reject its legitimate sibling references.

## Public surface

- `SchemaSource::{File(PathBuf), Inline(Value)}`
- `SchemaSyntax::{Auto, Json, Yaml}` for the root document. `Auto` reads the extension:
  `.yaml`/`.yml` parse as YAML, every other extension as JSON. Inline roots accept
  `Auto` and `Json`; `Yaml` is rejected because a parsed `Value` has no YAML text to parse.
- `FormatMode::{DraftDefault, Enforce, Annotate}`
- `compile(source, syntax, format) -> Result<CompiledSchema, SchemaError>`
- `CompiledSchema::violations(&Value) -> Vec<Violation>`. The order is the upstream
  validator traversal order, not a sort.
- `Violation { instance_path, message }`, formatted by consumers as
  `"{message} at {instance_path}"`.
- `SchemaError`, one variant per rejection class.

`MAX_FILE_BYTES`, `MAX_RESOURCES`, and `MAX_TOTAL_BYTES` are public constants. They are
not per-call options.

## Confinement contract

For `SchemaSource::File`, `compile` canonicalizes the root path and uses its parent
directory as the resolution root. The base URI is `file://` plus the canonical root path,
with each segment percent-encoded, so a directory name with a space or a URI-illegal
character still produces a parseable URI. Upstream resolves every reference against that
base URI, or against a `$id` rebase, before the retriever sees it. A `$id` rebase is not a
special case: it arrives as a resolved URI and passes the same checks as a direct `$ref`.

`ConfinedFileRetriever` runs these checks in order, before and around the read:

1. Scheme. The parsed URI must be `file` or empty; any other scheme is
   `SchemaError::RemoteRefRejected` before any I/O. Input that does not parse as an
   absolute URI is classified by its scheme prefix: `file` is
   `SchemaError::ExternalRefRejected`, another prefix is remote, and no prefix is treated
   as a relative path.
2. Percent-decoding. Invalid UTF-8 is `SchemaError::ExternalRefRejected`.
3. `std::fs::canonicalize`, which also resolves symlinks.
4. Containment. The canonical path must start with the canonical root directory,
   compared component-wise with `Path::starts_with`, never as a string prefix.
5. Regular file, checked from metadata before `open`, so a FIFO is rejected instead of
   blocking in `open`.
6. After `open`, the handle metadata is checked again for a regular file, then the file
   is read with a bound.

Fail closed is the rule at every step. A reference outside the boundary produces a named
error; it is never fetched and never ignored.

**Remote error literal.** `SchemaError::RemoteRefRejected` displays exactly:

```text
remote $ref '<uri>' rejected: schemas resolve from local files only (ADR-0100); vendor the file
```

Consumers preserve that literal verbatim through their own error wrapping.

## Resource budgets

| Limit | Value | Constant |
| --- | --- | --- |
| Per file | 16 MiB | `MAX_FILE_BYTES` |
| Distinct resources per compilation | 256 | `MAX_RESOURCES` |
| Total bytes per compilation | 32 MiB | `MAX_TOTAL_BYTES` |

The root document is resource 1, and its bytes count toward the total. Each distinct
external resource is counted once, deduplicated by canonical path, so repeated references
to the same file are free. `Budget` lives in one retriever, created per `compile` call,
and never spans compilations.

Reads are bounded, never metadata-only: `read_bounded` asks for `limit + 1` bytes and
rejects the resource when the extra byte arrives. A file that grows after an earlier
check therefore still trips the cap. Every cap fails closed: `SchemaError::TooManyResources`
and `SchemaError::TotalBudgetExceeded` name the limit they enforced.

## Inline-only retrieval

`SchemaSource::Inline` has no filesystem root. It compiles against the opaque base URI
`inline-schema:/`, so a fragment reference (`#/$defs/x`) resolves from the root document.
`InlineFailClosedRetriever` rejects every external reference before any I/O:
`inline-schema` and `file` targets are `SchemaError::ExternalRefRejected`, every other
scheme is `SchemaError::RemoteRefRejected`. The synthetic base URI doubles as the `root`
field of the rejection, because it is the boundary the reference escaped.

## Typed error recovery

Upstream wraps a retrieval failure in a boxed error, and the build error exposes no
`source()` chain. The boxed value therefore loses its concrete type, and the caller would
see only a generic compile message.

Both retrievers avoid that loss. Before returning the boxed error, the retriever clones
the `SchemaError` into a `CaptureSlot` (`Arc<Mutex<Option<SchemaError>>>`) shared with
`finish`. When `options.build` fails, `finish` drains the slot and returns the typed error
verbatim. When the slot is empty, the build text becomes `SchemaError::Compile`. Boxing
alone is not enough: the capture slot is what keeps the variant.

## Format mode

The root document's `$schema` selects the draft, and 2020-12 is the default. No draft is
forced.

`FormatMode::DraftDefault` leaves `should_validate_formats` unset, which reproduces
`jsonschema::validator_for` exactly: draft-07 and the format-assertion vocabulary enforce
formats, while 2020-12 without that vocabulary treats them as annotations. The MCP
consumers compile with `DraftDefault` for this reason. `Enforce` sets the flag to true;
`Annotate` sets it to false.

## Message identity and path naming

Every validation message that existed before ADR-0100 stays byte-identical. Only new
reference rejections add new text.

The two scopes name different paths:

- **Root scope.** Root-document I/O failures name the path the caller configured, as the
  pre-change literals did. `root_scope` rewrites `Open`, `Read`, and `FileTooLarge` so the
  configured path stays in the message; the per-file cap maps to `RootTooLarge`.
- **Include scope.** Reference rejections name the decoded candidate path, which is the
  path the check rejected. This matters for percent-encoded traversal: the URI form hides
  `%2e%2e%2f` behind encoding, so the message shows the decoded path.

## Workspace pin

The workspace dependency is `jsonschema = { version = "0.60", default-features = false }`.
`resolve-file` and `resolve-http` are not enabled. A bare `jsonschema::validator_for`
therefore fails closed on an external `$ref` instead of reading the file. Production
callers go through this crate.

## Known limits

- A root schema stored at the filesystem root `/` makes the parent-directory containment
  vacuous, because `/` is then the root schema directory. The other checks still apply.
  The case is accepted as a limit.
- The 16 MiB / 256 / 32 MiB caps are constants. A legitimate large schema set can be
  rejected, and no per-call override exists.
- **Residual TOCTOU risk (accepted).** The confinement checks run on the real path before
  `open`, and the regular-file check repeats on the open handle. A local writer that swaps
  a symlink at the canonical path inside that window can still redirect the read. This is
  out of the threat model: the schema tree is operator-supplied config, and a writer who
  can replace files in it already controls the schema. ADR-0100 records the risk so a
  later review does not read the gap as an oversight.

## Language

**Confined resolution**:
A reference resolves only when the canonical target stays inside the root schema
directory, is a regular file, and fits the resource budgets. Checks run on the real path
and, where applicable, again on the open handle.
_Avoid_: sandbox, jail, file access policy

**Root schema directory**:
The parent directory of the canonical root schema file. It is both the resolution root
and the boundary for containment.
_Avoid_: working directory, base directory

**Capture slot**:
The shared `Arc<Mutex<Option<SchemaError>>>` that keeps the typed retrieval failure before
upstream boxes and flattens it. `finish` drains it on build failure.
_Avoid_: error cache, error channel

**Resource budget**:
The per-compilation accounting of distinct resources and total bytes. The root document
is resource 1. It is created per `compile` call.
_Avoid_: quota, limits object

**Include-scope rejection**:
A reference rejection raised while resolving a non-root document. It names the decoded
candidate path, not the configured root path.
_Avoid_: external error, nested error

## Example dialogue

> "May a schema reference `../shared/common.json`?"
> "No. External references must stay inside the root schema's own directory. The
> rejection names the decoded path, and the remote literal names the URI."

> "Why not return the upstream build error directly?"
> "Upstream boxes the retrieval failure, so the `SchemaError` variant is lost. The capture
> slot keeps the typed error and `finish` returns it verbatim."

> "Does an inline schema differ from a file schema?"
> "Yes. An inline schema retrieves nothing: fragment references resolve from the root
> document, and every external reference fails closed."
