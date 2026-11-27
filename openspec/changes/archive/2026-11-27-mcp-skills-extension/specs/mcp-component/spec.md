## ADDED Requirements

### Requirement: MCP Skills extension is an opt-in server capability

The MCP server SHALL accept an optional `skills_root` directory on
`McpServerConfig`. When the field is absent, the server SHALL NOT load skills,
SHALL NOT advertise the extension, and SHALL answer `skills/list` and
`skills/get` as unsupported methods. When the field is present, the listener
SHALL load the skills catalog at spawn, before binding. A missing root, a root
that is not a directory, an unreadable file, or an invalid skill SHALL stop the
start with a typed `McpError` and SHALL NOT bind a listener or serve a partial
catalog. Two named servers that share a bind SHALL declare the same
`skills_root` or the later start SHALL fail with a named conflict.

A skill SHALL be a direct child directory of the root that contains `SKILL.md`.
Other direct children SHALL be ignored.

#### Scenario: no skills root leaves the extension off

- **GIVEN** a started MCP server whose configuration declares no `skills_root`
- **WHEN** a host issues `server/discover`
- **THEN** the capabilities carry Tools and Resources and no
  `io.modelcontextprotocol/skills` extension entry
- **AND** `skills/list` answers method not found

#### Scenario: a skills root advertises the extension

- **GIVEN** a server configuration with a `skills_root` directory
- **WHEN** a host issues `server/discover`
- **THEN** the capabilities carry Resources and
  `extensions["io.modelcontextprotocol/skills"]` with `directoryRead` set to
  `false`

#### Scenario: an invalid root fails listener start

- **GIVEN** a server configuration whose `skills_root` is missing or is not a
  directory
- **WHEN** the consumer starts
- **THEN** start fails with a typed `McpError` naming the root
- **AND** no listener is bound

#### Scenario: a conflicting root on a shared bind is rejected

- **GIVEN** a live listener on a bind with one `skills_root`
- **WHEN** a second named server starts on the same bind with a different
  `skills_root`
- **THEN** the second start fails with a named conflict
- **AND** the first listener keeps serving its own catalog

### Requirement: MCP Skills manifest is complete and immutable

`SKILL.md` SHALL begin with YAML frontmatter that carries string `name` and
`description` fields. The name SHALL be 1 to 64 characters of `[a-z0-9-]`, with
no leading or trailing hyphen and no double hyphen. The description SHALL be 1
to 1024 characters. The skill directory name SHALL equal `name`. A frontmatter
value that cannot render faithfully as JSON SHALL fail the load.

For each served skill, the server SHALL publish a complete manifest of every
regular file under the skill directory. Duplicate frontmatter keys SHALL fail
the load. Each manifest entry SHALL carry the resource URI, the `sha256:`
digest of the bytes the server will return, and the raw byte size. The entry
SHALL also identify the skill's `SKILL.md` URI and its
YAML frontmatter, with every frontmatter field preserved. URIs SHALL follow
`skill://<skill-directory>/<relative-path>` with percent-encoded path segments.
Manifests and skill order SHALL be deterministic. Each skill entry SHALL be
atomic: a paginated `skills/list` SHALL NOT split one skill's manifest across
pages. A skill with more than 512 files or more than 16,777,216 total bytes
SHALL fail the load.

#### Scenario: manifest lists every file with digest and size

- **GIVEN** a skill directory with `SKILL.md` and a supporting file
  `references/checklist.md`
- **WHEN** a host issues `skills/list`
- **THEN** the entry lists both URIs, each with a `sha256:` digest and byte size
  that match the served bytes

#### Scenario: frontmatter is returned unchanged

- **GIVEN** a `SKILL.md` whose frontmatter carries `name`, `description`, and an
  extra field
- **WHEN** a host issues `skills/list`
- **THEN** the entry carries all three fields with their declared values
- **AND** the entry URI names the skill's `SKILL.md`

#### Scenario: an invalid skill fails listener start

- **GIVEN** a `SKILL.md` without frontmatter, without `name` or `description`, a
  name that breaks the naming rule, a directory name that differs from `name`,
  duplicate frontmatter keys, a frontmatter value that cannot render as JSON
  (for example `.inf`), or a skill that exceeds the file or byte limit
- **WHEN** the consumer starts
- **THEN** start fails with a typed `McpError` naming the offending skill
- **AND** no listener is bound

### Requirement: MCP Skills listing and retrieval methods

The server SHALL implement `skills/list` and `skills/get` as MCP requests.
`skills/list` SHALL accept an optional `cursor` and SHALL return
`resultType: "complete"`, the `skills` array, `ttlMs`, `cacheScope`, and a
`nextCursor` when another page exists. A skill is never split across pages.
`skills/get` SHALL accept a skill URI and SHALL return `resultType: "complete"`,
`skill` with the same shape as the list entry, `ttlMs`, and `cacheScope`. The
server SHALL emit `ttlMs: 0` and `cacheScope: "private"` on both methods. A
malformed cursor, an out-of-range cursor, or an unknown skill URI SHALL answer
JSON-RPC `-32602` (Invalid params).

