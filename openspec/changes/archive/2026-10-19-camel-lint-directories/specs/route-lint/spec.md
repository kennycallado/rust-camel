## ADDED Requirements

### Requirement: Directory target discovery

The `camel lint` CLI SHALL accept a directory path: it SHALL recursively
collect every regular file below that directory whose name ends in `.yaml`,
`.yml`, or `.json`, matched case-sensitively, and lint each collected file.
An entry is a collection candidate only when `std::fs::metadata` (which
follows symlinks) reports it as a file; directories are descended, and any
other entry kind (FIFO, socket, device) is ignored. A symlinked file is
collected and linted through its target; a symlinked directory is never
descended. A dangling symlink with an eligible extension SHALL be treated
as a per-file read failure. Paths SHALL be built from the user-supplied
root without canonicalization. When the root path does not exist, the CLI
SHALL exit 2 with a CLI-error message, as it does for a missing file today.
A root that resolves to a file SHALL enter single-file mode unchanged.

#### Scenario: recursion lints the exact eligible set

- **GIVEN** a tree two levels deep containing `a.yaml`, `sub/b.yml`,
  `sub/c.json`, plus `notes.txt` and `D.YAML`
- **WHEN** `run_lint_dir` runs over the tree root
- **THEN** exactly `a.yaml`, `sub/b.yml`, and `sub/c.json` are linted and
  neither `notes.txt` nor `D.YAML` is collected

#### Scenario: dangling eligible symlink is a read failure

- **GIVEN** a unix filesystem where the tree contains `broken.yaml`, a
  symlink pointing at a nonexistent path
- **WHEN** the walk runs
- **THEN** `broken.yaml` is reported as a failed file on stderr and
  contributes to the exit-2 bucket

#### Scenario: missing root exits two

- **GIVEN** a path that does not exist
- **WHEN** `camel lint missing-dir/` runs
- **THEN** the process exits 2 with a CLI-error message

### Requirement: Structural pruning below the root

The directory walker SHALL prune, below the root only: every entry whose
file name starts with `.`, and every directory named `target`. It SHALL NOT
descend into symlinked directories, SHALL lint symlinked files, and SHALL
apply no pruning rule to the root itself. The walk SHALL terminate on
symlink cycles.

#### Scenario: hidden and target entries are pruned

- **GIVEN** a root containing `.hidden/r.yaml`, `target/f.json`, and
  `routes/g.yaml`
- **WHEN** the walker collects targets
- **THEN** only `routes/g.yaml` is collected

#### Scenario: a hidden or target root is still linted

- **GIVEN** the root passed on the command line is `<tree>/.hidden` or
  `<tree>/target`, each containing one eligible file
- **WHEN** `camel lint` runs on that root
- **THEN** the eligible file inside is linted

#### Scenario: symlinked directory is not descended, symlinked file is linted

- **GIVEN** a unix filesystem where the tree contains a symlinked
  subdirectory (pointing back at the root, forming a cycle) and a symlinked
  `.yaml` file pointing to a real route file
- **WHEN** the walker collects targets
- **THEN** the symlinked file is collected exactly once and no symlinked
  directory is descended, so the walk terminates

#### Scenario: non-regular entry is ignored

- **GIVEN** a unix filesystem where the tree contains a FIFO named
  `pipe.yaml`
- **WHEN** the walker collects targets
- **THEN** `pipe.yaml` is not collected and the walk does not block

#### Scenario: unreadable subdirectory is reported

- **GIVEN** a subdirectory that cannot be read (permission denied; the
  test is skipped when running as root, which bypasses permission bits)
- **WHEN** the walk runs
- **THEN** the failure is reported on stderr, counted as failed, and
  contributes to the exit-2 bucket while the rest of the tree is still
  walked

### Requirement: Reserved-document skipping in directory mode

In directory mode the CLI SHALL apply the shared reserved-document
predicate (`camel_dsl::discovery::is_reserved_document`, the ADR-0062
suffix contract) to every collected file before linting. The mission term
"fixture/reserved document" means exactly this suffix predicate; no file is
skipped because of its directory name. A reserved document SHALL NOT be
linted, SHALL produce an info line on stdout using the same wording as
single-file mode, and SHALL be counted as skipped in the summary.

#### Scenario: reserved documents inside the tree are not linted

- **GIVEN** a tree containing one clean route, one `demo.test.yaml`, and
  one `demo.job.yml`
- **WHEN** `run_lint_dir` runs over the tree root
- **THEN** the route is linted, neither reserved document is linted, an
  info line names each reserved document, and the exit code is 0

#### Scenario: directory with only reserved documents exits zero

- **GIVEN** a directory whose only eligible files are reserved documents
- **WHEN** `run_lint_dir` runs
- **THEN** zero files are linted, all are reported skipped, and the exit
  code is 0

