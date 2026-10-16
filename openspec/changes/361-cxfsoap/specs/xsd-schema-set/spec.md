# xsd-schema-set Specification (delta)

## ADDED Requirements

### Requirement: Schema set is transmitted as virtual paths plus a root systemId

The XML bridge registration call SHALL carry a schema set as a `map<string,
bytes> resources` of root-relative virtual paths to schema bytes, a
`map<string, string> aliases` from each supplied absolute-URI alias to its
normalized root-relative virtual path, and a root systemId identifying the entry
schema. The `resources` map MAY retain an alias-URI-keyed entry carrying the
aliased bytes for wire compatibility, but the `aliases` map is the authoritative
alias-to-virtual-base mapping and SHALL be transmitted; the matching `resources`
entry only redundantly carries bytes. The three `xml_bridge.proto` copies
(camel-validator, camel-xslt, bridges/xml) SHALL stay byte-identical for these
fields.

#### Scenario: Parent and child schemas register together

- **GIVEN** a root XSD importing a child XSD reachable from the root
- **WHEN** the schema set is registered
- **THEN** the request contains the reachable virtual paths and the root
  systemId, and registration succeeds

#### Scenario: Mirrored proto copies stay synchronized

- **GIVEN** the three `xml_bridge.proto` copies
- **WHEN** the schema-set fields are added
- **THEN** all three copies are byte-identical

#### Scenario: Alias map is transmitted beside resources

- **GIVEN** a supplied absolute-URI alias materialized into `resources` and
  retained in the alias map
- **WHEN** the schema set is registered
- **THEN** the request carries the alias-map entry (absolute URI → normalized
  root-relative virtual path) alongside the redundant `resources` entry, and
  registration succeeds

### Requirement: Schema-set numeric limits are enforced in Rust and Java

Rust SHALL enforce, during config load and discovery: `resources` at most 256
entries, the sum of transmitted byte values at most 8 MiB, and `schema_aliases`
at most 128 entries. Every transmitted virtual lookup key SHALL count toward the
256-entry cap, and its bytes SHALL count toward the 8 MiB total, including
multiple keys that share one canonical file. The caps SHALL be enforced
incrementally as keys are discovered and SHALL fail closed with a bounded error
when symlink aliases would otherwise produce unbounded virtual-key growth. Rust
SHALL reject **conflicting
provided resource definitions** — the same normalized virtual path supplied with
different bytes. Repeated reach of the same path is not a conflict and SHALL NOT
be an error.

The Java bridge SHALL enforce, on the **decoded** untrusted
`RegisterSchemaRequest` before compiling: decoded `resources` count at most 256,
the sum of decoded byte values at most 8 MiB, decoded `aliases` entries at most
128, and `resources` entries whose key is an alias URI at most 128, in addition
to the existing transport decoding cap. The alias-map cap and the
resource-alias-URI cap SHALL be two aligned caps, each enforced independently.
Java SHALL NOT attempt to detect duplicate map keys: proto map semantics
overwrite duplicates before decode, so the wire cannot represent a duplicate and
post-decode detection is impossible. Decoded absolute-URI `resources` entries
SHALL count toward the 256-resource cap and SHALL be limited to at most 128. An
alias and a relative path that denote the same file SHALL be separate entries
and SHALL each count toward the 256-entry cap and the 8 MiB total. Wire
duplicates are last-wins and SHALL NOT be counted.

#### Scenario: Rust discovery rejects more than 256 entries

- **GIVEN** a discovery result with 257 reachable entries (aliases included)
- **WHEN** the set is built
- **THEN** discovery fails with a bounded error naming the 256-entry cap

#### Scenario: Java registration rejects more than 256 decoded entries

- **GIVEN** an untrusted `RegisterSchemaRequest` decoded to 257 resources entries
- **WHEN** the bridge registers it
- **THEN** registration fails with a bounded error and no schema is compiled

#### Scenario: Rust discovery rejects more than 8 MiB total

- **GIVEN** a discovery result whose transmitted byte values sum to more than
  8 MiB
- **WHEN** the set is built
- **THEN** discovery fails with a bounded error naming the 8 MiB cap

#### Scenario: Java registration rejects more than 8 MiB decoded total

- **GIVEN** an untrusted request whose decoded resource byte values sum to more
  than 8 MiB
- **WHEN** the bridge registers it
- **THEN** registration fails with a bounded error

#### Scenario: Alias cap is enforced in Rust config

- **GIVEN** a `schema_aliases` config with 129 entries
- **WHEN** the pool config is validated
- **THEN** config validation fails

