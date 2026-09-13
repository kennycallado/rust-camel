# cli-jobs Specification

## Purpose
TBD - created by archiving change cli-jobs. Update Purpose after archive.
## Requirements
### Requirement: execute-section document parsing

The `execute:` section SHALL parse from the reserved `*.job.yaml` /
`*.job.yml` document family, SHALL be mutually exclusive with the
`scenario:` section and the unit-tier vocabulary (`inputs`, `expects`,
`intercepts`, `beans`, `repositories`, `sequence`, `settle`, `env`), and
SHALL declare exactly one route source (`routeFiles`, `routeFilesFromRoot`,
or `routes`) under the family conflict rule. The section SHALL carry a
mandatory `mode`, a mandatory positive humantime `timeout`, and exactly
one `send` action whose target uses the `direct:` or `seda:` scheme. The
document MAY declare an optional top-level `description` (string), shown
by job listing and rejected nowhere. Bodies SHALL be restricted to
string, object, and array forms. A document named `*.test.yaml` /
`*.test.yml` that declares `execute:` SHALL fail to load under
`camel job` with an error directing the author to rename the file to the
`.job.yaml` suffix.

#### Scenario: valid one-shot document parses

- **GIVEN** a `*.job.yaml` document whose only sections are `execute:`
  (mode `one-shot`, a `timeout`, one `direct:` send) and one route
  source key
- **WHEN** `camel job` loads the document
- **THEN** parsing succeeds and the route source resolves under the
  family semantics (`routeFiles` against the document directory,
  `routeFilesFromRoot` against the nearest ancestor `Camel.toml`)

#### Scenario: execute is mutually exclusive with scenario and expects

- **GIVEN** a `*.job.yaml` document declaring `execute:` together with
  `scenario:` or any unit-tier section such as `expects:`
- **WHEN** the document is loaded by `camel job`
- **THEN** loading fails with a mutual-exclusion error naming the mixed
  sections (test vocabulary does not belong in a job document)

#### Scenario: test-suffix document declaring execute is rejected

- **GIVEN** a `*.test.yaml` document declaring `execute:`
- **WHEN** `camel job` is asked to load it, or `camel test` dispatches
  it
- **THEN** `camel job` fails the load with an error directing the author
  to rename the file to `.job.yaml`, `camel test` refuses the document
  with a pointer to `camel job`, and the process exits with code 2

#### Scenario: missing or invalid timeout is rejected

- **GIVEN** a one-shot document whose `execute:` section omits `timeout`
  or declares a non-positive or unparsable value
- **WHEN** `camel job` loads the document
- **THEN** loading fails with an error naming `execute.timeout`, and the
  process exits with code 2

### Requirement: fail-closed consumer scheme allowlist

At load time, `camel job` SHALL reject documents whose discovered route
definitions consume (`from:` URI) from any scheme outside the job-safe
allowlist `{direct, seda, log, mock}`. Producer and sink `to:` URIs
SHALL NOT be restricted.

#### Scenario: non-job-safe consumer scheme is rejected

- **GIVEN** a job document whose routes include a route with
  `from: kafka:topic` (or any scheme outside the allowlist)
- **WHEN** `camel job` loads the routes
- **THEN** the document is rejected with an error naming the offending
  route, its from-URI, and the allowlist, and the process exits with
  code 2

#### Scenario: producers are unrestricted

- **GIVEN** a job document whose target route consumes from
  `direct:in` and contains `to: http://...` and `to: log:out` steps
- **WHEN** `camel job` loads the routes
- **THEN** the consumer gate passes and the job runs

### Requirement: exit-code taxonomy and JSON report

