# file-component Specification

## Purpose
TBD - created by archiving change file-ancestor-confinement. Update Purpose after archive.
## Requirements
### Requirement: Producer path confinement across intermediate components

The camel-file producer SHALL reject any write whose target path leaves the
configured base directory through an intermediate path component, including
escapes via symlinked components that exist at validation time, for every
`fileExist` strategy (Fail, Append, Override, TryRename, Ignore) and for the
`doneFileName` marker write. Every symlinked intermediate component below the
original configured base directory SHALL be rejected, including symlinks that
resolve to a location inside the canonicalized base. For target paths whose
parent directories do not yet exist, the producer SHALL verify containment
against the nearest existing ancestor of the target. When parent directories
are auto-created, the producer SHALL re-verify containment after creation and
before the first file open or rename, and SHALL NOT write through a parent
that resolves outside the base. The base directory itself MAY be a symlink;
only components below the configured base are subject to symlink rejection.
Lexical rejections (absolute paths, `..` traversal, NUL bytes) remain
unchanged.

#### Scenario: Symlinked intermediate ancestor rejected on every write path

- **GIVEN** a producer base containing `link -> /outside` (symlink to a directory outside the base) and fileName `link/new/file.txt`
- **WHEN** the producer processes a write under any of Fail, Append, Override, or TryRename, or a write with a safe body path but a `doneFileName` resolving through `link`
- **THEN** the producer returns a confinement error naming the violation, no file is created outside the canonicalized base, and no directory (including auto-created parents) is created outside the canonicalized base

#### Scenario: Ignore strategy does not silently succeed through a symlink

- **GIVEN** a producer base containing `link -> /outside` and a fileName resolving through `link` to an already-existing file outside the base
- **WHEN** the producer processes the write with `fileExist=Ignore`
- **THEN** the producer returns a confinement error instead of the early no-op success return

#### Scenario: Nested new directories without symlinks still succeed

- **GIVEN** a producer base with no symlinks below it and fileName `a/b/c.txt`
- **WHEN** the producer processes the write with `autoCreate=true`
- **THEN** parents are created inside the base and the write succeeds

#### Scenario: In-base intermediate symlink also rejected (Unix)

- **GIVEN** (on Unix) a producer base containing a real directory `real/` and a symlink `alias -> real` inside the base, and fileName `alias/new/file.txt`
- **WHEN** the producer processes a write
- **THEN** the producer returns a confinement error, because `alias` is a symlinked component below the configured base, even though it resolves inside the canonicalized base

#### Scenario: Symlinked base directory remains usable

- **GIVEN** a producer whose configured base directory is itself a symlink to a real directory
- **WHEN** the producer processes a write to a symlink-free relative fileName
- **THEN** the write succeeds and confinement is enforced relative to the canonicalized base

