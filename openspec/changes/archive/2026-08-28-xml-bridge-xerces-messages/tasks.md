# Tasks: xml-bridge-xerces-messages

## Task 1: Explicit Xerces factories and native-image resource inclusion

**Files:**

- `bridges/xml/src/main/java/org/rustcamel/xmlbridge/XsdValidatorService.java` (modified)
- `bridges/xml/src/main/java/org/rustcamel/xmlbridge/XsltTransformerService.java` (modified)
- `bridges/xml/src/test/java/org/rustcamel/xmlbridge/XsdValidationIntegrationTest.java` (modified)
- `bridges/xml/src/main/resources/application.yml` (modified)

**Steps:**

1. In `XsdValidatorService.java`, method `getOrCompileSchema` (~line 213): replace `SchemaFactory.newInstance(XMLConstants.W3C_XML_SCHEMA_NS_URI)` with `new org.apache.xerces.jaxp.validation.XMLSchemaFactory()`. Replace the comment claiming JAXP discovery finds the classpath Xerces with one stating direct construction is deterministic under native-image (GraalVM 25 discovery falls back to the JDK-internal Xerces, whose message bundles are absent from the image).
2. In `XsdValidatorService.java`, method `secureSaxSource` (~line 238): replace `SAXParserFactory.newInstance()` with `new org.apache.xerces.jaxp.SAXParserFactoryImpl()`. Update the adjacent comment the same way.
3. In `XsltTransformerService.java` (~line 210): replace `SAXParserFactory.newInstance()` with `new org.apache.xerces.jaxp.SAXParserFactoryImpl()` and update its adjacent comment.
4. Do not alter any hardening call: `FEATURE_SECURE_PROCESSING`, `ACCESS_EXTERNAL_DTD`/`ACCESS_EXTERNAL_SCHEMA` deny-all on the schema factory; namespace-aware, entity features off, `disallow-doctype-decl`, expansion limit 100, deny-all `EntityResolver` on the SAX path.
5. In `application.yml`, append `org/apache/xerces/impl/msg/*` to the existing `quarkus.native.resources.includes` list (keep the existing entries and the `additional-build-args` warning comment untouched).
6. Add test `externalSchemaImportDenied` to `XsdValidationIntegrationTest.java`, following the harness pattern of `validXmlWithMalformedXsdReturnsSchemaParseError`: build an XSD that declares `<xs:import namespace="http://127.0.0.1:9/external.xsd" schemaLocation="http://127.0.0.1:9/external.xsd"/>` and assert that compiling it fails with an error whose message references the denied external access (Xerces: "External Schema"/"external schema ... denied"), with no network fetch (port 9 = discard, nothing listens).
7. Run `cd bridges/xml && ./gradlew test --no-daemon` (JVM mode) — all tests must pass.

**Tests (existing plus new, JVM mode, run via step 7):**

- name: `externalSchemaImportDenied` (NEW)
  setup: JVM mode; an XSD whose `xs:import` points at `http://127.0.0.1:9/external.xsd`.
  action: validate any document against that schema.
  assert: schema compilation fails; the error references the denied external access; no fetch occurs.
  command: `cd bridges/xml && ./gradlew test --no-daemon`
  expected: FAIL before Task 1 only in the sense that the test is new; the denial behavior itself is pre-existing (`ACCESS_EXTERNAL_SCHEMA` deny-all) — the test pins it against the explicit factory. Owns the external-import half of the spec scenario "deny-all behavior survives the factory switch".
- name: `invalidXmlWithValidXsdReturnsValidationError`
  setup: Quarkus JVM test profile with the XSD validator service.
  action: validate a document that violates the XSD.
  assert: error kind VALIDATION_FAILED with a schema diagnostic message; no resource-bundle text.
  command: `cd bridges/xml && ./gradlew test --no-daemon`
  expected: pass before and after (JVM bundles resolve either way; guards against constructor-swap regressions).
- name: `xxeExternalEntityInjectionReturnsBridgeError`
  setup: JVM mode, `SecurityTest` suite.
  action: submit a document with a DOCTYPE declaring an external entity pointing at `file:///etc/passwd`.
  assert: rejected as a bridge security error; entity content never surfaces.
  command: `cd bridges/xml && ./gradlew test --no-daemon`
  expected: pass before and after — owns the spec scenario "deny-all behavior survives the factory switch" together with `billionLaughsPayloadFailsWithinTimeout`.
- name: `billionLaughsPayloadFailsWithinTimeout`
  setup: JVM mode, `SecurityTest` suite.
  action: submit an entity-expansion payload.
  assert: rejected within the timeout; expansion limit enforced.
  command: `cd bridges/xml && ./gradlew test --no-daemon`
  expected: pass before and after.

**Acceptance:**

- `cd bridges/xml && ./gradlew test --no-daemon` exits 0.
- `grep -cE "SchemaFactory\.newInstance|SAXParserFactory\.newInstance" bridges/xml/src/main/java/org/rustcamel/xmlbridge/XsdValidatorService.java bridges/xml/src/main/java/org/rustcamel/xmlbridge/XsltTransformerService.java` reports 0 for every file (the `OutputURIResolver.newInstance()` override at `XsltTransformerService.java:176` is a different, unrelated symbol and stays).
- `grep -n "org/apache/xerces/impl/msg" bridges/xml/src/main/resources/application.yml` matches exactly one line inside `quarkus.native.resources.includes`, with the pre-existing entries still present.

- [x] 1.1

## Task 2: Native-image verification and malformed-input XSLT diagnostic test

**Files:**

