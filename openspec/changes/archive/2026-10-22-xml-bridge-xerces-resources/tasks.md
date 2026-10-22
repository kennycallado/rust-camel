# Tasks: xml-bridge-xerces-resources

Single phase. Build commands run the project Docker native build; `$WT` is the worktree root. Native tests need `CAMEL_XML_BRIDGE_BINARY_PATH=$WT/bridges/xml/build/native/xml-bridge`.

## 1. Package the Xerces regex bundle (spec: invalid pattern, valid pattern family, regex bundle English)

**Files**
- `bridges/xml/src/main/resources/application.yml` (modified)

**Steps**
1. In `quarkus.native.resources.includes`, keep `org/apache/xerces/impl/msg/*` once (delete the second occurrence after `tls/placeholder-server.key`).
2. Add `org/apache/xerces/impl/xpath/regex/message*.properties` directly after the first `org/apache/xerces/impl/msg/*` entry.
3. Replace the Xerces comment above the list with: the two bundle locations, the command `unzip -l xercesImpl-*.jar | grep '\.properties'` to re-verify on a Xerces bump, and the fact that entries in `META-INF/native-image/resource-config.json` did not reach the image (verified with glob and with pattern entries).

**Tests** (native, `crates/camel-test/tests/xml_bridge_xerces_diagnostics_test.rs`, see task 3)
- `invalid_pattern_surfaces_real_message`, `valid_patterns_compile`, `bundle_present`.
- command: `CAMEL_XML_BRIDGE_BINARY_PATH=$WT/bridges/xml/build/native/xml-bridge cargo test -p camel-test --features integration-tests --test xml_bridge_xerces_diagnostics_test`
- expected before the change (binary built from main): all three FAIL with `Installation Problem???`; after: all three pass.

**Acceptance**
- `build-native.sh --in-container` exits 0 and produces `bridges/xml/build/native/xml-bridge`.
- The three native tests above pass against that binary (exit 0) and fail against a binary built from main (exit 101).
- `grep -c 'org/apache/xerces/impl/msg/\*' bridges/xml/src/main/resources/application.yml` prints 1.

- [x] 1.1

## 2. Keep the cause chain in bridge error text (spec: wrapped cause is not lost)

**Files**
- `bridges/xml/src/main/java/org/rustcamel/xmlbridge/XsdValidatorService.java` (modified)
- `bridges/xml/src/test/java/org/rustcamel/xmlbridge/XsdValidatorServiceCauseTest.java` (new)
- `bridges/xml/src/test/java/org/rustcamel/xmlbridge/XsdValidationIntegrationTest.java` (modified)

**Steps**
1. Add `static String withCauses(Throwable t)` to `XsdValidatorService`: walk `getCause()` up to 8 levels, skip null/blank messages and messages already contained in the accumulated text, join with ` | `, return `t.getClass().getSimpleName()` if nothing was collected.
2. Use it in `refusalMessage` (non-refusal branch), in the `registerSchema` generic catch (`"registerSchema failed: "`), in `validateInternal` for `SAXException` (`SCHEMA_PARSE_ERROR: `) and for `Exception` (`validate failed: `).
3. Add the three unit tests and the integration test below.
4. Out of scope for this task: `validateAgainstSchema` (document validation) keeps `getMessage()`; tracked in rc-o2lx9. Tail truncation of long patterns in `bound()` is tracked in rc-2vvqr.

**Tests**
- `XsdValidatorServiceCauseTest.nullOuterMessageFallsBackToCauseMessage`: arrange `new RuntimeException(null, new IllegalStateException("real xerces message"))`; assert `withCauses` equals `real xerces message`.
- `XsdValidatorServiceCauseTest.distinctCauseMessagesAreJoinedAndDuplicatesDropped`: outer message contains the inner message, assert only the outer text; outer `compile failed` with inner `pattern '[x' is invalid`, assert `compile failed | pattern '[x' is invalid`.
- `XsdValidatorServiceCauseTest.messagelessChainReportsExceptionType`: `new RuntimeException()` gives `RuntimeException`.
- `XsdValidationIntegrationTest.invalidPatternKeepsXercesMessageAndPattern` (JVM): register a legacy schema with `xs:pattern value="[unclosed"`; assert kind COMPILATION_FAILED, detail contains `[unclosed`, `not a valid regular expression`, `Unexpected end of the pattern in a character class`, length at most 512.
- command: in the GraalVM builder image `quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25`, `docker run --rm --user=0:0 --volume=$WT/bridges/xml:/project:z --volume=$WT/bridges/versions.toml:/versions.toml:ro --workdir=/project --env=GRADLE_USER_HOME=/project/.gradle-docker-cache --env=HOME=/tmp --env=APP_HOME= --tmpfs=/tmp:rw,exec --entrypoint bash <image> -c "./gradlew --no-daemon spotlessCheck test"`. The `versions.toml` mount satisfies `rootDir.resolve("../versions.toml")`; a mount at another path makes Spotless fail with "All target files must be within the project dir".

**Acceptance**
- `spotlessCheck` and `test` exit 0 (77 tests, 0 failures at the time of writing).
- `grep -c withCauses bridges/xml/src/main/java/org/rustcamel/xmlbridge/XsdValidatorService.java` prints 5 (one definition, four uses).

- [x] 2.1

## 3. Native regression tests (spec: all native scenarios)

**Files**
- `crates/camel-test/tests/xml_bridge_xerces_diagnostics_test.rs` (new)
- `.github/workflows/ci.yml` (modified)

**Steps**
1. Add helper `pattern_schema(pattern) -> SchemaSet` producing a single-file schema with element `v` restricted by `xs:pattern`.
2. Add `invalid_pattern_surfaces_real_message` for `[unclosed`: assert `CompilationFailed` message contains `[unclosed`, `not a valid regular expression`, `Unexpected end of the pattern in a character class`, and not `Installation Problem`.
3. Add `valid_patterns_compile` for `\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})?` (good `2026-10-21T10:15:30Z`, bad `2026-10-21`) and `([0-9])+[.][0-9][0-9] [A-Z][A-Z][A-Z]` (good `12.50 EUR`, bad `12.5 EUR`).
4. Add `bundle_present` for `a{2,1}`: message contains `A min quantity must be <= a max quantity`, not `Installation Problem`, not `???`.
5. Append `--test xml_bridge_xerces_diagnostics_test` to the CI step that runs `cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test --test xml_bridge_xslt_test`.

**Tests**
- the three test functions above; command as in task 1; each ends by shutting the owned bridge child down.

**Acceptance**
- `cargo fmt --check --all` and `cargo clippy -p camel-test --features integration-tests --tests -- -D warnings` exit 0.
- The tests pass against the fixed native binary and fail against the main-built binary.
- `grep -c xml_bridge_xerces_diagnostics_test .github/workflows/ci.yml` prints 1 or more.

- [x] 3.1

## 4. Docs (mission R5)

**Files**
- `docs/src/components/validator.md` (modified)

**Steps**
1. Add a `### Schema diagnostics` subsection before `## Bridge lifecycle`: native bridge packages the Xerces bundles including the regex bundle; invalid `xs:pattern` reports pattern and Xerces message; older builds showed `Installation Problem???`; error text bounded to 512 characters. No bridge version numbers.

**Tests**
- `cargo xtask lint-bridge-versions` exits 0; `cd docs && mdbook test` exits 0.

**Acceptance**
- both commands exit 0.

- [x] 4.1
