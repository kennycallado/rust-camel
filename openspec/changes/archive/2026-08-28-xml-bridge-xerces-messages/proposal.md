# Proposal: xml-bridge-xerces-messages

## Why

bd rc-7293. The GraalVM 25 toolchain move (f09f8ffb) landed with its CI run cancelled, so no verdict was seen. The first completed CI run after it (33111744850, commit 7a6c7471) fails `xsd_validation_invalid_document_returns_error` deterministically:

```
SECURITY_ERROR: Could not load any resource bundle by
com.sun.org.apache.xerces.internal.impl.msg.XMLSchemaMessages
```

Root cause: `XsdValidatorService` builds its validator through JAXP ServiceLoader discovery (`SchemaFactory.newInstance`). In the native image, discovery falls back to the JDK-internal Xerces instead of the explicit `xerces:xercesImpl:2.12.2` dependency the build declares for this purpose. GraalVM 25 no longer bundles that internal implementation's message resources, so formatting a validation error throws `MissingResourceException`. The generic catch classifies it as `SECURITY_VIOLATION`, and the bundle-failure text replaces the schema diagnostic.

The verdict is still correct (invalid documents are rejected); the defect is degraded error text. A CI re-run does not help — the failure is deterministic.

## What Changes

- Instantiate Xerces-J factories directly in the XML bridge: `org.apache.xerces.jaxp.validation.XMLSchemaFactory` (schema compilation) and `org.apache.xerces.jaxp.SAXParserFactoryImpl` (secure SAX sources in validator and XSLT paths).
- Include the Xerces message-bundle resources in the native image by appending `org/apache/xerces/impl/msg/*` to the existing `quarkus.native.resources.includes` list in `application.yml`.
- Register the 13 Xerces `ObjectFactory` reflective fallback targets (DTD/schema datatype factories incl. the XML 1.1 path, grammar loaders, parser configurations, validators) via `@RegisterForReflection` in `NativeImageReflectionRegistrations.java` — Xerces loads them by string name internally, and without registration every schema RPC fails in the native image.
- Update the stale code comment that claims discovery resolves to the classpath Xerces.

Excluded: CXF/JMS bridges (no evidence of the same fallback), the generic-Exception error-kind mapping (follow-up, not this change), Rust-side code, bridge protocol.

## Acceptance criteria

- `xsd_validation_invalid_document_returns_error` passes against the native binary built by `cargo xtask build-xml-bridge`.
- A malformed XML document fed to the XSLT path in the native image yields a readable parser diagnostic, not a resource-bundle failure.
- Valid documents still validate; XSLT integration tests still pass (shared SAX source path).
- Security posture unchanged across both control sets: `SchemaFactory` keeps `FEATURE_SECURE_PROCESSING` and deny-all `ACCESS_EXTERNAL_DTD`/`ACCESS_EXTERNAL_SCHEMA`; SAX sources keep entity/DOCTYPE denial plus the deny-all `EntityResolver`.
- JVM-mode Gradle tests still pass.

## Risk budget

Low. Same Xerces version already on the classpath; direct construction removes discovery ambiguity instead of adding machinery. Out of bounds: new dependencies, bridge protocol changes, GraalVM downgrade, relaxing the Rust-side test assertion.

Affected crates: none in Rust; `bridges/xml` (Java + native-image config) plus the `camel-test` integration suite as verifier.