#### Scenario: Resource alias-URI count cap is enforced in Java

- **GIVEN** an untrusted request whose `resources` entries keyed by an alias URI
  exceed 128
- **WHEN** the bridge registers it
- **THEN** registration fails with a bounded error naming the resource-alias cap

#### Scenario: Alias mapping cap is enforced in Java

- **GIVEN** an untrusted request whose decoded `aliases` map exceeds 128 entries
- **WHEN** the bridge registers it
- **THEN** registration fails with a bounded error naming the alias-map cap

#### Scenario: Rust rejects conflicting resource definitions

- **GIVEN** a build where the same normalized virtual path is supplied with two
  different byte values
- **WHEN** the set is built
- **THEN** discovery fails with a bounded error

#### Scenario: Java does not reject repeated map keys

- **GIVEN** a wire request whose map key is overwritten before Java decodes it
- **WHEN** the bridge registers it
- **THEN** no duplicate-key error is raised; the decoded map is capped as above

### Requirement: Discovery reads confined from Rust; the Java resolver never fetches

Rust discovery MAY read files from the confined virtual root (the root file's
parent directory) to discover the set; these confined filesystem reads are
permitted. The Java resolver, by contrast, SHALL perform no fetch of any kind —
no network access and no filesystem read — and SHALL resolve only from the
already-transmitted virtual `resources` bytes. The two surfaces SHALL NOT be
conflated.

#### Scenario: Rust discovery reads within the root only

- **GIVEN** a root file whose imports reach files inside the virtual root
- **WHEN** discovery runs
- **THEN** Rust reads those files from the confined root and no path outside it

#### Scenario: Java resolver reads no filesystem or network

- **GIVEN** a compiled schema set handed to the bridge
- **WHEN** an `xs:import` is resolved
- **THEN** the bytes come only from the transmitted map and no filesystem or
  network access occurs

### Requirement: Discovery follows imports reachable from the root file only

Discovery SHALL start from `schema_path` (the root schema file) with the virtual
root being that file's parent directory, and SHALL include only schemas
reachable by transitively following `xs:import`/`xs:include` targets — it SHALL
NOT walk the directory tree wholesale. Discovery SHALL cache each canonical
file's bytes and parsed references once, but SHALL expand a canonical file once
per distinct `(canonical target, normalized virtual path)` state: the visited
set SHALL be keyed by that pair, NOT by canonical target alone. Two virtual
paths that resolve to one canonical file but have different base directories
SHALL each expand their own relative references — for example symlinks
`a/shared.xsd` and `b/shared.xsd` resolving to one canonical file that contains
`xs:include schemaLocation="child.xsd"` SHALL expand `a/child.xsd` and
`b/child.xsd` respectively, and both SHALL be transmitted. Every distinct
reachable virtual lookup key SHALL be transmitted as its own `resources` entry,
because the Java resolver serves supplied keys only; omitting a virtual key
would make the importing schema unresolvable. Diamonds and cycles SHALL be
permitted, and expansion SHALL stop when the same `(canonical target,
normalized virtual path)` pair recurs, so a cycle SHALL terminate without a
depth loop. Depth SHALL count newly expanded states on one traversal path;
maximum import/include depth SHALL be 32. Because symlinks can produce unbounded
virtual-key growth over the 256-key, 128-alias, and 8 MiB caps, those caps SHALL
be enforced incrementally during expansion and SHALL fail closed with a bounded
error. Discovery SHALL use the existing XML
parser stack (the workspace `quick-xml` dependency) and SHALL NOT introduce a
new dependency. The root systemId SHALL be the root file's normalized relative
basename. Discovery SHALL derive
`canonical_root = canonicalize(schema_path.parent())` and canonicalize every
candidate file: a file whose canonical target is inside `canonical_root` SHALL
be allowed (symlinks included), and a file whose canonical target escapes
`canonical_root` SHALL be rejected. Transmitted keys remain root-relative
virtual paths. Symlinks in a config-map root SHALL be supported. A single root
file with no imports SHALL yield a one-entry set.

#### Scenario: Reachable imports are included

- **GIVEN** a root file importing one child that includes one grandchild
- **WHEN** discovery runs
- **THEN** the set contains the root, child, and grandchild as normalized
  relative virtual paths

#### Scenario: Unrelated files are excluded

- **GIVEN** a virtual root containing an unreferenced schema beside the root
- **WHEN** discovery runs
- **THEN** the unreferenced file is not in the set

#### Scenario: Diamond import is included once without a duplicate error

- **GIVEN** root A importing B and C, each importing D
- **WHEN** discovery runs
- **THEN** D is included exactly once and no duplicate error is raised

