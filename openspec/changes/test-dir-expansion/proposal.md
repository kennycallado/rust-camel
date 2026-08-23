# Proposal: test-dir-expansion

## Why

`camel test` accepts explicit FILE paths only (`TestArgs.files`,
`crates/camel-cli/src/commands/test.rs`). A directory argument fails with
"Is a directory" per arg. Running a suite means naming every document or
shell globbing (expansion outside the tool, no recursion, no portability).

bd issue: rc-adrq (demo-prep feedback, third request).

## What Changes

- `camel test` accepts FILE and DIRECTORY arguments (value_name
  `FILE|DIR`). A directory expands to the `*.test.yaml` / `*.test.yml`
  documents it contains, recursively, byte-sorted for determinism,
  reusing `camel_dsl::discovery::is_test_document` (single source of the
  reserved-suffix rule; no re-implementation).
- Walk exclusions: `target`, `.git`, `node_modules` directories are
  skipped at any depth.
- A directory that expands to zero test documents is a misuse error
  (exit 2 class) naming the directory.
- Duplicate documents (same file reached twice, e.g. explicit file plus
  its directory) are deduplicated; first occurrence order wins.
- Plain FILE arguments keep today's behavior unchanged (verbatim,
  parsed whatever their suffix).

## Impact

- Code: `crates/camel-cli/src/commands/test.rs` (expansion helper +
  `run_tests` integration + `TestArgs` doc/value_name), unit tests with
  `tempfile`.
- Docs: `crates/camel-cli/README.md` usage line.
- Spec: `mock-testkit` ADDED requirement (directory arguments).
- No `camel run` interaction: expansion lives in `camel test` only and
  reads the file system; route discovery is untouched.
