# Proposal: xml-bridge-xerces-resources

## Why

bd rc-taebo (camel-112). In the XML bridge native image, every `xs:pattern` diagnostic is replaced by `Installation Problem??? Couldn't load messages: Can't find bundle for base name org.apache.xerces.impl.xpath.regex.message`. A pattern that fails to compile is therefore undiagnosable.

Root cause (verified against a native binary): the Xerces regex message bundle `org/apache/xerces/impl/xpath/regex/message*.properties` is not packaged. The earlier change `xml-bridge-xerces-messages` packaged only `org/apache/xerces/impl/msg/*`. The Xerces jar has exactly two bundle locations; the regex one was missed.

Measured on an unmodified-main native build: valid and invalid patterns alike fail registration with `InvalidRegex ... Installation Problem???`. After packaging the bundle, valid patterns compile and invalid ones report the real parse message.

## What Changes

- Add `org/apache/xerces/impl/xpath/regex/message*.properties` to `quarkus.native.resources.includes` in `bridges/xml/src/main/resources/application.yml`. Remove a duplicated `org/apache/xerces/impl/msg/*` entry.
- Keep the Xerces message and the offending pattern in the bridge error text: add `XsdValidatorService.withCauses`, which appends distinct cause messages, and use it where a compile or validate exception is turned into a `BridgeError`. The existing 512-character bound still applies.
- Native regression tests `crates/camel-test/tests/xml_bridge_xerces_diagnostics_test.rs` (T1 invalid pattern, T2 valid pattern family, T3 bundle text rendered) and Java tests for the cause chain and for the JVM error text.
- `docs/src/components/validator.md`: note on schema diagnostics.

## Deviation from the mission order

The order asks for patterns in `META-INF/native-image/resource-config.json`. Two native builds (one with `glob` entries, one with `pattern` entries) showed that entries in that file do not reach the image: the resource count stayed at the baseline and the bundle stayed absent. `application.yml` is the working, single site. `resource-config.json` stays unchanged.

## Out of scope

The order-dependent pattern failure (R4): no TCCL, Locale or System-property changes. Version bump, tag and release stay with master.

## Acceptance criteria

- T1-T3 pass against the native binary built by the project Docker build (`build-native.sh` in the GraalVM CE builder image).
- Same tests fail against a native binary built from unmodified main.
- Java suite and `spotlessCheck` green.

## Risk budget

Low. Two config lines, one small pure helper, no protocol change. The binary grows by the size of four small property files.

Affected crates: none in Rust production code; `bridges/xml` (config + Java), `camel-test` (new test file), `docs`.