#### Scenario: Cyclic include terminates without a depth loop

- **GIVEN** schema A that includes B, and B that includes A
- **WHEN** discovery runs
- **THEN** discovery terminates with no depth loop and no error

#### Scenario: Distinct virtual bases expand their own relative children

- **GIVEN** symlinks `a/shared.xsd` and `b/shared.xsd` resolving to one canonical
  file that includes `child.xsd` relative to its own directory
- **WHEN** discovery runs
- **THEN** the canonical file's bytes are read once, and both `a/child.xsd` and
  `b/child.xsd` are transmitted so each importing base resolves

#### Scenario: Symlinked include cycle terminates and keeps every virtual lookup key

- **GIVEN** a schema whose `xs:include` target is a symlink alias resolving back
  to an already-expanded canonical file (`xs:include`, not a same-namespace
  `xs:import`, so the fixture is natively valid)
- **WHEN** discovery runs
- **THEN** expansion stops on the repeated `(canonical target, normalized
  virtual path)` pair, and every distinct virtual key is transmitted so the
  supplied-keys-only resolver can serve the include

#### Scenario: Depth limit is enforced

- **GIVEN** an import chain deeper than 32 new nodes on one traversal path
- **WHEN** discovery runs
- **THEN** discovery fails with a bounded, actionable error

#### Scenario: In-root symlink is accepted

- **GIVEN** a schema referenced through a symlink whose canonical target is
  inside `canonical_root`
- **WHEN** discovery runs
- **THEN** the schema is included

#### Scenario: Escaping symlink is rejected

- **GIVEN** a schema referenced through a symlink whose canonical target is
  outside `canonical_root`
- **WHEN** discovery runs
- **THEN** discovery is refused

#### Scenario: Traversal outside the virtual root is rejected

- **GIVEN** an import whose canonical target resolves outside `canonical_root`
- **WHEN** discovery runs
- **THEN** discovery is refused

#### Scenario: Single file without imports is a one-entry set

- **GIVEN** a root file with no imports
- **WHEN** discovery runs
- **THEN** the set contains exactly one entry

### Requirement: Supplied absolute-URI aliases resolve; only unsupplied URLs are refused

`ValidatorConfig` SHALL support a bounded `schema_aliases` map from absolute URI
to a root-relative virtual path (at most 128 entries, per the numeric-limit
requirement). Discovery SHALL include only aliases that are supplied, and they
SHALL participate in the reachable discovery. A supplied absolute-URI alias
SHALL resolve from its transmitted bytes with no fetch. Only an UNSUPPLIED
network/system URL SHALL be refused.

The Java bridge SHALL validate the decoded `aliases` map before compiling:
every key SHALL be an absolute URI, and every value SHALL be a normalized
root-relative virtual path present in `resources`. An alias whose key is not an
absolute URI, whose value is absolute, whose value is missing from `resources`,
or whose value, after normalization against the registration anchor, escapes the
virtual root SHALL be rejected with a bounded error and no fetch. A `..` segment
that normalizes to a path still inside the virtual root SHALL be accepted.

#### Scenario: Alias key must be an absolute URI

- **GIVEN** a decoded alias map whose key is a relative path
- **WHEN** the bridge registers it
- **THEN** registration fails with a bounded error and no fetch occurs

#### Scenario: Alias value must be normalized relative and present in resources

- **GIVEN** a decoded alias map whose value is absolute, missing from
  `resources`, or whose normalized form escapes the virtual root
- **WHEN** the bridge registers it
- **THEN** registration fails with a bounded error and no fetch occurs

#### Scenario: In-root traversal in an alias value normalizes and resolves

- **GIVEN** a decoded alias map whose value contains a `..` segment that
  normalizes to a root-relative virtual path present in `resources`
- **WHEN** the bridge registers and compiles the schema
- **THEN** the alias resolves from the normalized in-root key and no fetch
  occurs

#### Scenario: Supplied alias resolves without a fetch

- **GIVEN** an `xs:import` with an absolute URI and a supplied `schema_aliases`
  entry mapping it to a root-relative path
- **WHEN** the schema compiles
- **THEN** it resolves from the supplied bytes and no fetch occurs

#### Scenario: Unsupplied URL is refused

- **GIVEN** an `xs:import` with an `http`/`https`/`file` systemId absent from
  `schema_aliases`
- **WHEN** the schema compiles
- **THEN** resolution is refused and no fetch occurs

#### Scenario: Alias and relative path for the same file count separately