### Requirement: Deterministic ordering

Directory mode SHALL visit collected targets in depth-first pre-order with
each directory's entries sorted by file-name bytes (`OsStr`), and SHALL
render each file's diagnostics in engine order. Repeated runs over the
same tree SHALL produce byte-identical output.

#### Scenario: visit order is sorted depth-first

- **GIVEN** a tree where one directory contains `b.yaml` and `a-b.yaml`,
  and a sibling subdirectory `a/` contains `z.yaml`
- **WHEN** the walker collects targets
- **THEN** the visit order is `a/z.yaml`, `a-b.yaml`, `b.yaml`

#### Scenario: repeated runs are byte-identical

- **GIVEN** a tree with several eligible files producing diagnostics
- **WHEN** directory mode runs twice
- **THEN** both runs produce byte-identical stdout and stderr

### Requirement: Aggregated exit code

In directory mode the CLI SHALL exit with the highest applicable code:
2 when the root is missing, engine construction fails, any per-file read
fails (including non-UTF-8 content and dangling eligible symlinks), or a
subdirectory cannot be read; 1 when at least one error-severity diagnostic
is emitted across all files; 0 otherwise. Root and engine failures SHALL
abort before the walk and print no summary. The walk SHALL continue after
per-file failures so the report covers every file.

#### Scenario: one error file makes the tree exit one

- **GIVEN** a tree of clean routes where exactly one file produces an
  error-severity diagnostic
- **WHEN** `run_lint_dir` runs over the tree root
- **THEN** the aggregated exit code is 1

#### Scenario: all-clean tree exits zero

- **GIVEN** a tree where every eligible file lints clean
- **WHEN** `run_lint_dir` runs over the tree root
- **THEN** the aggregated exit code is 0

#### Scenario: per-file read failure exits two and the walk continues

- **GIVEN** a unix filesystem tree with one file containing invalid UTF-8,
  one dangling symlink named `broken.yaml`, and one file with an
  error-severity diagnostic
- **WHEN** `run_lint_dir` runs over the tree root
- **THEN** every file is reported (exactly two stderr lines beginning with
  `error: `, one diagnostic), the walk covers all three files, and the
  aggregated exit code is 2

### Requirement: Directory summary output

In directory mode the CLI SHALL print exactly one summary line to stdout,
of the form `linted N file(s), M skipped, F failed, K diagnostic(s)`.
N counts files read and linted (failed files excluded), M counts reserved
documents skipped, F counts per-file read failures (including dangling
eligible symlinks and unreadable subdirectories), and K counts diagnostics
of all severities. The line SHALL contain no timings. Single-file mode
SHALL NOT print a summary line. When the collected target set is empty
(before the reserved filter, so a reserved-only directory does not warn),
the CLI SHALL print a `warning:` line to stderr, exit 0 unless a higher
exit bucket applies, and print the summary with zero linted counts.

#### Scenario: summary counts every outcome

- **GIVEN** a unix filesystem tree with two clean routes, one route with a
  warning, one reserved document, and one dangling symlink named
  `broken.yaml`
- **WHEN** directory mode runs
- **THEN** stdout ends with `linted 3 file(s), 1 skipped, 1 failed,
  1 diagnostic(s)`

#### Scenario: single-file mode prints no summary

- **GIVEN** a single route file
- **WHEN** file mode runs
- **THEN** no stdout or stderr line matches
  `^linted \d+ file\(s\), \d+ skipped, \d+ failed, \d+ diagnostic\(s\)$`

#### Scenario: empty directory warns and exits zero

- **GIVEN** an empty directory
- **WHEN** `camel lint empty-dir/` runs
- **THEN** a `warning:` line is printed to stderr and the exit code is 0

### Requirement: Single-file mode unchanged

The `camel lint` CLI SHALL keep single-file mode byte-identical to its
behavior before this change: file-mode dispatch, diagnostics rendering,
skip info wording, and exit codes 0/1/2 stay exactly as specified in the
`camel lint` CLI runs the engine and exits by severity requirement. The
directory feature SHALL be reachable only when the given path resolves to
a directory.

#### Scenario: existing single-file cases keep their outcomes

- **GIVEN** the existing single-file fixtures: a clean route, a route with
  an error diagnostic, a missing path, and a reserved document
- **WHEN** each is linted in file mode
- **THEN** the outcomes are, in order: exit 0 with no diagnostics, exit 1
  with the diagnostic, exit 2 with a CLI error, and exit 0 with the skip
  info line

#### Scenario: file-mode output is byte-identical to the pre-change binary

- **GIVEN** golden stdout and stderr captures recorded from the
  pre-change binary for the clean, error, missing, and reserved single-file
  cases
- **WHEN** the same cases run in file mode after this change
- **THEN** stdout and stderr are byte-identical to the golden captures
