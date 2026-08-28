## ADDED Requirements

### Requirement: Deterministic Xerces factory selection in the XML bridge

The XML bridge SHALL construct its schema and parser factories directly from the Xerces-J reference implementations on the classpath, without JAXP ServiceLoader discovery; SHALL include the Xerces message-bundle resources in the native image; and SHALL register the Xerces implementation classes that its internal `ObjectFactory` loads reflectively by name, so validation works and diagnostics are available in every build mode.

#### Scenario: schema registration and validation work in the native image

- **GIVEN** the XML bridge native binary with the Xerces reflective fallback targets registered
- **WHEN** a schema is registered and a document validated
- **THEN** the `ObjectFactory` lookups resolve and no `Provider ... not found` configuration error surfaces

#### Scenario: invalid document yields a schema diagnostic in the native image

- **GIVEN** the XML bridge native binary built with the GraalVM 25 toolchain
- **WHEN** a document that violates the configured XSD is validated
- **THEN** the returned error is of kind VALIDATION_FAILED carrying a schema diagnostic, and does not reference any resource-bundle load failure

#### Scenario: security hardening applies to the explicit factories

- **GIVEN** the bridge constructs Xerces-J factories directly
- **WHEN** a schema factory or a SAX parser factory is configured for validation work
- **THEN** the schema factory enables secure processing and denies external DTD and schema access, and the SAX source path denies external entities and DOCTYPE declarations and installs a deny-all entity resolver, identical to the previous discovery-based configuration

#### Scenario: deny-all behavior survives the factory switch

- **GIVEN** the bridge native binary with explicit Xerces-J factories
- **WHEN** a validated document carries a DOCTYPE declaration or entity reference, or the schema declares an external import
- **THEN** the external access is denied or ignored exactly as before the switch

#### Scenario: XSLT path keeps parser diagnostics in the native image

- **GIVEN** the bridge native binary
- **WHEN** a malformed XML document is transformed through the XSLT path
- **THEN** the reported error carries a readable parser diagnostic and does not reference any resource-bundle load failure
