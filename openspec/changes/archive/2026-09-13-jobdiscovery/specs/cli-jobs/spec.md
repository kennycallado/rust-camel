## MODIFIED Requirements

### Requirement: job discovery and no-argument listing

The system SHALL accept a `[jobs]` table with ordered `dirs`, defaulting to `["jobs"]`, and reject unknown keys. Legacy `dir` SHALL fold into one `dirs` entry. Each root SHALL resolve against the `Camel.toml` root. A bare name SHALL probe only `<name>.job.yaml`, report a miss with the probed path, and use the first matching root when exactly one match exists. Multiple matching roots SHALL produce an exit-2 collision naming all paths. Explicit paths and recognized document suffixes SHALL bypass root probing. A job SHALL retain its mandatory explicit route source and SHALL never inherit `routes/` discovery. No-argument listing SHALL show `.job.yaml` and `.job.yml` stems, descriptions or `(no description)`, replace embedded newlines with spaces, tolerate malformed siblings as `(unparseable)`, print the existing creation hint for absent or empty roots, and reject listing-only report options with exit 2. Listing and run reports SHALL never co-occur.
When both `dir` and `dirs` are present, `dirs` SHALL take precedence and `dir` SHALL not add a duplicate root. Recursive listing SHALL preserve the existing root-level output line for files directly under a configured root. For nested files, it SHALL prefix the stem with the configured-root-relative path as `<relative-path>: <stem> — <description>`. Bare-name resolution SHALL remain root-level only.

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

## ADDED Requirements

### Requirement: bounded metadata listing

The system SHALL recursively inspect each configured root up to depth 8 and 512 encountered files per root, in lexical entry order without following symlinked directories, filter candidates with `camel_dsl::discovery::is_job_document`, preserve existing description-only probing, and emit one warning containing `listing truncated at N; narrow [jobs].dirs` when a cap truncates the scan while returning exit 0.

#### Scenario: Recursive job listing

- **GIVEN** a configured root contains nested `.job.yaml` documents and a `.test.yaml` document
- **WHEN** `camel job` lists jobs
- **THEN** it lists the nested job documents and silently skips the test document
- **TEST:** `job_listing_recurses_and_skips_test_documents`
- **SETUP:** Create nested `.job.yaml` and `.test.yaml` files under one root.
- **ACTION:** Run `cargo test -p camel-cli --test job_signal_test job_listing_recurses_and_skips_test_documents`.
- **ASSERT:** Exact stdout line is `domain/report.job.yaml: report — nested job`; test document is absent; exit is 0.

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
- **SETUP:** Configure roots `first` and `second`. In `first`, create lexical files `000.txt` through `510.txt`, `511.job.yaml`, and `512.job.yaml`. The first job is file 512 and the second is file 513. Put a valid job in `second`.
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

The system SHALL probe `<name>.job.yaml` in every configured root, detect cross-root stem collisions as exit 2 errors naming all matching files, and otherwise select the first match in declared root order.

#### Scenario: Cross-root collision

- **GIVEN** the same named job exists in both configured roots
- **WHEN** `camel job report` resolves the job
- **THEN** it reports all matching paths as an ambiguity and exits 2
- **TEST:** `named_job_collision_reports_all_matching_paths`
- **SETUP:** Create `Camel.toml` with `dirs = ["first", "second"]`. Put valid `report.job.yaml` files in both roots with distinct route output markers `first-marker` and `second-marker`.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test named_job_collision_reports_all_matching_paths`.
- **ASSERT:** Stderr names both exact files and exit status is 2.

### Requirement: metadata-only listing boundary

The system SHALL not invoke route discovery, environment interpolation, or security compilation while performing no-argument listing.

#### Scenario: Route pipeline remains unused

- **GIVEN** a job document contains values that would require route interpolation or security context
- **WHEN** `camel job` lists the discovery set
- **THEN** listing uses only filesystem filtering and description probing, prints the description, emits no interpolation or security error, and exits 0
- **TEST:** `job_listing_does_not_boot_route_pipeline`
- **SETUP:** Create `sentinel.job.yaml` with `description: safe listing`, `${env:JOB_DISCOVERY_MUST_NOT_RUN}` in an unused body field, and `security_policy: __invalid_listing_sentinel__` in its route source.
- **ACTION:** Run `cargo test -p camel-cli --test job_one_shot_test job_listing_does_not_boot_route_pipeline`.
- **ASSERT:** stdout contains `sentinel` and `safe listing`, stderr contains neither `JOB_DISCOVERY_MUST_NOT_RUN` nor `__invalid_listing_sentinel__`, and exit status is 0.