`camel job` SHALL exit 0 when the pipeline completes, 1 when the
pipeline fails, and 2 for load, validation, boot, interruption,
drain-timeout, shutdown, and report-write errors; when classes mix, exit
2 SHALL outrank exit 1. A first SIGINT or SIGTERM SHALL cancel an in-flight
send or batch drain, perform bounded teardown, and report outcome
`Interrupted`. A second SIGINT or SIGTERM received during that teardown SHALL
force-exit with code 1. The signal streams SHALL be armed before config
loading so signals during boot are buffered rather than default-killing the
process. For outcomes that reach the send (verdict, interruption, or
timeout), and for shutdown failures after a recorded verdict, the command
SHALL emit a JSON report
(`{document, mode, outcome, terminated_early, duration_ms, reply?, error?, shutdown_error?}`)
to stdout, or to the `--report` path when given; early exit-2 classes remain
stderr-only. When `capture-reply` is set and a reply exchange is returned,
the report SHALL carry the reply body and headers. A batch drain timeout
SHALL report outcome `Timeout` and preserve the existing exit-2 behavior.
When a signal event is ready at the same poll point as send completion or
deadline expiry, signal handling SHALL win and report `Interrupted`. The
implementation SHALL use signal-first selection ordering to make this
precedence deterministic.

#### Scenario: first signal interrupts an active job

- **GIVEN** a `camel job` process has booted and its send is in flight
- **WHEN** the operator sends SIGINT or SIGTERM once
- **THEN** the send is cancelled, teardown runs under at least
  `MIN_SHUTDOWN_BUDGET`, the report outcome is `Interrupted`, and the
  process exits with code 2

#### Scenario: signal during boot is buffered

- **GIVEN** a `camel job` process is loading config or booting components
- **WHEN** the operator sends SIGINT or SIGTERM
- **THEN** the signal is handled by the job contract after boot reaches its
  signal-aware phase, rather than default-killing the process

On non-Unix platforms, the portable Ctrl+C listener is installed when first
polled; the boot-buffer guarantee applies to Unix SIGINT/SIGTERM streams.

#### Scenario: second signal force-exits during teardown

- **GIVEN** the first signal was consumed and bounded job teardown is running
- **WHEN** a second SIGINT or SIGTERM arrives before teardown completes
- **THEN** the process exits with code 1 without waiting for teardown

#### Scenario: interruption keeps apparatus precedence

- **GIVEN** a first signal interrupts a job and teardown also reports an error
- **WHEN** the JSON report is emitted
- **THEN** outcome remains `Interrupted`, shutdown detail is recorded, and
  the process exits with code 2

#### Scenario: signal interrupts batch drain

- **GIVEN** a batch job's send completed and its drain wait is still active
- **WHEN** the operator sends SIGINT or SIGTERM once
- **THEN** the drain wait is cancelled, teardown runs under the existing
  batch overall-deadline budget without a floor, the report outcome is
  `Interrupted`, and the process exits with code 2

#### Scenario: completed job with captured reply

- **GIVEN** a job completes and `capture-reply` is enabled
- **WHEN** the report is emitted
- **THEN** it contains outcome `Completed`, the captured reply body and
  headers, no `error` or `shutdown_error` for successful teardown, and the
  process exits 0

#### Scenario: failed pipeline exits 1

- **GIVEN** the target route pipeline fails during execution
- **WHEN** the report is emitted
- **THEN** it contains outcome `Failed` with a non-empty error and the
  no shutdown detail when teardown succeeds, and the process exits 1

#### Scenario: timeout and shutdown failure preserve precedence

- **GIVEN** send or batch drain exceeds the mandatory overall timeout, or
  teardown fails after a recorded verdict
- **WHEN** the report is emitted
- **THEN** timeout reports outcome `Timeout`, shutdown failure records
  `shutdown_error` only when teardown had a non-zero remaining budget, no
  shutdown detail when teardown succeeds, and either case exits 2

#### Scenario: shutdown failure after a verdict preserves both details

- **GIVEN** a pipeline verdict was recorded and teardown fails with a
  non-zero remaining budget