#### Scenario: list carries the required result shape and cache metadata

- **GIVEN** a started server with two skills
- **WHEN** a host issues `skills/list`
- **THEN** the result carries `resultType: "complete"`, two complete skill
  entries, `ttlMs: 0`, and `cacheScope: "private"`

#### Scenario: get by URI matches the list entry

- **GIVEN** a skill entry returned by `skills/list`
- **WHEN** a host issues `skills/get` for that entry's URI
- **THEN** `result.skill` equals the list entry
- **AND** the result carries `resultType: "complete"`, `ttlMs: 0`, and
  `cacheScope: "private"`

#### Scenario: pagination returns every skill exactly once

- **GIVEN** more skills than one page holds
- **WHEN** a host follows `nextCursor` until it is absent
- **THEN** every skill appears exactly once across the pages
- **AND** every entry is complete on the page that carries it

#### Scenario: bad cursor and unknown skill URI are invalid params

- **GIVEN** a started server with a skill root
- **WHEN** a host issues `skills/list` with a malformed or out-of-range cursor,
  or `skills/get` for a URI the server does not serve
- **THEN** the server answers JSON-RPC `-32602`

### Requirement: MCP Skills file reads and path safety

Skill files SHALL be served read-only through `resources/read`. The server
SHALL return the stored bytes, a MIME type derived from the file extension, and
`ttlMs: 0` plus `cacheScope: "private"`. The server SHALL canonicalize every
loaded path against the canonical skills root and SHALL require the canonical
target to be a regular file. A symlink or path whose canonical target lies
outside the root SHALL be rejected with a typed `McpError` and SHALL never be
served. A request URI that contains a `..` path segment, raw or percent-encoded,
SHALL answer JSON-RPC `-32602` and SHALL never reach the filesystem. An unknown
skill file URI SHALL answer `-32602`. `resources/list` SHALL NOT add skill files
to the resource catalog.

#### Scenario: skill files are served through resources/read

- **GIVEN** a started server with a skill root
- **WHEN** a host reads the skill's `SKILL.md` and supporting file URIs
- **THEN** each reply carries the file bytes, `text/markdown` for Markdown
  files, `ttlMs: 0`, and `cacheScope: "private"`

#### Scenario: an escaping symlink is rejected at load

- **GIVEN** a skill directory that contains a symlink whose canonical target is
  outside the skills root
- **WHEN** the consumer starts
- **THEN** start fails with `McpError::SkillPathEscape`
- **AND** no listener is bound

#### Scenario: a dot-dot request URI is rejected

- **GIVEN** a started server with a skill root
- **WHEN** a host issues `resources/read` for a URI with a raw or
  percent-encoded `..` segment that points outside the root
- **THEN** the server answers `-32602` and no file outside the root is returned

#### Scenario: unknown skill file and resource listing stay closed

- **GIVEN** a started server with a skill root and no route-registered resource
- **WHEN** a host reads an unknown `skill://` URI and issues `resources/list`
- **THEN** the read answers `-32602`
- **AND** the list does not carry the skill files

### Requirement: MCP Skills catalog follows caller authentication

The server SHALL apply the same kernel catalog rule to skills that it applies to
`tools/list`: the route plans registered on the bind, credential extraction from
the normalized inbound headers, and `kernel_authenticate`. The bind SHALL record
whether it has ever committed a non-`Public` plan for the process lifetime,
across listener respawns. Until that first commitment, skills SHALL be public.
After it, skills SHALL be listed only when the request's credential is admitted
by at least one live non-`Public` plan, including the request-independent
authorization rule; if no live protected plan remains, the catalog SHALL stay
hidden. A stopped protected route SHALL NOT make the catalog public again. A
hidden skill SHALL be omitted from `skills/list`, and `skills/get` and
`resources/read` SHALL answer `-32602` with the same message as an unknown
skill. A denial, a missing provider snapshot, or an evaluation error SHALL hide
the catalog (fail closed).

#### Scenario: policy-less bind lists every skill

- **GIVEN** a bind that has never committed a non-Public route plan
- **WHEN** a caller with no credential issues `skills/list`
- **THEN** every loaded skill is listed

#### Scenario: unauthenticated caller sees no skills on a protected bind

- **GIVEN** a bind with a non-Public plan and no credential on the request
- **WHEN** the caller issues `skills/list`
- **THEN** the result carries an empty skills array
- **AND** `skills/get` and `resources/read` for a loaded skill answer `-32602`

#### Scenario: authenticated caller sees the catalog

- **GIVEN** a bind whose non-Public plan admits a valid credential
- **WHEN** the caller sends that credential with `skills/list`
- **THEN** every loaded skill is listed

#### Scenario: a stopped protected route does not reopen the catalog

