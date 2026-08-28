# Design: xml-bridge-xerces-messages

## Approach

Replace JAXP ServiceLoader discovery with direct construction of the Xerces-J reference implementations, and make their message bundles reachable inside the native image. Three coordinated edits; none alone is sufficient:

1. **Explicit factories.** `XsdValidatorService.getOrCompileSchema` constructs `new org.apache.xerces.jaxp.validation.XMLSchemaFactory()`. `secureSaxSource` (validator) and `XsltTransformerService` construct `new org.apache.xerces.jaxp.SAXParserFactoryImpl()`. Direct construction removes JAXP ServiceLoader discovery; it does NOT remove reflection entirely — see mechanism 3 below, discovered during native verification. The two existing control sets apply unchanged to the concrete factories:

   - `SchemaFactory` (schema compilation): `FEATURE_SECURE_PROCESSING` on, deny-all `ACCESS_EXTERNAL_DTD` and `ACCESS_EXTERNAL_SCHEMA` (JAXP 1.5 properties).
   - `SAXParserFactory`/`XMLReader` (document parsing in `secureSaxSource` and the XSLT source path): namespace-aware, `FEATURE_SECURE_PROCESSING` on, external general/parameter entities off, external DTD load off, `disallow-doctype-decl` on, entity-expansion limit 100, plus a deny-all `EntityResolver` returning an empty input source.

   `trySetProperty`/`trySetFeature` already tolerate unsupported properties on either control set.

2. **Resource inclusion.** Xerces-J resolves diagnostics from classpath resources under `org/apache/xerces/impl/msg/`. Quarkus merges two resource-inclusion sources: the `quarkus.native.resources.includes` list in `application.yml` (already carrying `application.yml` itself and the TLS placeholder pems) and classpath `META-INF/native-image/**/resource-config.json` files. Add `org/apache/xerces/impl/msg/*` to the existing `application.yml` includes list — one line in the single source of truth already proven in this bridge, instead of opening a second site in the empty `resource-config.json`. Xerces 2.12.2 keeps all message bundles (XMLMessages, XMLSchemaMessages, JAXPValidationMessages, including `_en` variants) as direct children of that directory, so a single-`*` glob suffices. Without inclusion, explicit factories would fail with the same `MissingResourceException` class of error, only from the other Xerces. The `additional-build-args` env-var-only constraint documented in `application.yml` concerns build args, not resources, and stays untouched.

3. **ObjectFactory reflection registrations.** Discovered during native verification (Task 2): explicit construction bypasses JAXP discovery, but Xerces' internal `ObjectFactory` still loads implementation classes reflectively by string name (`DTDDVFactoryImpl`, `XML11DTDDVFactoryImpl` for the XML 1.1 scanner path, `SchemaDVFactoryImpl`, `XMLSchemaLoader`, `XMLDTDLoader`, parser configurations, validators — 13 targets total). Without registration, every `register_schema` RPC fails natively with `ObjectFactory$ConfigurationError`. The fix registers all 12 via `@RegisterForReflection` in `NativeImageReflectionRegistrations.java` — the bridge's established reflection mechanism. Note: Quarkus ignores the application's own `META-INF/native-image/**/reflect-config.json`; an attempt to register there was a verified no-op and was reverted. This registration list is a load-bearing compatibility surface to re-check on the next GraalVM or Xerces bump. Deliberately unregistered: DOM `ObjectFactory` targets (the bridge is SAX-only) and XSD 1.1 `ExtendedSchemaDVFactoryImpl` (the JAXP `XMLSchemaFactory` is XSD 1.0) — unreachable on current paths.

Error-kind mapping (`SAXException` → `VALIDATION_FAILED`, generic `Exception` → `SECURITY_VIOLATION`) stays as is. With diagnostics resolvable again, the invalid-document path returns `VALIDATION_ERROR: <schema diagnostic>` as designed; the misclassification becomes unreachable for this input class.

## Affected crates

- `bridges/xml` (Java sources, `NativeImageReflectionRegistrations.java`, `application.yml` native resources list): the only production change.
- `crates/camel-test`: verification plus two new native-mode regression tests — malformed-input XSLT parser diagnostics (`xml_bridge_xslt_test.rs`) and DOCTYPE rejection pinning (`xml_bridge_validator_test.rs`).

## Architecture boundaries

The change is confined to the XML sidecar bridge internals behind its gRPC contract. No Rust crate, no bridge protocol, no data/control plane surface changes. The `sidecar-xml-security` requirements keep holding: the same fail-loud security configuration is applied to the explicit factories, so the security posture of the sidecar is invariant under this change.

Single-phase change (one coherent slice) — no `## Phases` section, no phase headings in tasks.md.

## Alternatives considered

- **Register the JDK-internal bundles in native-image.** Those resources live in the JDK runtime, not on the application classpath; `resource-config.json` patterns cannot reach them. Rejected as not implementable.
- **Remap the generic catch to `INTERNAL`.** Fixes the misclassification, not the message. The operator still gets garbage text. Treated symptom only; rejected as primary fix.
- **Relax the Rust test assertion.** Masks the product regression (degraded operator diagnostics). Rejected.
- **Set `javax.xml.validation.SchemaFactory:...` system properties.** Equivalent in spirit to direct construction but indirect; system properties are a second discovery mechanism that can be overridden by environment. Direct construction is the strongest guarantee.

## Verification

`cargo xtask build-xml-bridge` (Docker, GraalVM CE 25, musl-static) then `CAMEL_XML_BRIDGE_BINARY_PATH=bridges/xml/build/native/xml-bridge cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test --test xml_bridge_xslt_test`. JVM-mode coverage via the bridge's own Gradle test suite. CI repeats the same steps on ubuntu.