- **WHEN** the report is emitted
- **THEN** `error` preserves the pipeline or timeout verdict,
  `shutdown_error` carries teardown detail, and the process exits 2

#### Scenario: signal-first tie is deterministic

- **GIVEN** a signal and send completion become ready at the same poll point
- **WHEN** the command selects its next operation
- **THEN** the signal branch wins and the report outcome is `Interrupted`

#### Scenario: zero-budget shutdown failure is stderr-only

- **GIVEN** the overall deadline has already expired and teardown has no
  remaining budget
- **WHEN** shutdown reports its expected zero-budget failure
- **THEN** the report keeps outcome `Timeout`, the shutdown detail is written
  to stderr without replacing the verdict, and the process exits 2

#### Scenario: batch drain completion reports Completed

- **GIVEN** a batch job's send completes and all expected SEDA queues drain
- **WHEN** the report is emitted
- **THEN** it contains mode `batch`, outcome `Completed`, and the process
  exits 0

#### Scenario: shutdown failure after a verdict forces exit 2

- **GIVEN** a job whose pipeline completed but whose teardown fails or
  exceeds its budget
- **WHEN** the report is emitted
- **THEN** the report carries the shutdown detail in `shutdown_error` and
  the process exits 2

#### Scenario: failed verdict plus shutdown failure keeps both details

- **GIVEN** a job whose pipeline failed and whose teardown also fails
- **WHEN** the report is emitted
- **THEN** `error` holds the pipeline failure, `shutdown_error` holds the
  teardown detail, and the process exits 2

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

### Requirement: one-shot send with load-gated route startup

`camel job` SHALL boot the real composition root (the `camel run` seams),
SHALL force `auto_startup = true` on every discovered route in the job
document (including routes configured `autoStartup: false`), and SHALL
reject a send target with no matching consumer route at load, as well as a
send target whose base is ambiguous across multiple consumer routes. Route
side-effect safety SHALL come from the fail-closed consumer scheme
allowlist at load: every consumer entry point in a job document is
internal by construction, while producer and sink `to:` endpoints remain
unrestricted. The send target SHALL remain the single entry point. The job
SHALL send exactly one exchange to the document's target and SHALL wrap
send, drain, and teardown in the mandatory overall `timeout` (anchored at
process start, covering boot). For `seda:` targets the send SHALL carry
`waitForTaskToComplete=Always` so the producer awaits the pipeline result
regardless of exchange pattern.

#### Scenario: all document routes start

- **GIVEN** a job document whose routes include the send target's
  `direct:` consumer route, a helper `direct:` consumer route the target
  hops to (configured `autoStartup: false`), and one unrelated `seda:`
  consumer route
- **WHEN** the job starts the context
- **THEN** every document route's consumer starts (including the
  `autoStartup: false` helper), and the send target
  remains the sole entry point the job injects into

#### Scenario: direct helper route participates end to end

- **GIVEN** a job document whose target route hops with
  `to: direct:enrich` to a second route that transforms the exchange
- **WHEN** `camel job` runs the document
- **THEN** the hop resolves: the helper route started, its consumer is
  registered, and the job completes with exit 0

#### Scenario: seda send is synchronous

- **GIVEN** a job document targeting `seda:work` whose route pipeline
  fails during execution
- **WHEN** the send completes
- **THEN** the failure surfaces as the job's outcome (exit 1, outcome
  `Failed`), not a fire-and-forget `Completed`

#### Scenario: overall timeout expiry

- **GIVEN** a job whose send does not complete within the declared
  `timeout`
- **WHEN** the deadline expires
- **THEN** the JSON report carries outcome `Timeout` with a
  drain-timeout-class error and the process exits with code 2

### Requirement: supported job execution modes

The `mode` key SHALL accept `one-shot` and `batch`. Any other value SHALL
be rejected at load with an unsupported-mode error naming both accepted
values, and the process SHALL exit with code 2 before any boot.

#### Scenario: unsupported mode is rejected at load

