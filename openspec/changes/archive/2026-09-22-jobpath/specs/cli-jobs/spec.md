## MODIFIED Requirements

### Requirement: job discovery and no-argument listing

The system SHALL accept a `[jobs]` table with ordered `dirs`, defaulting to `["jobs"]`, and reject unknown keys. Legacy `dir` SHALL fold into one `dirs` entry. Each root SHALL resolve against the `Camel.toml` root. A bare name SHALL probe only `<name>.job.yaml` at the root level of every configured root, report a miss with the probed path, and use the first matching root when exactly one match exists. Multiple matching roots SHALL produce an exit-2 collision naming all paths. Bare names SHALL never consult the current working directory. Document-argument resolution beyond bare names SHALL follow the named job resolution requirement; an explicit-class argument that exists relative to the current working directory SHALL bypass root probing. A job SHALL retain its mandatory explicit route source and SHALL never inherit `routes/` discovery. No-argument listing SHALL show `.job.yaml` and `.job.yml` stems, descriptions or `(no description)`, replace embedded newlines with spaces, tolerate malformed siblings as `(unparseable)`, print the existing creation hint for absent or empty roots, and reject listing-only report options with exit 2. Listing and run reports SHALL never co-occur.
When both `dir` and `dirs` are present, `dirs` SHALL take precedence and `dir` SHALL not add a duplicate root. Recursive listing SHALL preserve the existing root-level output line for files directly under a configured root (the bare stem). For nested files, it SHALL show the configured-root-relative path as `<relative-path> — <description>` — the exact spelling that resolves as a document argument, so the displayed name is invocable verbatim. Bare-name resolution SHALL remain root-level only: a bare name never resolves a document nested below a root.

#### Scenario: lists found jobs with exit 0

- **GIVEN** a configured root contains valid job documents
- **WHEN** `camel job` runs without a document argument
- **THEN** it lists each job and exits 0

#### Scenario: empty or absent directory is exit 0

- **GIVEN** every configured root is empty or absent
- **WHEN** `camel job` runs without a document argument
- **THEN** it prints a creation hint and exits 0

#### Scenario: bare-name resolution is deterministic

- **GIVEN** `report.job.yaml` exists at a configured root
- **WHEN** `camel job report` runs from a project subdirectory
- **THEN** it loads the Camel.toml-rooted document

#### Scenario: explicit path wins over bare-name resolution

- **GIVEN** an explicit document path exists outside configured roots
- **WHEN** `camel job path/to/document.job.yml` runs
- **THEN** it loads that path without root probing

#### Scenario: unparseable sibling does not abort listing

- **GIVEN** a configured root contains one valid job and one malformed job document
- **WHEN** `camel job` lists jobs
- **THEN** both entries appear and the malformed entry shows `(unparseable)`

#### Scenario: listed job.yml is not bare-name resolvable

- **GIVEN** only `legacy.job.yml` exists in a configured root
- **WHEN** `camel job legacy` runs
- **THEN** it probes only `legacy.job.yaml` and exits 2

#### Scenario: multiline description renders on one line

- **GIVEN** a job description contains embedded newlines
- **WHEN** `camel job` lists jobs
- **THEN** the description appears on one line with spaces

#### Scenario: jobs dir anchors at the Camel.toml root

- **GIVEN** the shell runs from a nested project directory
- **WHEN** `camel job` lists jobs
- **THEN** it scans roots relative to Camel.toml, not the shell directory

#### Scenario: route source is never defaulted to routes discovery

- **GIVEN** a job document has no explicit route source
- **WHEN** `camel job <name>` runs
- **THEN** loading fails with exit 2 and no routes fallback occurs

#### Scenario: report flag without a document is a usage error

- **GIVEN** `camel job --report out.json` has no document argument
- **WHEN** the command runs
- **THEN** it exits 2 without listing

#### Scenario: Ordered configured roots

