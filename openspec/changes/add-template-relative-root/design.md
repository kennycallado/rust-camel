# Design: add-template-relative-root

## Context

Today the external template component accepts exactly one URI shape,
`template:file:///<abs-path>` (`uri.rs::parse_template_uri`). Endpoint
construction is parse-only (no filesystem access); `lifecycle.rs::start()`
opens the entry's parent directory as the confinement root and every
closure read is `openat`-relative to that root handle
(`path_util::open_root` / `OwnedHandle::open_relative`, O_NOFOLLOW).
The closure walk (`closure.rs::acquire_closure`) is already fully
root-relative: it takes a root handle plus a root-relative entry name.

That existing shape makes the feature small: the machinery only needs a
configured root to replace the `entry.parent()` anchor, plus a relative
entry form.

## Goals / Non-Goals

Goals:
- `[components.template] root` config (kebab-case, deny unknown fields).
- One canonical relative entry URI form, resolved inside the canonical root.
- Root-configured confinement: `..`, absolute escape, symlink escape,
  missing root/entry all fail with clear errors.
- Absolute form behavior preserved bit-for-bit when no root is configured.

Non-Goals:
- Reload/watcher (rc-hasr3), render diagnostics/filters, inline minijinja,
  HTTP static serving, autoescape contract changes.

## Decisions

### D1: Canonical relative URI form is `template:file:<rel-path>`

After `template:file:`, a `//` prefix selects the existing absolute branch
(empty authority, absolute path, no `..` — unchanged). Anything else selects
the new relative branch: the remainder must be non-empty and must not
contain an empty component (double slash), a leading `/` (impossible in this
branch), or a `.`/`..` component. `template:file:./x.html` is therefore a
config error, not a tolerated spelling. One canonical form keeps validation
tight and error messages exact; documentation states the form once.

`uri.rs::TemplateEndpointConfig` gains a `TemplateEntry` enum
(`Absolute(PathBuf)` | `Relative(PathBuf)`) replacing the lone
`entry_abs_path` field (enum-over-Option per ADR-0091 style norms).
Construction stays parse-only: the relative form does NO filesystem access
at construction, matching the existing spec requirement.

### D2: `root` semantics — configured root anchors every endpoint

`TemplateBundleConfig` gains `#[serde(default)] root: Option<PathBuf>`
(`root` needs no kebab rename). A relative `root` value resolves against
the process CWD at route start, then canonicalizes; a missing root
directory fails with a clear `CamelError` naming the configured value.

Precedence matrix (documented in README + docs page):

| root configured | URI form  | behavior |
|-----------------|-----------|----------|
| no              | absolute  | today's behavior: anchor = entry parent (unchanged) |
| no              | relative  | `CamelError::Config` at construction |
| yes             | absolute  | entry canonicalized; must land inside canonical root |
| yes             | relative  | entry = canonical_root.join(rel); canonicalized; inside check |

Rationale for "yes + absolute validates against root": the configured root
is the operator's declared trust boundary; letting an absolute URI anchor
somewhere else would silently bypass it. This is stricter, never looser,
and zero-regression (the field is new; no existing deployment sets it).

### D3: Resolution happens in lifecycle `start()`, not at construction

`start()` already owns all filesystem work. It branches on a
`RootAnchor` enum carried by `TemplateEndpoint`:

- `EntryParent` — current code path, untouched (no canonicalization added).
- `ConfiguredRoot(PathBuf)` — validation then confined open:

  1. Canonicalize the configured anchor; missing directory → clear error
     naming the configured value.
  2. Determine the entry's ORIGINAL root-relative spelling:
     - relative URI → the URI rel path as written;
     - absolute URI → the absolute path stripped LEXICALLY against the
       canonical root. A strip that fails (path only reaches the root's
       target through a different spelling) is a clear config error:
       spell the entry relative to the configured root or its canonical
       spelling. This keeps the enforcement layer possible at all.
  3. Canonicalize the full candidate entry path; missing → clear error;
     canonical entry outside the canonical root → escape error naming
     both sides. This is the VALIDATION layer: it produces precise
     "outside the root" diagnostics.
  4. Open the root handle, then open the entry's ORIGINAL root-relative
     components through `OwnedHandle::open_relative` (O_NOFOLLOW). This
     is the ENFORCEMENT layer: leaf and intermediate in-root symlinks are
     rejected exactly like closure includes. Canonicalization never
     replaces the confined open — canonical names would erase the symlink
     components O_NOFOLLOW exists to reject (e_bless finding, fixed).
  5. Environment keying and `acquire_closure` use the ORIGINAL
     root-relative entry name, which MAY contain directories
     (`templates/page.html`). Today `lifecycle.rs`, `closure.rs`
     (`build_snapshot`), and `reload.rs` all reduce the entry to
     `file_name()`; each becomes a parameter carrying the full
     root-relative name. `EntryParent` mode keeps passing `file_name()`
     (identical behavior). Regression scenario: nested entry
     `templates/page.html` alongside a DIFFERENT root-level `page.html`
     must load the nested file.

The include/extends closure itself needs no confinement changes — it
already walks `openat`-relative to the passed root handle; only the
root anchor and the entry name change.

`TemplateEndpoint::new` grows a `RootAnchor` parameter (pub(crate) call
sites only). `TemplateComponent` gains the configured root via a
`with_root` builder, mirroring the existing `default-autoescape`
plumbing; `TemplateComponent::new` keeps its signature (root = None) so
existing callers/tests stay valid.

### D4: Error taxonomy

All new failures are `CamelError::Config` at construction (relative URI
without root) or lifecycle start errors naming the offending path
(missing root, missing entry, `..` in URI, canonical entry outside root).
No new error variants; messages name the configured value and the
canonical root so operators see both sides of the failed containment.

## Phases

Single-phase change: config + URI form + resolution + integration tests +
docs form one coherent slice touching one crate. No phase grouping.

## Risks / Trade-offs

- `[trust-boundary]` A relative root resolved against process CWD depends
  on where the process starts; documented explicitly (docs say: run from
  the project directory or configure an absolute root).
- Canonicalize-before-open adds two syscalls at startup only (hot path
  unaffected — render stays compiled-state only).
- `TemplateEndpointConfig` field type changes are pub(crate); no external
  API break.
- Absolute entries lexically under the canonical root only: a path spelled
  through a different symlink alias of the root is rejected with guidance.
  Accepting it would require canonical-name opens, which erase symlinks
  (rejected in D3); the strict spelling rule is documented.

## Migration Plan

Additive config + additive URI form. No operator action needed; existing
routes behave identically. Docs describe opt-in `root`.

## Open Questions

None — mission 461 fixes the requirements; D1 canonical-form choice is
the only free variable and is decided above.