- **GIVEN** an alias and a relative path that denote the same schema file
- **WHEN** the set is built
- **THEN** both are separate entries and each counts toward the 256-entry cap
  and the 8 MiB total

### Requirement: Supplied aliases reach the component through the validator URI

`ValidatorConfig::from_uri` SHALL parse the operator-facing alias surface: one
optional `schemaAliases` query parameter whose value is a single percent-encoded
JSON object mapping each supplied absolute-URI alias to a normalized
root-relative virtual path, for example
`schemaAliases=%7B%22http%3A%2F%2Fexample.com%2Fp.xsd%22%3A%22a%2Fparent.xsd%22%7D`.
The value SHALL be percent-decoded exactly once and then parsed with the
workspace `serde_json` dependency; the parser SHALL NOT decode twice. This URI
parameter is the sole operator surface for `schema_aliases`; the field is not
configured by a direct constructor.

`from_uri` SHALL reject, with a bounded `CamelError::InvalidUri`, a duplicated
`schemaAliases` query parameter, malformed percent-encoding, a non-object JSON
body, a non-string key or value, and more than 128 entries. These failures SHALL
occur while `ValidatorComponent::create_endpoint` parses the URI, before any
bridge startup, and SHALL NOT be deferred to the first validation call. The
`validator` `uri_options` metadata SHALL list `schemaAliases`, and the parser
keys, the metadata descriptor, and the published URI table SHALL agree.

#### Scenario: Alias query is parsed at endpoint creation

- **GIVEN** a `validator:<root>.xsd` URI carrying a percent-encoded
  `schemaAliases` JSON object with one absolute-URI entry
- **WHEN** the endpoint is created
- **THEN** `ValidatorConfig.schema_aliases` contains that entry and no bridge
  has started

#### Scenario: Percent-encoded alias value is decoded exactly once

- **GIVEN** a double-encoded `schemaAliases` value (a valid JSON object
  percent-encoded twice)
- **WHEN** the endpoint is created
- **THEN** creation fails with a bounded error, because one decode yields
  invalid JSON rather than a second decode

#### Scenario: Malformed alias query fails endpoint creation

- **GIVEN** a `schemaAliases` value that is malformed percent-encoding,
  non-object JSON, or has a non-string value
- **WHEN** the endpoint is created
- **THEN** creation fails with a bounded error before any bridge startup

#### Scenario: Duplicate alias query parameter is rejected

- **GIVEN** a `validator:` URI with two `schemaAliases=` query pairs
- **WHEN** the endpoint is created
- **THEN** creation fails with a bounded error naming the option

#### Scenario: Alias cap is enforced at URI parse

- **GIVEN** a `schemaAliases` JSON object with 129 entries
- **WHEN** the endpoint is created
- **THEN** creation fails with a bounded error naming the 128-alias cap and no
  bridge starts

### Requirement: Resolver serves supplied keys only with defined semantics

The Java `LSResourceResolver` SHALL resolve `xs:import`/`xs:include` targets
only from supplied keys, in this precedence order:

1. When the importing schema has a virtual base path, the resolver SHALL first
   normalize the relative systemId against that base to a normalized
   root-relative virtual path and resolve that key. Base-relative normalization
   SHALL take precedence over an exact match on the raw systemId: a nested
   importer `a/parent.xsd` whose target is `child.xsd` SHALL resolve
   `a/child.xsd`, never a root-level `child.xsd` that happens to equal the raw
   systemId.
2. An absolute URI — a systemId carrying an explicit URI scheme such as
   `http`, `https`, or `file` — SHALL resolve only when supplied in the alias
   map, by exact match on the alias key. The resolved schema's `LSInput` SHALL
   set `systemId` to the alias's mapped normalized root-relative virtual path,
   NOT the absolute URI, and SHALL leave `baseURI` UNSET; the mapped virtual path
   then expands from the process anchor exactly as a natively reached child's
   systemId does, so the schema's own relative `xs:import`/`xs:include` targets
   resolve against the virtual directory with a single base. Setting both
   `systemId` and `baseURI` to the virtual path applies the base twice (for
   example `a/a/child.xsd`) and SHALL NOT be done.
3. A relative systemId SHALL resolve by exact raw key only when there is no
   importing-schema virtual base to normalize against; this is the distinguished
   root resolved reference and SHALL NOT shadow a base-relative key.