- **GIVEN** `Camel.toml` declares `dirs = ["team-a", "team-b"]`
- **WHEN** `camel job` discovers jobs
- **THEN** it scans `team-a` before `team-b`, with both paths resolved relative to `Camel.toml`
- **TEST:** `configured_job_dirs_are_scanned_in_order`
- **SETUP:** Create `Camel.toml` and one valid job in each root.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test configured_job_dirs_are_scanned_in_order`.
- **ASSERT:** Output order matches `team-a`, then `team-b`.

#### Scenario: Legacy directory alias

- **GIVEN** `Camel.toml` declares only `dir = "legacy-jobs"`
- **WHEN** `camel job` discovers jobs
- **THEN** it scans `legacy-jobs` as the sole discovery root
- **TEST:** `legacy_jobs_dir_alias_is_supported`
- **SETUP:** Create `Camel.toml` with only `[jobs].dir` and one job below it.
- **ACTION:** Run `cargo test -p camel-config --lib legacy_jobs_dir_alias_is_supported`.
- **ASSERT:** Deserialized roots contain exactly `legacy-jobs`.

#### Scenario: Explicit dirs take precedence

- **GIVEN** `Camel.toml` declares `dir = "legacy"` and `dirs = ["first", "second"]`
- **WHEN** `camel job` discovers jobs
- **THEN** it scans only `first` and `second`
- **TEST:** `explicit_job_dirs_override_legacy_dir`
- **SETUP:** Put `legacy-only.job.yaml` only in `legacy`, and `first.job.yaml` only in `first`.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test explicit_job_dirs_override_legacy_dir`.
- **ASSERT:** Configured roots equal `first`, `second`; stdout contains the exact root-level line `first — (no description)` and does not contain `legacy-only`.

#### Scenario: Default root

- **GIVEN** `Camel.toml` has no jobs directory setting
- **WHEN** `camel job` discovers jobs
- **THEN** it scans `jobs` relative to `Camel.toml`
- **TEST:** `default_jobs_root_is_used`
- **SETUP:** Create `Camel.toml` and one job under its `jobs` directory.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test default_jobs_root_is_used`.
- **ASSERT:** The job appears and the command exits 0.

#### Scenario: Named job exists only in later root

- **GIVEN** only the second configured root contains `report.job.yaml`
- **WHEN** `camel job report` runs
- **THEN** it loads the second-root document, emits no miss or ambiguity on stderr, and exits 0
- **TEST:** `named_job_uses_later_matching_configured_root`
- **SETUP:** Create `Camel.toml` with `dirs = ["first", "second"]`. Put valid `report.job.yaml` only in `second` with output marker `second-marker`.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test named_job_uses_later_matching_configured_root`.
- **ASSERT:** Stable report fields contain `"document":"second/report.job.yaml"` and `"outcome":"Completed"`; stderr is empty and exit status is 0. Ignore dynamic duration fields.

#### Scenario: Explicit path and bare-name miss

- **GIVEN** `ops/one-shot.job.yml` exists and `missing.job.yaml` does not
- **WHEN** explicit path and bare-name commands run from a nested directory
- **THEN** explicit path wins, while miss stderr names every configured root probe and exit is 2
- **TEST:** `explicit_job_path_bypasses_roots_and_bare_miss_is_named`
- **SETUP:** Create `Camel.toml` with `dirs = ["first", "second"]`, nested `ops/one-shot.job.yml`, and no `missing.job.yaml` in either root.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test explicit_job_path_bypasses_roots_and_bare_miss_is_named`.
- **ASSERT:** Explicit path is loaded as-is. Miss stderr names `first/missing.job.yaml` and `second/missing.job.yaml`. Both outcomes match exit contracts.

#### Scenario: Listing preserves descriptions and display-only yml

- **GIVEN** `.job.yaml` has a multiline description and `.job.yml` has no description
- **WHEN** no-argument listing runs
- **THEN** descriptions become one line, `.job.yml` stem is listed, and bare lookup probes only `.job.yaml`
- **TEST:** `listing_formats_descriptions_and_yml_is_display_only`
- **SETUP:** Create both files under `jobs/` with valid route sources.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test listing_formats_descriptions_and_yml_is_display_only`.
- **ASSERT:** Newlines become spaces, `(no description)` appears, and bare `.job.yml` lookup exits 2 naming `.job.yaml`.

### Requirement: bounded metadata listing

The system SHALL recursively inspect each configured root up to depth 8 and 512 encountered files per root, in lexical entry order without following symlinked directories, filter candidates with `camel_dsl::discovery::is_job_document`, preserve existing description-only probing, and emit one warning containing `listing truncated at N; narrow [jobs].dirs` when a cap truncates the scan while returning exit 0.

#### Scenario: Recursive job listing

