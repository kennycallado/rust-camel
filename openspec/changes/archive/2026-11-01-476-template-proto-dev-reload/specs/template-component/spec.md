## ADDED Requirements

### Requirement: Dev-watch source exposure

A template reload target registered in the `TemplateReloadRegistry` SHALL
expose, as unique absolute file paths, every file in its last committed
dependency closure — the entry and all includes, nested includes included.
The confinement root directory SHALL NOT be reported as a watch source.

#### Scenario: closure with nested include reports all files

- **GIVEN** a route with a `template:file:` endpoint whose entry includes a
  child template, which itself includes a grandchild
- **WHEN** the reload target's watch sources are enumerated
- **THEN** exactly three absolute file paths are reported — the entry, the
  child, and the grandchild — with no duplicates and no directory path

#### Scenario: sources refresh after a reload that grows the closure

- **GIVEN** a committed template set whose closure contains two files
- **WHEN** a reload commits a set whose closure adds a new include
- **THEN** subsequent enumeration reports the new file without re-registering
  the target

#### Scenario: failed reload keeps reporting the committed closure

- **GIVEN** a committed template set
- **WHEN** a reload build fails and no commit happens
- **THEN** enumeration still reports the previously committed closure only
