# cli-compile deltas — sealed-artifact-materialization

## MODIFIED Requirements

### Requirement: Run embedded documents without extraction

The artifact SHALL feed the indexed documents through the existing parse, runtime interpolation, lowering, boot, and start paths using an in-memory virtual store. Runtime SHALL build configuration from embedded `Camel.toml`, include, and selected-profile entries. Embedded job configuration SHALL pass through the same job boot projection as argv jobs before context configuration; embedded route configuration SHALL boot unchanged. Runtime SHALL perform no source-tree reads, glob expansion, ambient `Camel.toml` or profile loading, canonicalization of declared asset strings, temporary extraction, watch, or hot reload. The only permitted filesystem writes are an explicitly requested report and the confined per-boot materialization of TLS-family asset entries — the remaining legacy path-reading consumers; the `xslt stylesheet`, `xsd schema`, and `sql file` classes resolve in memory through the embedded-asset registry and write nothing. Source diagnostics SHALL use `compiled://<logical-path>` identities. `${env:NAME}` expressions SHALL resolve from the deployment environment, not the compiler environment. Route `--report <path>` SHALL write JSON object `{ "kind": "route", "status": "completed"|"failed", "error": string|null }`; job reports SHALL use the existing job outcome schema. Boot and report-write failures SHALL exit 2; route pipeline failures SHALL exit 1; completed routes SHALL exit 0. The read-only deployment contract is class-dependent: an artifact whose assets are all memory-served SHALL write nothing under any filesystem; an artifact containing a disk-written TLS-family class SHALL require a writable location — the system temp directory (`TMPDIR`) or, failing that, a writable directory next to the artifact — and SHALL fail before boot with a typed diagnostic naming the class and the requirement when no writable location exists.

#### Scenario: Multi-document artifact runs without its source tree

- **GIVEN** a valid compiled artifact whose source files, `Camel.toml`, and working directory are unavailable
- **WHEN** the artifact starts with required deployment environment values
- **THEN** all indexed route documents boot and run from memory without file discovery, extraction, or temporary writes beyond the documented materialization carve-out

#### Scenario: Runtime cannot discover an unembedded file

- **GIVEN** a compiled artifact and a new route file placed beside it after compilation
- **WHEN** the artifact starts
- **THEN** the new file is ignored because runtime consumes only indexed store entries

#### Scenario: Unknown store schema fails closed

- **GIVEN** a marked artifact whose index declares a store schema other than the supported `store_schema: 2` — including the removed schema 1 — beside an otherwise current (schema 3, 4, or 5) manifest
- **WHEN** the artifact starts
- **THEN** it exits 2 before configuration or route boot with an error naming the unsupported store schema and the remedy to recompile from source with this version

#### Scenario: Read-only deployment

- **GIVEN** a valid artifact whose assets are all static files (memory-served), running with a read-only root filesystem and no writable `TMPDIR`
- **WHEN** it boots and receives deployment environment values
- **THEN** it runs without materializing any entry and writes nothing other than an explicitly requested report

#### Scenario: In-process classes write nothing at boot

- **GIVEN** a valid artifact whose asset classes are only `xslt stylesheet`, `xsd schema`, and `sql file`, running with a read-only root filesystem and no writable `TMPDIR`
- **WHEN** it boots
- **THEN** no per-boot directory is created, nothing is written under any filesystem beyond an explicitly requested report, and the owning components resolve their assets in process from the embedded-asset registry

#### Scenario: Disk-written class boots on read-only root with writable TMPDIR

- **GIVEN** a valid artifact containing a disk-written class (for example an embedded TLS certificate), running with a read-only root filesystem and a writable `TMPDIR`
- **WHEN** it boots
- **THEN** the confined per-boot materialization succeeds inside `TMPDIR`, the listener spawns against the substituted paths, and nothing is written outside `TMPDIR` and an explicitly requested report

#### Scenario: Disk-written class without writable temp falls back to the artifact directory

- **GIVEN** a valid artifact containing a disk-written TLS-family class, running with no writable `TMPDIR` and a writable directory next to the artifact
- **WHEN** it boots
- **THEN** the confined per-boot materialization is created next to the artifact with the same 0700/0600 confined semantics, the listener spawns against the substituted paths, and the system temp directory is not written

#### Scenario: Disk-written class with no writable location fails before boot

- **GIVEN** a valid artifact containing a disk-written TLS-family class, running with neither a writable system temp directory nor a writable artifact directory
- **WHEN** it starts
- **THEN** it exits 2 before route or configuration boot with the typed diagnostic naming the asset class and the missing writable-directory requirement, carrying a path-free OS cause (the attempted candidate paths never appear in the message)

#### Scenario: Embedded job artifact binds no diagnostic listeners