- **GIVEN** a configured root contains nested `.job.yaml` documents and a `.test.yaml` document
- **WHEN** `camel job` lists jobs
- **THEN** it lists the nested job documents and silently skips the test document
- **TEST:** `job_listing_recurses_and_skips_test_documents`
- **SETUP:** Create nested `.job.yaml` and `.test.yaml` files under one root.
- **ACTION:** Run `cargo test -p camel-cli --test job_signal_test job_listing_recurses_and_skips_test_documents`.
- **ASSERT:** Exact stdout line is `domain/report.job.yaml — nested job`; test document is absent; exit is 0.

#### Scenario: Depth cap is non-fatal per root

- **GIVEN** a root contains a job at depth 8 and another at depth 9
- **WHEN** `camel job` lists jobs
- **THEN** depth-8 job is listed, depth-9 job is excluded, one warning contains `listing truncated at 8; narrow [jobs].dirs`, and exit is 0
- **TEST:** `job_listing_stops_at_depth_eight`
- **SETUP:** Configure roots `first` and `second`. Treat each root as depth 0. Put depth-8 and depth-9 files in `first`, and a valid job in `second`.
- **ACTION:** Run `cargo test -p camel-cli --test job_signal_test job_listing_stops_at_depth_eight`.
- **ASSERT:** Only depth-8 job and second-root job appear. Stderr has exactly one warning naming `first`, exit is 0.

#### Scenario: File cap is non-fatal per root

- **GIVEN** a root contains 512 encountered files and one additional job file
- **WHEN** `camel job` lists jobs
- **THEN** files within the limit are processed, the additional file is excluded, one warning contains `listing truncated at 512; narrow [jobs].dirs`, and exit is 0
- **TEST:** `job_listing_stops_at_512_files`
- **SETUP:** Configure roots `first` and `second`. In `first`, create lexical files `000.txt` through `510.txt`, `511.job.yaml`, and `512.job.yaml`. The first job is file 512 and the second job is file 513. Put a valid job in `second`.
- **ACTION:** Run `cargo test -p camel-cli --test job_signal_test job_listing_stops_at_512_files`.
- **ASSERT:** File-512 job and second-root job appear, file-513 job does not, one warning names `first`, exit is 0.

#### Scenario: Malformed sibling

- **GIVEN** one candidate has an unreadable job description and another candidate is valid
- **WHEN** `camel job` lists jobs
- **THEN** the malformed candidate renders `(unparseable)` and the valid candidate remains listed
- **TEST:** `malformed_job_sibling_does_not_abort_listing`
- **SETUP:** Create one valid job and one malformed YAML job file.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test malformed_job_sibling_does_not_abort_listing`.
- **ASSERT:** Both entries appear with the malformed entry labeled `(unparseable)` and exit 0.

#### Scenario: Missing root

- **GIVEN** a configured discovery root does not exist or is empty
- **WHEN** `camel job` lists jobs
- **THEN** it behaves like `ls` for that root and exits 0
- **TEST:** `missing_job_root_is_successful`
- **SETUP:** Create `Camel.toml` with a configured root that does not exist.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test missing_job_root_is_successful`.
- **ASSERT:** Creation hint appears and exit status is 0.

#### Scenario: Lexical traversal and symlink exclusion