- **GIVEN** a job document declaring `mode: stream` (or any value other
  than `one-shot` and `batch`)
- **WHEN** `camel job` loads the document
- **THEN** loading fails with an unsupported-mode error naming the
  accepted values and the process exits with code 2 before any boot

### Requirement: arg flag header injection

`camel job` SHALL accept a repeatable `--arg NAME=VALUE` flag. Each pair
SHALL inject one string header onto the trigger exchange at send time,
applied after the document's `send.headers`, so a CLI value overrides a
document header with the same name. When the same name repeats on the
command line, the last occurrence SHALL win. Values SHALL be raw strings
with no interpolation. A malformed pair (no `=`, or an empty name) SHALL
fail as a usage error with exit 2 before any boot. The flag SHALL work in
both execution modes.

#### Scenario: single and repeated args reach the route

- **GIVEN** a job document whose target route records exchange headers to
  a `mock:` endpoint
- **WHEN** `camel job` runs the document with `--arg name=John` and
  `--arg tier=gold`
- **THEN** the job exits 0 and the recorded exchange carries headers
  `name=John` and `tier=gold`

#### Scenario: arg overrides a colliding document header

- **GIVEN** a job document whose `send.headers` declares `name: Doc` and
  whose target route records exchange headers to a `mock:` endpoint
- **WHEN** `camel job` runs the document with `--arg name=Cli`
- **THEN** the job exits 0 and the recorded header `name` holds `Cli`

#### Scenario: malformed arg is a usage error

- **GIVEN** an `--arg` value with no `=` (for example `nameonly`) or an
  empty name (for example `=value`)
- **WHEN** `camel job` runs the document with that flag value
- **THEN** the process exits 2 with a usage error before any boot

### Requirement: batch mode drains until empty

A `mode: batch` job SHALL boot the same composition root, apply the same
send-target validation, start all document routes, and send the same
single trigger exchange as a one-shot job, including the `seda:`
`waitForTaskToComplete=Always` rewrite and `--arg` header injection.
After the trigger send the job SHALL drain: it SHALL wait until every
`seda:` consumer queue of the document's routes is empty, then emit the
JSON report with outcome `Completed` and mode `batch`, and exit 0. A
batch document with no `seda:` consumer routes SHALL complete immediately
after the trigger send. Zero-depth observations made before the trigger
send completes SHALL NOT satisfy the drain. The mandatory overall
`timeout` SHALL still bound boot, send, drain, and teardown; expiry
reports outcome `Timeout` with exit 2. A trigger-send pipeline failure
reports `Failed` with exit 1 under the existing taxonomy.

#### Scenario: batch drains a fan-out pipeline then exits 0

- **GIVEN** a batch job whose target route fans messages out through one
  or more `seda:` queues to worker routes
- **WHEN** `camel job` runs the document
- **THEN** the job waits until every document `seda:` queue is empty, the
  report shows outcome `Completed` with mode `batch`, and the process
  exits 0

#### Scenario: batch does not complete while work is in flight

- **GIVEN** a batch job whose worker route holds each exchange in flight
  for a short delay before recording it to a `mock:` endpoint
- **WHEN** the drain observes a queue-depth sample of zero while the
  exchange is still in flight
- **THEN** the job does not complete from that sample: exit happens only
  after the worker recorded the exchange

#### Scenario: batch overall timeout expiry

- **GIVEN** a batch job whose traffic does not quiesce within the
  declared `timeout`
- **WHEN** the deadline expires
- **THEN** the JSON report carries outcome `Timeout` with a
  drain-timeout-class error and the process exits with code 2

#### Scenario: batch works with arg injection

- **GIVEN** a batch job whose worker routes record exchange headers to a
  `mock:` endpoint
- **WHEN** `camel job` runs the document with `--arg batch-id=42`
- **THEN** the job drains, exits 0, and the recorded exchanges carry
  header `batch-id=42`

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

