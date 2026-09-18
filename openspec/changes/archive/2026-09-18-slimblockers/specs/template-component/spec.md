## ADDED Requirements

### Requirement: minijinja engine optionality

camel-template SHALL make `camel-language-minijinja` and `minijinja` optional
dependencies enabled by `dep:` entries in the `default` feature list, with no
other feature keys, so consumers using default features keep today's exact
behavior and closure, while `default-features = false` consumers resolve the
crate without either engine crate in their dependency closure.

#### Scenario: default closure unchanged

- **GIVEN** any consumer depending on camel-template with default features
- **WHEN** the dependency closure is resolved before and after the change
- **THEN** camel-language-minijinja and minijinja are present in both, and
  the camel-cli golden deptree fixture passes without regeneration

#### Scenario: engine-absent build compiles

- **GIVEN** camel-template built with `--no-default-features`
- **WHEN** the crate is compiled
- **THEN** neither camel-language-minijinja nor minijinja appears in the
  resolved dependencies, compilation succeeds, and the exported surface is
  exactly the engine-free set (config and error modules)

#### Scenario: engine-on behavior identical

- **GIVEN** camel-template compiled with default features
- **WHEN** its existing unit and integration tests run
- **THEN** results are identical to the pre-change baseline (no behavioral
  change behind the default set)

### Requirement: engine module boundary

camel-template SHALL cfg-gate on `feature = "default"` exactly the
engine-coupled modules — bundle, closure, component, endpoint, lifecycle,
producer, reload, template_set — plus their lib.rs re-exports
(`TemplateBundle`, `TemplateBundleConfig`, `TemplateComponent`). The public
config and error modules and the crate-private path_util and uri modules
SHALL remain engine-free and ungated.

#### Scenario: engine-free modules stay ungated

- **GIVEN** camel-template compiled with `--no-default-features`
- **WHEN** the crate's public engine-free items are used (limits-config
  types, `TemplateReloadError`)
- **THEN** they resolve without the default feature enabled, and the
  engine-free modules contain no reference to minijinja or
  camel-language-minijinja

#### Scenario: no named re-enable surface this change

- **GIVEN** camel-template's `[features]` table contains only `default`
- **WHEN** a consumer activates any non-default feature combination
- **THEN** no partial engine surface is exposed (the crate is all-or-nothing
  this change; the named re-enable feature and its cfg-key rename are
  deferred to the camel-cli mission that owns the consumer manifests and the
  golden-fixture regeneration)