- **GIVEN** a root has entries `a/`, `b/`, and a symlinked directory
- **WHEN** `camel job` lists jobs
- **THEN** entries follow lexical order and symlinked directories are not traversed
- **TEST:** `job_listing_is_lexical_and_does_not_follow_directory_symlinks`
- **SETUP:** Create `a/` and `b/` inside the root. Create symlink `linked/` inside the root pointing to `outside/` located outside the root. Put jobs in all three targets.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test job_listing_is_lexical_and_does_not_follow_directory_symlinks`.
- **ASSERT:** `a` precedes `b`, the symlink target is absent, and exit is 0.

### Requirement: named job resolution

The system SHALL resolve a document argument through an ordered ladder. An absolute argument SHALL be used as-is without root probing. An explicit-class argument — one containing a path separator or ending (case-insensitively) in `.yaml`, `.yml`, or `.json` — that exists relative to the current working directory SHALL be used as-is: an explicit CWD-relative path wins over root probing. A bare name SHALL never consult the current working directory and SHALL instead probe every configured root with exactly one probe per root, appending `.job.yaml`: the bare-name probe is `<root>/<name>.job.yaml`, root level only. An explicit-class argument that misses the CWD SHALL probe every configured root with exactly one probe per root, joined from the argument as spelled: an argument ending (case-insensitively) in `.yaml`, `.yml`, or `.json` probes `<root>/<argument>` verbatim, and a separator-bearing argument without such a suffix probes `<root>/<argument>.job.yaml`. Probing SHALL perform no normalization and no confinement: probes are plain joins of the argument as spelled, so arguments containing `.` or `..` components or trailing separators probe exactly as joined and appear verbatim in diagnostics. The system SHALL collect every match before selection: exactly one match resolves; zero matches exit 2 with a miss diagnostic naming every probed path; two or more matches exit 2 with a collision diagnostic naming every matching path. Bare-name probing stays `.job.yaml`-only: a bare name never resolves a document nested below a root, and `.job.yml` stays display-only for bare lookup.

#### Scenario: Cross-root collision

- **GIVEN** the same named job exists in both configured roots
- **WHEN** `camel job report` resolves the job
- **THEN** it reports all matching paths as an ambiguity and exits 2
- **TEST:** `named_job_collision_reports_all_matching_paths`
- **SETUP:** Create `Camel.toml` with `dirs = ["first", "second"]`. Put valid `report.job.yaml` files in both roots with distinct route output markers `first-marker` and `second-marker`.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test named_job_collision_reports_all_matching_paths`.
- **ASSERT:** Stderr names both exact files and exit status is 2.

#### Scenario: Nested stem path resolves

- **GIVEN** `daily/ingest.job.yaml` exists nested in a configured root
- **WHEN** `camel job daily/ingest` runs from the project root
- **THEN** the nested document loads and runs, and the exit code is 0
- **TEST:** `nested_stem_path_resolves_across_root`
- **SETUP:** Create `Camel.toml` with `dirs = ["jobs"]` and a valid `jobs/daily/ingest.job.yaml` with an output marker.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test nested_stem_path_resolves_across_root`.
- **ASSERT:** Report outcome is `Completed`, the report's document field names `jobs/daily/ingest.job.yaml`, stderr is empty, and exit status is 0.

#### Scenario: Nested document path resolves verbatim

- **GIVEN** `daily/ingest.job.yaml` exists nested in a configured root and no such path exists relative to the CWD
- **WHEN** `camel job daily/ingest.job.yaml` runs from the project root
- **THEN** the displayed listing spelling resolves verbatim — the probe is `<root>/daily/ingest.job.yaml` — and the exit code is 0
- **TEST:** `nested_document_path_resolves_verbatim`
- **SETUP:** Same fixture as `nested_stem_path_resolves_across_root`.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test nested_document_path_resolves_verbatim`.
- **ASSERT:** Report outcome is `Completed` and exit status is 0.

#### Scenario: CWD-relative existence wins over root probe

- **GIVEN** a separator-bearing argument names an existing file relative to the CWD and the same relative spelling also exists under a configured root, with distinct route output markers
- **WHEN** `camel job <that-path>` runs
- **THEN** the CWD-relative file loads (its marker, not the root copy) and the exit code is 0
- **TEST:** `cwd_relative_existence_wins_over_root_probe`
- **SETUP:** Create a CWD-relative `local/echo.job.yaml` and a same-spelled `jobs/local/echo.job.yaml` with different markers.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test cwd_relative_existence_wins_over_root_probe`.
- **ASSERT:** The report proves the CWD copy ran; the root copy's marker is absent.

#### Scenario: Relative-path cross-root collision names every match

- **GIVEN** `daily/ingest.job.yaml` exists under both configured roots
- **WHEN** `camel job daily/ingest` runs
- **THEN** stderr names both matching nested paths as an ambiguity and the exit code is 2
- **TEST:** `nested_relative_path_collision_names_every_match`
- **SETUP:** Create `Camel.toml` with `dirs = ["first", "second"]` and valid `daily/ingest.job.yaml` files in both roots.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test nested_relative_path_collision_names_every_match`.
- **ASSERT:** Stderr names `first/daily/ingest.job.yaml` and `second/daily/ingest.job.yaml`; exit status is 2.

#### Scenario: Relative-path miss names every probed file

