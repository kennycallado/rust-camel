# Tasks: audit-fix-docdrift-t2t3

## Task 1: Fix camel-endpoint README derive-macro example

**Files:**
- `crates/camel-endpoint/README.md`

**Steps:**
1. Read `crates/camel-endpoint/README.md:44-59` (the UriConfig example block)
2. Read `crates/camel-endpoint/camel-endpoint-macros/src/lib.rs` to confirm correct attribute names
3. Fix `#[uri(default = "*")]` → `#[uri_param(default = "*")]`
4. Fix `#[uri(rename = "repeatCount")]` → `#[uri_param(name = "repeatCount")]`
5. Add `#[uri_scheme = "timer"]` above `struct TimerConfig`

**Tests:**
- Name: readme-attribute-names-match-macro
- Arrange: Read README.md lines 49-56 and macro lib.rs attribute definitions
- Act: Compare attribute names used in README example with actual macro accepted attributes
- Assert: Every attribute in the README example exists in the macro's accepted attribute set

**Acceptance:**
- `#[uri_scheme]`, `#[uri_param]` are the correct attribute names (not `#[uri]`)
- The example structurally matches what the macro expects

- [x] 1

## Task 2: Add #[uri_config(crate)] to camel-endpoint-macros rustdoc

**Files:**
- `crates/camel-endpoint/camel-endpoint-macros/src/lib.rs`

**Steps:**
1. Read the rustdoc `## Struct-level attributes` section (lines 18-20)
2. Add a third bullet: `#[uri_config(crate = "path")]` — default `camel_endpoint`; component crates use `camel_component_api`

**Tests:**
- Name: rustdoc-lists-all-three-attributes
- Arrange: Read lib.rs rustdoc section
- Act: Count attribute bullets
- Assert: Three bullets present (`#[uri_scheme]`, `#[uri_param]`, `#[uri_config]`)

**Acceptance:**
- docs.rs rustdoc surface lists all three struct-level attributes

- [x] 2

## Task 3: Fix camel-bean README (BeanError + register + ProcessorError)

**Files:**
- `crates/camel-bean/README.md`

**Steps:**
1. Read `crates/camel-bean/src/error.rs` for the actual BeanError variants and From impl
2. Add `InvalidName` variant to the README BeanError enum block (`:179-194`)
3. Fix ProcessorError line (`:206`): `CamelError::ProcessorError(err.to_string())` → `CamelError::ProcessorErrorWithSource(err.to_string(), Arc::new(err))`
4. Add `?` to `register()` calls at lines 49, 285, 287

**Tests:**
- Name: readme-beanerror-variants-match-source
- Arrange: Read error.rs BeanError enum definition and README enum block
- Act: Compare variant sets
- Assert: README lists all variants including InvalidName

- Name: readme-register-calls-have-result-propagation
- Arrange: Grep README for `.register(`
- Act: Check each match has `?` suffix
- Assert: All register() calls propagate the Result

**Acceptance:**
- README BeanError variants match source code
- ProcessorErrorWithSource signature is correct
- All register() calls use `?`

- [x] 3

## Task 4: Fix camel-bean-macros lib.rs doc-comments

**Files:**
- `crates/camel-bean-macros/src/lib.rs`

**Steps:**
1. Read lines 27-28 and line 32
2. Change "detected by the Bean derive macro" → "detected by `#[bean_impl]`"
3. Delete the dead "Task 2.2" reference in the line 32 inline comment

**Tests:**
- Name: doc-comment-references-correct-detector
- Arrange: Read lib.rs doc comment
- Act: Check whether it says `#[bean_impl]` not "Bean derive macro"
- Assert: Comment names the real detector

**Acceptance:**
- Doc-comment accuracy: references `#[bean_impl]` as the detector
- No dead task references

- [x] 4

## Task 5: Fix camel-test README (.with_seda + unused import)

**Files:**
- `crates/camel-test/README.md`

**Steps:**
1. Read the builder methods table (`:77-84`)
2. Add row: `| .with_seda() | Registers SedaComponent |`
3. Read examples at lines 18 and 48
4. Remove `StepAccumulator` from unused import lines

**Tests:**
- Name: readme-lists-with-seda
- Arrange: Read README builder methods section
- Act: Grep for `with_seda`
- Assert: `.with_seda()` appears in the methods table

**Acceptance:**
- Builder methods table includes `.with_seda()`
- Examples don't import unused `StepAccumulator`

- [x] 5
