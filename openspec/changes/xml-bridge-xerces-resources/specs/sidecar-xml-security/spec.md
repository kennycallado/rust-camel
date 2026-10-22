## MODIFIED Requirements

### Requirement: Deterministic Xerces factory selection in the XML bridge

The XML bridge SHALL construct its schema and parser factories directly from the Xerces-J reference implementations on the classpath, without JAXP ServiceLoader discovery; SHALL include every Xerces diagnostic message-bundle resource in the native image, including the regular-expression bundle that `xs:pattern` diagnostics resolve; SHALL keep the underlying Xerces message and the offending pattern in the bounded error text when schema compilation fails; and SHALL register the Xerces implementation classes that its internal `ObjectFactory` loads reflectively by name, so validation works and diagnostics are available in every build mode.

#### Scenario: schema registration and validation work in the native image

- **GIVEN** the XML bridge native binary with the Xerces reflective fallback targets registered
- **WHEN** a schema is registered and a document validated
- **THEN** the `ObjectFactory` lookups resolve and no `Provider ... not found` configuration error surfaces

#### Scenario: invalid document yields a schema diagnostic in the native image

- **GIVEN** the XML bridge native binary built with the GraalVM 25 toolchain
- **WHEN** a document that violates the configured XSD is validated
- **THEN** the returned error is of kind VALIDATION_FAILED carrying a schema diagnostic, and does not reference any resource-bundle load failure

#### Scenario: invalid pattern reports the real Xerces message

- **GIVEN** the XML bridge native binary
- **WHEN** a schema containing the invalid `xs:pattern` value `[unclosed` is registered
- **THEN** registration fails with kind COMPILATION_FAILED, the error message contains the pattern and the Xerces parse message, and does not contain `Installation Problem`

#### Scenario: valid pattern family compiles and validates

- **GIVEN** the XML bridge native binary
- **WHEN** schemas using the patterns `\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})?` and `([0-9])+[.][0-9][0-9] [A-Z][A-Z][A-Z]` are registered
- **THEN** both compile, a matching document validates, and a non-matching document is rejected

#### Scenario: regex bundle text is rendered in English

- **GIVEN** the XML bridge native binary
- **WHEN** a schema with the pattern `a{2,1}` is registered
- **THEN** the error message contains the English regex message `A min quantity must be <= a max quantity` and no unresolved-message marker

#### Scenario: wrapped cause is not lost

- **GIVEN** a schema-compilation exception (schema registration, or the inline schema compile of a validate call) whose own message is null, or does not contain the cause's message
- **WHEN** the bridge builds the compilation error message
- **THEN** the distinct cause messages are included, and the text stays within the diagnostic bound (very long patterns can still push the Xerces parse message past the bound)

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