- **GIVEN** no probe for the argument exists in any configured root and the argument does not exist relative to the CWD
- **WHEN** `camel job daily/missing` runs
- **THEN** stderr names every probed path (`<root>/daily/missing.job.yaml` for each root) and the exit code is 2
- **TEST:** `nested_relative_path_miss_names_probes`
- **SETUP:** Create `Camel.toml` with `dirs = ["first", "second"]` and no matching nested documents.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test nested_relative_path_miss_names_probes`.
- **ASSERT:** Stderr names `first/daily/missing.job.yaml` and `second/daily/missing.job.yaml`; exit status is 2.

#### Scenario: An absolute argument is used as-is without root probing

- **GIVEN** a job document exists at an absolute path outside every configured root
- **WHEN** `camel job /abs/path/echo.job.yaml` runs with that absolute argument
- **THEN** the absolute document loads as-is, no root probe occurs, and the exit code is 0; an absolute argument that does not exist fails with the filesystem diagnostic for that path (never a root-probe miss diagnostic)
- **TEST:** `absolute_argument_is_used_as_is_without_probing`
- **SETUP:** Create `Camel.toml` with `dirs = ["jobs"]` and a job document at a tempdir absolute path outside `jobs/`.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test absolute_argument_is_used_as_is_without_probing`.
- **ASSERT:** The absolute document runs (exit 0); a second run with a nonexistent absolute path exits 2 with stderr naming that path and no configured-root probe path.

#### Scenario: Bare names stay root-level

- **GIVEN** only `daily/ingest.job.yaml` exists nested in a configured root and no root-level `ingest.job.yaml` exists
- **WHEN** `camel job ingest` runs
- **THEN** the probe stays root-level (`<root>/ingest.job.yaml`), the miss diagnostic names those probes, and the exit code is 2
- **TEST:** `bare_name_does_not_descend_into_subdirectories`
- **SETUP:** Create `Camel.toml` with `dirs = ["jobs"]` and only the nested document.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test bare_name_does_not_descend_into_subdirectories`.
- **ASSERT:** Stderr names `jobs/ingest.job.yaml` and does not name `daily/ingest.job.yaml`; exit status is 2.

#### Scenario: A bare name never consults the CWD

- **GIVEN** a CWD-relative file named `report` (no extension) exists while a valid root-level `report.job.yaml` also exists in a configured root
- **WHEN** `camel job report` runs
- **THEN** the root document loads — the CWD file is ignored — and the exit code is 0
- **TEST:** `bare_name_ignores_cwd_entries`
- **SETUP:** Create `Camel.toml` with `dirs = ["jobs"]`, a valid `jobs/report.job.yaml`, and a decoy file named `report` in the CWD.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test bare_name_ignores_cwd_entries`.
- **ASSERT:** The report proves the root document ran; the decoy was never read; exit status is 0.

#### Scenario: Arguments probe as spelled, without normalization or confinement