The root schema SHALL be passed to the validator as a source whose `systemId` is
the transmitted root systemId (the normalized relative basename) and whose
`baseURI` is UNSET. Xerces expands that `systemId` against the process anchor (the
JVM working directory) into an absolute `file:` URI; the bridge SHALL capture that
expanded root URI through Java relative-to-absolute normalization and derive the
anchor root directory from it. On each resolver call the bridge receives the
importing schema's expanded `file:` URI as its base; it SHALL (1) validate that
the expanded base is under the same computed anchor root — a base outside the
anchor SHALL be rejected; (2) strip the anchor prefix to obtain the importer's
normalized root-relative virtual key; (3) normalize the relative systemId against
that key's directory to a normalized root-relative virtual path; and (4) look the
key up in the supplied map. A returned child `LSInput` SHALL set `systemId` to the
normalized root-relative virtual path and SHALL leave `baseURI` UNSET, so Xerces
re-expands it from the same process anchor on the next call. An explicit virtual
anchor URI MAY be used instead of the process anchor only if it yields the
identical deterministic key mapping, and the mapping SHALL be deterministic and
isolated per registration (no process-global anchor leaks between registrations).
No filesystem fetch SHALL ever occur; external access SHALL remain disabled and
the XXE posture SHALL NOT weaken.

Normalization SHALL run before the key lookup: a `..` segment that normalizes to
a path still inside the anchor root SHALL be permissible and resolved, and only a
`..` that escapes the anchor root SHALL be rejected. A missing key, an alias key
that is not an absolute URI, an alias value that is absolute, missing, or whose
normalized form escapes the root, an unsupplied network URL, a base outside the
anchor, or a DTD reference SHALL be rejected.

#### Scenario: Relative import resolves from the supplied set

- **GIVEN** a root schema whose `xs:import` targets a supplied relative path
- **WHEN** the schema compiles
- **THEN** the child resolves from the in-memory bytes

#### Scenario: Base-relative resolution beats an exact root raw key

- **GIVEN** a supplied set with a root-level `child.xsd` and a nested
  `a/parent.xsd` whose relative `child.xsd` normalizes to `a/child.xsd`, where
  the two `child.xsd` schemas are natively valid and impose observably different
  constraints
- **WHEN** `a/parent.xsd` resolves its target
- **THEN** it resolves `a/child.xsd`, not the root-level `child.xsd` that equals
  the raw systemId

#### Scenario: Alias resolution uses the mapped virtual base

- **GIVEN** an absolute-URI alias mapped to `a/parent.xsd`, where `a/parent.xsd`
  contains a relative `xs:include` of `child.xsd`, and a conflicting root-level
  `child.xsd`, both natively valid and imposing observably different constraints
- **WHEN** the aliased `a/parent.xsd` resolves its relative target
- **THEN** it resolves `a/child.xsd` from the mapped virtual base, never the
  root-level `child.xsd`, proving the `LSInput` `systemId` is the virtual path
  (with `baseURI` unset) and not the absolute URI

#### Scenario: Nested importer and alias do not double the base

- **GIVEN** a root schema importing a nested `nested/child.xsd` and supplying an
  absolute-URI alias mapped to `a/parent.xsd`, where `a/parent.xsd` imports
  `a/child.xsd`, with a conflicting root-level `child.xsd`, all natively valid
- **WHEN** the aliased `a/parent.xsd` resolves its relative target
- **THEN** it resolves `a/child.xsd` from the single mapped virtual base, never
  `a/a/child.xsd`, `nested/a/child.xsd`, or the root-level `child.xsd`, proving
  the root source and the returned `LSInput` set only `systemId` with `baseURI`
  unset

#### Scenario: Escaping traversal and DTD are rejected

- **GIVEN** an `xs:import` whose `..` segment normalizes outside the anchor root,
  or a DTD directive
- **WHEN** the schema compiles
- **THEN** resolution is refused

#### Scenario: In-root traversal normalizes before key lookup

- **GIVEN** an `xs:import` whose systemId contains a `..` segment that normalizes
  to a supplied root-relative virtual key still inside the anchor root
- **WHEN** the schema compiles
- **THEN** the normalized key resolves from the supplied bytes and no fetch occurs

### Requirement: Schema cache key includes child digests and replay re-registers the set

The schema-set cache key SHALL be derived from the root systemId, the sorted
virtual keys, each entry's bytes/child digest, and the alias map. A changed
child SHALL yield a new key. After a bridge reconnect/restart, the component
SHALL re-register the full schema set.

#### Scenario: Changed child yields a new cache key

- **GIVEN** a registered schema set
- **WHEN** a child schema's bytes change
- **THEN** the computed key differs and validation uses the new set

#### Scenario: Restart replay re-registers the whole set

- **GIVEN** a registered schema set
- **WHEN** the bridge restarts and the reconnect handler replays registrations
- **THEN** every virtual path in the set is re-registered, including children