- `crates/camel-test/tests/xml_bridge_xslt_test.rs` (modified)
- `crates/camel-test/tests/xml_bridge_validator_test.rs` (modified)
- `bridges/xml/src/main/java/org/rustcamel/xmlbridge/NativeImageReflectionRegistrations.java` (modified — registration step discovered mid-task, see step 2)

**Steps:**

1. Build the native binary: `cargo xtask build-xml-bridge` (Docker, GraalVM CE 25, musl-static).
2. If validator RPCs fail with `ObjectFactory$ConfigurationError: Provider org.apache.xerces... not found`, Xerces' internal `ObjectFactory` fallback (reflective load by string name) is unregistered: add `@RegisterForReflection` entries for the 13 targets — `DTDDVFactoryImpl`, `XML11DTDDVFactoryImpl`, `SchemaDVFactoryImpl`, `XMLSchemaLoader`, `XMLDTDLoader`, `XIncludeAwareParserConfiguration`, `PSVIDocumentImpl`, `XMLSchemaValidator`, `XML11DTDValidator`, `XMLDTDValidator`, `XML11DTDProcessor`, `XIncludeParserConfiguration`, `XPointerParserConfiguration` — in `NativeImageReflectionRegistrations.java` (the bridge's established mechanism; the application's own `reflect-config.json` is ignored by Quarkus and must NOT be used), then rebuild.
3. Add test `xslt_malformed_input_document_returns_parser_error` to `xml_bridge_xslt_test.rs`, following the harness pattern of `xslt_compile_error_surfaces_on_transform`: register one route `direct:start -> <valid stylesheet> -> mock:result` using the existing `write_xslt()` helper, send a malformed XML body (a string that is not well-formed XML at all, e.g. `"<not-xml"`), and assert the send returns `Err`.
4. In that test, assert the error message does NOT contain `resource bundle` (the regression signature) and contains at least one of `Premature end of file`, `parse`, `PARSE` (Xerces `XMLMessages` output for a truncated document, asserted as a disjunction like the sibling compile-error test).
5. Add test `xsd_validation_doctype_rejected` to `xml_bridge_validator_test.rs`, following the harness pattern of `xsd_validation_invalid_document_returns_error`: same route shape and `write_xsd()` schema, but the body carries a DOCTYPE declaration with an external entity (`<!DOCTYPE order [<!ENTITY xxe SYSTEM "file:///etc/passwd">]>` referencing `&xxe;`). Assert the send returns `Err`, the message contains no `resource bundle` substring, and the rejection references the DOCTYPE policy (disjunction: `DOCTYPE`, `doctype`, `disallow`). Owns the native-mode DOCTYPE/entity half of the spec scenario "deny-all behavior survives the factory switch"; the external schema-import half is owned natively-infeasible-port-wise and covered in JVM mode by `externalSchemaImportDenied`.
6. Run both integration suites against the built binary:
   `CAMEL_XML_BRIDGE_BINARY_PATH=bridges/xml/build/native/xml-bridge cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test --test xml_bridge_xslt_test -- --test-threads=1`

**Tests (native mode, run via step 6):**

- name: `xsd_validation_invalid_document_returns_error`
  setup: native binary from step 1; the CI-red test from run 33111744850, job 98656038373 (pre-fix failure documented — reproducing the red build locally is optional, not required).
  action: validate `<order></order>` against the order XSD through a running route.
  assert: `Err` whose message contains `validation failed` / `XSD validation failed` / `VALIDATION_ERROR`; no `resource bundle` text.
  command: see step 6.
  expected: FAIL before the Task-1 change (documented in CI), PASS after.
- name: `xslt_malformed_input_document_returns_parser_error` (NEW)
  setup: native binary from step 1; valid stylesheet from `write_xslt()`.
  action: transform the malformed body `"<not-xml"`.
  assert: `Err`; message contains no `resource bundle` substring and contains at least one of `Premature end of file`, `parse`, `PARSE`.
  command: see step 6.
  expected: written in step 3, executed in step 6; PASS after Task 1.
- name: `xsd_validation_doctype_rejected` (NEW)
  setup: native binary from step 1; valid order XSD from `write_xsd()`.
  action: validate a document whose DOCTYPE declares an external entity and references it.
  assert: `Err`; message contains no `resource bundle` substring and matches at least one of `DOCTYPE`, `doctype`, `disallow`.
  command: see step 6.
  expected: written in step 5, executed in step 6; PASS after Task 1 (pre-existing hardening pinned natively).
- name: `xsd_validation_valid_document_passes`
  setup: native binary; valid document and XSD.
  action: validate.
  assert: exchange reaches the mock; no error.
  command: see step 6.
  expected: pass (regression guard).
- name: `xslt_transform_produces_expected_output`
  setup: native binary; valid document and stylesheet.
  action: transform.
  assert: transformed output at the mock matches the expected value.
  command: see step 6.
  expected: pass (regression guard; owns the shared SAX source path).
- name: `xslt_compile_error_surfaces_on_transform`
  setup: native binary; invalid stylesheet from `write_invalid_xslt()`.
  action: transform a valid document.
  assert: `Err` surfaces the stylesheet compile error.
  command: see step 6.
  expected: pass (regression guard).

**Acceptance:**

- Step 6 command exits 0 with all six tests passing.
- The combined test output contains no `resource bundle` substring: `CAMEL_XML_BRIDGE_BINARY_PATH=bridges/xml/build/native/xml-bridge cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test --test xml_bridge_xslt_test -- --test-threads=1 --nocapture 2>&1 | grep -c "resource bundle"` returns 0.

- [x] 2.1