- **GIVEN** an explicit-class argument containing `..` components exists neither relative to the CWD nor at the joined probe location
- **WHEN** `camel job ../outside/ingest` runs
- **THEN** the miss diagnostic names the probe joined exactly as spelled (for example `first/../outside/ingest.job.yaml`) and the exit code is 2
- **TEST:** `probe_is_joined_as_spelled_without_normalization`
- **SETUP:** Create `Camel.toml` with `dirs = ["first"]` and no matching document at the joined probe location.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test probe_is_joined_as_spelled_without_normalization`.
- **ASSERT:** Stderr names `first/../outside/ingest.job.yaml` verbatim; exit status is 2.

### Requirement: job-scoped help

`camel job <NAME> --help` SHALL render the job's declared interface from
the parsed job document instead of clap's subcommand help. The output SHALL
show the resolved document's display name — the configured-root-relative
path when the resolved path lexically strips against a configured root
with more than one remaining component, the file stem otherwise —, the
document `description` (or `(no description)`), the execution mode, the
send target as authored, and
the declared arguments ordered lexically by argument name, one line per
argument, each carrying the argument name, the declared type rendered as
`string` (also when `type:` is omitted), `int`, `bool`, or `enum[...]`
with its comma-separated member list, a `required`/`optional` marker, the
`default` value when one is declared, and the argument `description` when
present. Each argument SHALL render on one line: any maximal run of CR and
LF characters inside a `default` value or `description` SHALL render as a
single space. The type column SHALL pad to the widest rendered type in
that job, using the same per-job width strategy as the name column. A
document whose top-level `args:` block is absent or empty SHALL print
`(no arguments)` in place of the argument table. Display-name matching
SHALL be lexical on the resolved path spelling before canonicalization
and SHALL share the listing's construction, so the two surfaces cannot
drift within one spelling; symlink aliasing SHALL NOT be
identity-resolved. The `job` subcommand
SHALL suppress clap's automatic `--help`/`-h` handling so a present
positional name always reaches the job-scoped path; `--help` without a
name SHALL print the `camel job` usage text, and bare `camel job` SHALL
keep the discovery listing. Help SHALL be a pure projection: it SHALL NOT
boot the route pipeline, write a report, or install signal handlers, and
it SHALL take precedence over `--report` when both are passed. The help
parse SHALL enforce the same structural and declaration checks as
execution parsing (suffix contract, section exclusivity, strict serde
shape, route-source conflict, per-argument declaration validation
including `type` grammar and typed-default coercion, mode spelling,
`timeout` presence) and SHALL NOT resolve `--arg` pairs, apply defaults,
interpolate, or validate execution values (timeout duration, send
scheme). Resolution, structural, and declaration failures SHALL exit 2
with the existing loud diagnostics.

#### Scenario: help with a job name renders the declared interface

- **GIVEN** a discovered job whose document declares `args:` with a
  required argument with a description and an optional argument with a
  default
- **WHEN** `camel job <name> --help` runs
- **THEN** stdout shows the document's display name, the document
  description, the mode, the send target, and one line per declared
  argument with name, the declared type, `required`/`optional`, the
  default when declared, and the description when present, and the exit
  code is 0

#### Scenario: nested job help shows the invocable path

- **GIVEN** `daily/ingest.job.yaml` exists nested in a configured root
- **WHEN** `camel job daily/ingest --help` runs
- **THEN** the first stdout line is `daily/ingest.job.yaml` — the same
  spelling the listing shows — and the exit code is 0

#### Scenario: help renders the enum member list

- **GIVEN** a discovered job declaring `tier: {type: "enum[bronze,gold]"}`
- **WHEN** `camel job <name> --help` runs
- **THEN** the `tier` row's type column shows `enum[bronze,gold]` and the
  exit code is 0

#### Scenario: help type column aligns across rows

- **GIVEN** a discovered job declaring `count: {type: int}` and `tier: {type: "enum[bronze,gold]"}`
- **WHEN** `camel job <name> --help` runs
- **THEN** both rows render their type column padded to the widest type (`enum[bronze,gold]`), so the `required`/`optional` markers start at the same column on every row, and the exit code is 0

#### Scenario: help with a malformed document fails loud

- **GIVEN** a discovered job document that fails structural or declaration
  parsing (for example an unknown field or a malformed argument
  declaration)
- **WHEN** `camel job <name> --help` runs
- **THEN** stderr carries the parse diagnostic (not clap help) and the
  exit code is 2

#### Scenario: help succeeds without satisfied required arguments

- **GIVEN** a schema-valid job document declaring a required argument
  without a default, whose `to` and `timeout` reference `${arg:...}`
  tokens
- **WHEN** `camel job <name> --help` runs without any `--arg`
- **THEN** the interface renders with the `to` token as authored and the
  declared arguments unchanged, and the exit code is 0

#### Scenario: help without an args block prints no arguments

- **GIVEN** a discovered job whose document has no top-level `args:`
  block, or an empty `args: {}` block
- **WHEN** `camel job <name> --help` runs
- **THEN** stdout shows the display name, the description, the mode, the
  send target, and the line `(no arguments)` under `Arguments:`, and the
  exit code is 0

#### Scenario: help does not execute the job

- **GIVEN** a discovered job document with a valid `execute:` section
- **WHEN** `camel job <name> --help` runs
- **THEN** no route boots, no report file is written, no signal handlers
  are installed, and the process exits 0 after printing the interface

#### Scenario: help with an unknown job name fails loud

- **GIVEN** no job resolves under the configured `[jobs].dirs` roots
- **WHEN** `camel job <name> --help` runs
- **THEN** stderr carries the existing not-found diagnostic and the exit
  code is 2

#### Scenario: help flag without a job name prints command usage

- **GIVEN** the `job` subcommand with no positional name
- **WHEN** `camel job --help` runs
- **THEN** stdout shows the `camel job` usage text and the exit code is 0,
  while bare `camel job` still prints the discovery listing

#### Scenario: help takes precedence over report

- **GIVEN** a discovered job document
- **WHEN** `camel job <name> --help --report <file>` runs
- **THEN** the declared interface is printed, no report file is written,
  and the exit code is 0
