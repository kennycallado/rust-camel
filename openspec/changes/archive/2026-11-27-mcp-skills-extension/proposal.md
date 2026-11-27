# Proposal: mcp-skills-extension

## Why

The MCP Skills extension (`io.modelcontextprotocol/skills`, protocol revision
`2026-07-28`) lets a server publish agent skills as a directory of files. A host
discovers skills once, receives a complete file manifest with digests, and reads
files through `resources/read`. Without this extension, a rust-camel MCP server
can expose tools and resources but cannot publish skills.

The `mcp-component` spec currently promises exactly the Tools and Resources
primitives and declines every other primitive. This change adds Skills as a
negotiated, off-by-default extension. A plain `mcp:` configuration keeps today's
contract.

Bd: rc-xh07d.

## What Changes

- Add `skills_root` to `[mcp.servers.<name>]` (`McpServerConfig`). The value is
  an optional directory path. Absent root → no extension.
- When a root is configured, the listener loads every skill directory at spawn:
  `SKILL.md` frontmatter, plus every supporting file with a `sha256:` digest and
  byte size. A load failure stops the listener start with a typed error.
- `server/discover` declares `resources` plus
  `extensions["io.modelcontextprotocol/skills"] = { "directoryRead": false }`
  when a root is configured.
- Implement `skills/list` and `skills/get` through rmcp's typed SEP-2640
  handlers. List results page by `cursor`/`nextCursor`; every entry is atomic
  and complete.
- Serve skill files read-only through `resources/read` at `skill://` URIs.
- Reject path escapes with a typed error: a symlink whose canonical target is
  outside the skills root fails the load, and a request URI with a `..` segment
  answers `-32602`.
- Reuse the `tools/list` catalog authentication path: skills are public only
  while the bind has never held a protected route plan in this listener's
  lifetime, and require a credential that satisfies a live protected plan
  otherwise.
- Emit `ttlMs: 0` and `cacheScope: "private"` on `skills/list`, `skills/get`,
  and skill-file `resources/read`.

Explicitly excluded:

- `resources/directory/read` (`directoryRead` stays `false`).
- Dynamic manifests (`"dynamic"`); every manifest is a fixed file list.
- DSL `mcp:` block support for `skills_root`; the TOML server config owns it in
  this slice.
- Changes to `resources/list`: skill files appear only in the `skills/list`
  manifest, not in the resource catalog.
- New dependencies. The crate uses `sha2` and `noyalib` (already in the
  workspace and lockfile) plus rmcp's typed SEP-2640 handlers;
  `on_custom_request` stays unimplemented.

## Acceptance criteria

- The seven mission-named tests pass, including capability absence without a
  root, complete manifests with digests, byte-exact `resources/read` content,
  symlink and `..` rejection, catalog authentication, and cursor pagination.
- Without `skills_root`, `server/discover` capabilities and every method reply
  match today's behavior.
- The relevant gates in `AGENTS.md` pass; `openspec validate` passes.
- The spec, plan, and holistic review carry recorded verdicts.

## Risk budget

Accepted: one new startup failure mode (bad skills root or invalid skill) that
stops the listener with a named error. Accepted: `skill://` becomes an
extension-owned namespace while a root is configured. Out of bounds: serving
bytes outside the configured root, advertising the extension without a root,
changing behavior for plain `mcp:` configurations, and adding a crate
dependency.