- **GIVEN** a compiled job artifact whose embedded configuration enables `[observability.prometheus]` and `[observability.health]` and declares `[runtime_journal]`
- **WHEN** the artifact starts
- **THEN** it projects the configuration through the job boot policy, opens no journal, binds no Prometheus or health listener, and exits with the normal job outcome codes

### Requirement: Embed and serve deploy-time assets

The artifact SHALL carry deploy-time assets as typed `asset` store entries under the `assets/` logical namespace with `store_schema: 2`, embedding asset bytes verbatim and recording each entry's asset class. The index SHALL carry the compile-time substitution table recording each (document, declared string) pair's exact byte span(s) in the normalized document or config entry together with its asset logical path; runtime SHALL replace only those recorded spans — applied last-offset-first so earlier offsets and line numbers stay stable — and SHALL NOT perform declared-string text search, canonicalize, re-resolve, or re-read declared asset strings from the filesystem. Values substituted into a URI SHALL be percent-encoded so they round-trip consumer decoding (the `validator:` URI path is percent-decoded before use); when a materialization path contains URI-reserved characters or whitespace that cannot be encoded safely, the runtime SHALL fail before boot with a named diagnostic. The artifact runtime SHALL populate an in-memory, read-only asset registry from the decoded asset entries before boot, verifying each BLAKE3 digest against the manifest and failing closed on mismatch, duplicate, or missing entry. Static-file entries SHALL be served from the registry. The `xslt stylesheet`, `xsd schema`, and `sql file` classes are memory-served as well (the mission-350 in-process pattern): at boot the runtime registers each referenced asset's verified bytes into the process-global embedded-asset registry (`camel_component_api::embedded_assets`) under its store path and rewrites each recorded span to a `camel-embedded:<store path>` reference — percent-encoded at URI sites so it round-trips consumer decoding, while a `literal`-context site for these classes keeps failing closed on the literal charset clause. The owning component SHALL resolve a `camel-embedded:` reference from that registry before any filesystem attempt — applying its existing size cap — and SHALL fail closed when the reference names no registered asset; a value carrying either the canonical or the percent-encoded (`camel-embedded%3A`) prefix spelling that names no registered asset fails with a typed error naming the reference and never probes the host filesystem (the prefix is reserved, so prefixed values never reach the filesystem path); `camel-validator` keeps decoding the URI path exactly once in its own `from_uri` parse and `camel-sql` receives the canonical spelling through camel-endpoint's `parse_uri` decode-once, while `camel-xslt` — whose manual URI parse performs no decoding — accepts both spellings at its seam, decoding a complete encoded reference exactly once (a canonical reference's suffix is a literal key and is never decoded), without changing plain-path behavior. The XSD class resolves through registry-backed ROOT discovery in the `compile_xsd`/`discover_schema_set` path: the root schema resolves from the registry, import/include resolution keeps the existing budgets (8 MiB total transmitted schema-set bytes — alias and relative-path entries both count toward it — 256 resources, 128 aliases, 32-level import depth), and an embedded root carrying an unresolvable import/include fails with a typed, named error naming the missing resource. The `camel-embedded:` prefix is reserved workspace-wide together with `camel-proto-compiler::embedded`: a real file whose name starts with the prefix is shadowed, a documented reservation. Consumers that read files by path and are named in the design as legacy consumers (TLS PEM loading from document fields and TLS URI parameters) SHALL receive a per-boot confined materialization: only referenced TLS-family assets, written into a directory created under `std::env::temp_dir()` with a random per-boot name at mode 0700 — when that directory cannot be created, falling back to a writable directory next to the artifact (the running executable's parent); the first candidate that accepts the directory wins — each file written once with `create_new` semantics at mode 0600 so an existing entry or symlink is never followed, ancestor confinement re-checked per the bd `rc-0ks57` discipline, referenced through substituted paths resolved before discovery, and removed at shutdown AND on boot failure by a guard owned by the artifact runtime. When neither the system temp directory nor the artifact-adjacent directory is writable, the runtime SHALL fail before boot with the typed `TempUnavailable` diagnostic naming the asset class and the writable-directory requirement, with a path-free OS cause — the raw tempfile source error carries the attempted path and MUST NOT be interpolated into the user-facing message, staying reachable through `Error::source()`. An abrupt `SIGKILL`, a `process::exit` call, or the in-tree force-exit on a second stop signal bypasses the guard and may leave residue in the materialization location; this caveat SHALL be documented. The artifact SHALL perform no bulk extraction, and the registry SHALL stay empty in normal `camel run` operation. Runtime SHALL NOT load assets that the store does not contain. TLS-class references — document `tls` blocks, the `tlsCert`/`tlsKey` URI parameters, and the gRPC `*Path` family — SHALL resolve exclusively through the embedded store and its confined materialization, with NO host-filesystem fallback: the runtime SHALL NOT probe the host filesystem for any TLS path, and a host path that was not embedded SHALL NOT be consulted.