- **GIVEN** a bind that registered a non-Public plan and later lost it with the
  route's stop
- **WHEN** a caller with no credential issues `skills/list`
- **THEN** the result stays empty

## MODIFIED Requirements

### Requirement: Resource URI addressing

A declared resource SHALL carry an MCP resource URI (operator config). The
`McpResourceRegistry` SHALL map that URI to the resource route.
`resources/list` SHALL advertise the registered URIs. `resources/read {uri}`
SHALL resolve the URI through the registry, send an `McpResourceRead` into the
bound route, and return the route's body. A `resources/read` for an unknown
URI or a stopped resource route SHALL return a clean MCP error and SHALL NOT
create an Exchange.

While a skills root is configured, the Skills catalog SHALL own the `skill://`
namespace: manifest URIs SHALL be served from the catalog before registry
resolution, and a resource route declaring a `skill://` URI on that listener
SHALL fail consumer start with a `CamelError`. One URI has exactly one owner.

Resource registration SHALL follow the same owner-liveness scoping as tool
registration. A resource URI whose owner is dead SHALL be replaceable by a
new registration. A late `stop()` SHALL NOT remove a successor's resource
entry. `resources/list` SHALL NOT advertise a URI whose owner is dead.

#### Scenario: resources list advertises URIs

- **GIVEN** a started server with a resource route declaring URI
  `crm://customers`
- **WHEN** a host issues `resources/list`
- **THEN** the response includes `crm://customers`

#### Scenario: resource read by URI dispatches to the route

- **GIVEN** a started resource route for URI `crm://customers`
- **WHEN** a host issues `resources/read {uri: "crm://customers"}`
- **THEN** the read is routed to the bound route and the route's body is
  returned

#### Scenario: unknown resource URI rejected

- **GIVEN** a started server with no resource registered at `crm://unknown`
- **WHEN** a host issues `resources/read {uri: "crm://unknown"}`
- **THEN** the call returns a clean MCP error and no Exchange is created

#### Scenario: stopped resource route rejected

- **GIVEN** a resource route for URI `crm://customers` that has stopped and
  unregistered
- **WHEN** a host issues `resources/read {uri: "crm://customers"}`
- **THEN** the call returns a clean MCP error and no Exchange is created

#### Scenario: dead-owner resource URI is replaced on restart

- **GIVEN** a resource consumer for URI `crm://customers` that was dropped
  without `stop()`
- **WHEN** a new resource consumer for the same URI starts in the same
  process
- **THEN** registration succeeds, and a `resources/read` dispatches to the
  new route

#### Scenario: a skill URI route conflicts with a skills root

- **GIVEN** a live listener with a configured skills root
- **WHEN** a resource consumer declares a `skill://` URI on that bind
- **THEN** consumer start fails with a `CamelError`
- **AND** catalog reads keep serving the skill files

### Requirement: v1 protocol surface

The system SHALL support exactly one transport — Streamable HTTP — and exactly
two base MCP primitives — Tools (full) and Resources (read-on-demand) — plus one
negotiated extension: Skills. The Skills extension SHALL be advertised only when
a skills root is configured; without a root the server advertises no extension
and behaves exactly as before. The system SHALL reject or decline every other
MCP transport and primitive: the stdio transport, the legacy HTTP+SSE
transport, the Prompts primitive, `resources/subscribe`, the `2026-07-28`
`subscriptions/listen` change-feed stream, and the optional
`resources/directory/read` method (`directoryRead` stays `false`).

#### Scenario: stdio transport unsupported

- **GIVEN** an `mcp:` endpoint configuration
- **WHEN** a configuration declares a stdio transport
- **THEN** the configuration is rejected with a `CamelError` at construction

#### Scenario: legacy HTTP+SSE transport unsupported

- **GIVEN** an `mcp:` endpoint configuration
- **WHEN** a configuration declares the legacy HTTP+SSE transport
- **THEN** the configuration is rejected with a `CamelError` at construction

#### Scenario: prompts unsupported

- **GIVEN** a started MCP server
- **WHEN** a host issues `prompts/list`
- **THEN** the server responds that the prompts capability is unavailable

#### Scenario: resource subscriptions unsupported

- **GIVEN** a started MCP server
- **WHEN** a host issues `resources/subscribe`
- **THEN** the server responds that the operation is unsupported

#### Scenario: skills extension is absent without a root

- **GIVEN** a started MCP server whose configuration declares no skills root
- **WHEN** a host issues `server/discover` and then `skills/list`
- **THEN** discover advertises no Skills extension
- **AND** `skills/list` answers method not found

#### Scenario: skills extension is present with a root

- **GIVEN** a started MCP server whose configuration declares a skills root
- **WHEN** a host issues `server/discover`
- **THEN** discover advertises Resources and the
  `io.modelcontextprotocol/skills` extension with `directoryRead` false
- **AND** `resources/directory/read` is not advertised or implemented