#### Scenario: Asset round trip through the registry

- **GIVEN** a compiled artifact embedding a static-file tree
- **WHEN** the artifact boots
- **THEN** the registry exposes the asset bytes by logical path, the digest matches the manifest, and no temporary file is created — static files are the memory-served class

#### Scenario: In-process asset classes never touch the filesystem

- **GIVEN** a compiled artifact whose routes reference `xslt:`, `validator:`, and `sql:file:` assets
- **WHEN** the artifact boots
- **THEN** the substitution sites carry `camel-embedded:` references, the owning components receive the exact embedded bytes from the registry, no file is written for these classes, and a reference naming an unregistered asset fails closed with the component's normal error shape

#### Scenario: Percent-encoded reference round trips through consumer parses

- **GIVEN** a compiled artifact whose `xslt:` substitution sites carry the percent-encoded spelling `camel-embedded%3A<store path>` written by the runtime (the manual xslt URI parse does not decode) and a `sql:file:` site carrying the canonical spelling (the `parse_uri` decode already ran)
- **WHEN** the owning component parses the URI and resolves the stylesheet or query
- **THEN** the xslt seam decodes the spelling exactly once and resolves the same registered bytes as the canonical spelling, the sql query resolves from the registry through the canonical spelling, a prefixed-but-unregistered reference fails with a typed error naming the reference and never probes the host filesystem, and a plain-path stylesheet or query resolves exactly as before the change

#### Scenario: Embedded XSD set behavior is explicit

- **GIVEN** a compiled artifact with a `validator:` root schema that is standalone (no import/include) and a second artifact whose root schema imports a file that was never embedded
- **WHEN** the artifacts boot
- **THEN** the standalone schema resolves entirely from the registry under the existing budgets (8 MiB total transmitted schema-set bytes — alias and relative-path entries both count toward it — 256 resources, 128 aliases, 32-level import depth), while the importing artifact fails with a typed, named error identifying the missing schema resource and never probes the host filesystem for it

#### Scenario: Host decoys for in-process classes are never probed

- **GIVEN** a compiled artifact and new files placed after compilation at the original `xslt:`, `validator:`, and `sql:file:` declared paths
- **WHEN** the artifact starts
- **THEN** the owning components resolve only through the embedded registry, the decoy files are never opened, and the served bytes equal the embedded entries

#### Scenario: Legacy consumer materialization is confined

- **GIVEN** a compiled artifact whose TLS listener references embedded certificate and key entries
- **WHEN** the artifact boots and the listener spawns
- **THEN** the referenced assets exist only inside the confined per-boot directory (system temp directory, or the artifact-adjacent directory when the temp dir is not writable), the substituted paths resolve inside it, files are mode 0600 and the directory mode 0700, no asset is written outside it, and shutdown removes the directory

#### Scenario: Materialization is removed on boot failure

- **GIVEN** a compiled artifact whose route boot fails after the confined materialization has run
- **WHEN** the artifact exits with a boot failure
- **THEN** the per-boot directory and its files are removed by the runtime guard and no materialized file outlives the failed boot

#### Scenario: Post-compile asset decoy is ignored

- **GIVEN** a compiled artifact and a new file placed at a path one of its embedded asset references originally named
- **WHEN** the artifact starts
- **THEN** runtime resolves declared strings only through the substitution table without canonicalizing them, serves only the embedded entry, and never reads the new file

#### Scenario: TLS resolution never touches the host filesystem

- **GIVEN** a compiled artifact whose TLS assets were embedded, deployed on a host where the originally declared certificate and key paths do not exist
- **WHEN** the artifact boots and the TLS listener spawns
- **THEN** TLS paths resolve only through the embedded store and the confined materialization, the absent host paths are never probed, and the listener boots successfully

#### Scenario: Asset digest mismatch fails boot

- **GIVEN** a marked artifact whose asset content bytes no longer match the recorded BLAKE3 digest
- **WHEN** the artifact starts
- **THEN** it exits 2 before configuration or route boot and never populates the registry

#### Scenario: Normal run never populates the registry

- **GIVEN** a plain `camel run` invocation without a compiled artifact
- **WHEN** routes boot
- **THEN** the asset registry stays empty and asset embedding plays no part in discovery

#### Scenario: Registry tests are process-isolated

- **GIVEN** the registry test suite runs in its own test process with the process-global registry
- **WHEN** a test populates the registry and asserts through `is_populated()`
- **THEN** no other test binary observes the population, and no test resets global state
