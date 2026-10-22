# Tasks: cxf-soap-headers

Per-profile allow-list of SOAP header QNames exported to the route, plus the
reserved-name spoofing fix for `rc-e6ddw`. The design is fixed by the blessed
proposal/design/specs; these tasks implement it without widening the security surface.

## Verification guardrails

**Worktree.** All work happens in
`/home/shared/rust-camel-worktrees/375-cxf-soap-headers`. Never build in the main
checkout.

**Rust containment header — every Rust test command below uses this wrapper.** Run it
from the worktree root. Before the battery:

```bash
test "$(pgrep -c compiled_artifa || true)" -eq 0
```

Run the battery in a fresh user scope (replace only the final `cargo test` command
with the command named in the task):

```bash
UNIT="rc375-$(date +%s)"
TMPDIR=/home/shared/tmp systemd-run --user --scope --collect --unit="$UNIT" \
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill \
  -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 cargo test -p camel-component-cxf
systemctl --user stop "$UNIT" 2>/dev/null || true
```

After the battery:

```bash
test "$(pgrep -c compiled_artifa || true)" -eq 0
```

Never point `TMPDIR` at `/tmp`. `cargo check`, `fmt`, and `clippy` also run only in
this worktree with `TMPDIR=/home/shared/tmp`.

**Java toolchain.** Set the tool `workdir` to `bridges/cxf` for every Gradle command;
do not use a shell `cd` in a command snippet. This host has no `java` on `PATH`. Use
the JDK 21 store path:

```bash
JAVA_HOME=/nix/store/1arq9a57sqrn2z5ac362y2233dhcqgkc-openjdk-headless-21.0.12.1+1
PATH=$JAVA_HOME/bin:$PATH TMPDIR=/home/shared/tmp ./gradlew --no-daemon spotlessCheck test
```

If that store path is gone, discover one with
`nix shell nixpkgs#jdk21_headless -c ./gradlew --no-daemon spotlessCheck test` and
report the substitution. Every Java task ends with the full suite plus
`spotlessCheck`.

**Red protocol.** Write the task's tests first and run them. For a task that
introduces a new class or builder method, the red is the compile error naming the
absent symbol — run the test command, capture that output, then implement. For a
wiring task whose symbol already exists, snapshot the production file with
`sha256sum`, apply the minimal code-only mutation named in the task, observe the
expected failure, then reverse the exact edit and confirm the sha256 matches. Never
use `git checkout`/`git restore` to undo a mutation.

## Phase 1: Bridge exports allow-listed SOAP headers

### Proto

#### Task 1.1: Add the additive `SoapHeader` wire field to both proto copies

**Files:**
- `bridges/cxf/src/main/proto/cxf_bridge.proto` (modified)
- `crates/components/camel-cxf/proto/cxf_bridge.proto` (modified)
- `crates/components/camel-cxf/src/bridge_client_test.rs` (modified)
- `crates/components/camel-cxf/src/consumer.rs` (modified)
- `crates/components/camel-cxf/tests/consumer_unit_test.rs` (modified)
- `crates/components/camel-cxf/tests/consumer_signer_test.rs` (modified)
- `crates/components/camel-cxf/tests/consumer_profile_test.rs` (modified)
- `crates/components/camel-cxf/tests/consumer_emission_test.rs` (modified)

**Steps:**
1. In the first file, add this message immediately before `message ConsumerRequest`:

   ```proto
   // Inbound application header explicitly exported by the consumer profile's
   // allow-list. Never merged into `headers`: gRPC metadata keys are lowercased
   // and matched case-insensitively, so an HTTP key could spoof a SOAP value.
   message SoapHeader {
     string ns = 1;          // namespace URI (never a reserved WSS/SOAP namespace)
     string local_name = 2;  // element local name; unique per request
     string xml = 3;         // serialized element XML (xpath-friendly), bounded
     bool signed = 4;        // true only when a verified SIGN result covers this element
   }
   ```

2. Add `repeated SoapHeader soap_headers = 8;` as the last field of
   `message ConsumerRequest`, after `signer = 7`, with the comment
   `// allow-listed application headers; additive (old builds ignore it)`.
3. Copy the complete file to `crates/components/camel-cxf/proto/cxf_bridge.proto`
   byte-for-byte.
4. Before re-running the build, add `soap_headers: vec![],` to every literal
   `ConsumerRequest` struct literal in these files (prost structs are exhaustive):
   `src/bridge_client_test.rs` (one), `src/consumer.rs` (all `ConsumerRequest`
   literals in the `#[cfg(test)]` module), `tests/consumer_unit_test.rs` (all
   literals), `tests/consumer_signer_test.rs` (one helper), `tests/consumer_profile_test.rs`
   (all literals), `tests/consumer_emission_test.rs` (all literals). Do not add
   `..Default::default()`; list the field explicitly so the proto field is proven to
   exist.
5. Run the compile gate with `cargo check -p camel-component-cxf --tests`: the step-4
   literals fail while the proto field is absent (`struct ConsumerRequest has no field
   named 'soap_headers'`) and pass after the proto edits. `--tests` is mandatory;
   plain `cargo check` does not compile the edited test targets.

**Tests:** (executable spec)
- `proto_copies_are_identical`: run `diff bridges/cxf/src/main/proto/cxf_bridge.proto
  crates/components/camel-cxf/proto/cxf_bridge.proto` → exit 0, no output.
- `rust_proto_field_compiles`: add the literals first, run
  `TMPDIR=/home/shared/tmp cargo check -p camel-component-cxf --tests` in the scope
  wrapper → RED with `no field named 'soap_headers'` (the `--tests` flag is required:
  the literals live in `#[cfg(test)]` modules and integration tests); after the proto
  edits → exit 0. This compile is the executable proof that the generated struct
  carries the field.
- `java_proto_compiles`: run the Java command with `compileJava` →
  `BUILD SUCCESSFUL`; `cxf_bridge/SoapHeader.java` exists under
  `bridges/cxf/build/generated`.

**Acceptance:**
- Both proto files are byte-identical.
- `cargo check -p camel-component-cxf --tests` exits 0.
- Java `compileJava` exits 0.

- [x] 1.1

### Java bridge

#### Task 1.2: Add `SoapHeaderExporter` (extraction, signed flag, caps)

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapHeaderExporter.java` (new)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapHeaderExporterTest.java` (new)

**Steps:**
1. Create `final class SoapHeaderExporter` (package-private, no CDI) in
   `org.rustcamel.cxf` with:
   - constants `MAX_HEADERS = 16`, `MAX_HEADER_BYTES = 16 * 1024`,
     `MAX_TOTAL_BYTES = 64 * 1024`;
   - `static final Set<String> RESERVED_NAMESPACES` = WSSE 1.0
     (`http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd`),
     WSSE 1.1 (`http://docs.oasis-open.org/wss/oasis-wss-wssecurity-secext-1.1.xsd`),
     WSU (`http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd`),
     DS (`http://www.w3.org/2000/09/xmldsig#`),
     XENC (`http://www.w3.org/2001/04/xmlenc#`),
     SOAP 1.1 (`http://schemas.xmlsoap.org/soap/envelope/`),
     SOAP 1.2 (`http://www.w3.org/2003/05/soap-envelope`);
   - `static final class SoapHeaderExportException extends Exception` with a
     detail-only constructor (no header value in the message);
   - `static List<cxf_bridge.SoapHeader> export(Document doc, List<QName> allowList,
     List<WSSecurityEngineResult> results) throws Exception`. The broad clause is
     required because `SoapEnvelopeHelper.sourceToString` throws `Exception`; only
     duplicate-QName and size-cap violations throw `SoapHeaderExportException`, so
     serialization failures keep the generic Receiver-fault classification.
2. Implement `export` exactly:
   - empty/null allow-list → `List.of()`;
   - locate the direct `Header` child of the SOAP 1.1/1.2 `Envelope` root in the
     root's namespace (no `Header` → `List.of()`; never recurse into nested headers);
   - for each direct element child: resolve `{ns}local` (null namespace becomes `""`),
     skip it when not in `allowList`, skip it defensively when its namespace is in
     `RESERVED_NAMESPACES`;
   - throw `SoapHeaderExportException` when the same allow-listed QName appears twice
     (message names the QName only);
   - serialize with `SoapEnvelopeHelper.sourceToString(new DOMSource(element), true)`;
   - measure `xml.getBytes(StandardCharsets.UTF_8).length`; throw when a single value
     `> 16_384` or the running total `> 65_536` (message names the QName and the byte
     count; never the XML);
   - `signed = coveredBySignResult(element, results)`;
   - build `cxf_bridge.SoapHeader.newBuilder().setNs(namespaceUri).setLocalName(localName).setXml(xml).setSigned(signed).build()`.
3. Implement `private static boolean coveredBySignResult(Element element,
   List<WSSecurityEngineResult> results)`: return false for null/empty results; iterate
   only results whose `TAG_ACTION` equals `WSConstants.SIGN`; for each
   `TAG_DATA_REF_URIS` `WSDataRef`, return true only when
   `getProtectedElement() != null && getProtectedElement().isSameNode(element)`.
   A reference with no protected element returns false even when its `wsu:Id` matches;
   `ENCR` results are ignored. The contract fails closed on identifier-only evidence.
4. Add an immutable defensive copy when returning a non-empty list.

**Tests:** (executable spec — JUnit 5, package `org.rustcamel.cxf`)
- `exportsAllowListedChildWithNamespaceXmlAndOrder`: parse a SOAP 1.1 envelope with
  Header children `rh:requestHeaderType` (ns `http://tempuri.org/commonSchema`) then a
  second element → AQ: one `SoapHeader`, `getNs()`/`getLocalName()` match, `getXml()`
  equals `SoapEnvelopeHelper.sourceToString(new DOMSource(thatElement), true)`.
- `unlistedHeaderIsIgnored`: allow-list one QName, document carries it plus another →
  size 1, no exception.
- `missingHeaderReturnsEmpty`: Envelope with no Header → empty list.
- `duplicateQnameThrows`: two elements with the same allow-listed QName → throws
  `SoapHeaderExportException` whose message contains the QName and no XML.
- `reservedNamespaceIsSkippedDefensively`: raw allow-list `{WSSE_NS}Security` plus a
  `wsse:Security` Header child → empty list, no exception.
- `singleHeaderOverCapThrows`: one header whose serialized UTF-8 length is 16,385 →
  throws; message contains `16384` or `16,384` and the QName, not the content.
- `multibyteCharactersMeasureBytes`: a header with `"é"` repeated so the character
  count is below the cap but the UTF-8 byte length is above → throws.
- `aggregateOverCapThrows`: five allow-listed headers of 14 KiB each (each under the
  single cap, total 70 KiB) → throws.
- `exactLimitsAreAccepted`: build headers whose serialized lengths are exactly 16,384
  (single) and, in a separate document, exactly 65,536 total → no exception; adjust
  padding with a filler element/attribute until `getBytes(UTF_8).length` equals the
  target.
- `signedTrueOnlyForProtectedElement`: DOM header element; fake results
  `new WSSecurityEngineResult(WSConstants.SIGN, List.of(ref))` where
  `ref.setProtectedElement(element)` → `signed=true`; a second ref pointing at a
  sibling element → `signed=false`.
- `idOnlyReferenceDoesNotSetSigned`: fake SIGN result whose `WSDataRef` has
  `setWsuId("hdr-1")` and no protected element, while the header element carries
  `wsu:Id="hdr-1"` → `signed=false`.
- `signedFalseForEncryptedOnlyResult`: fake result
  `new WSSecurityEngineResult(WSConstants.ENCR, List.of(ref))` with the ref pointing
  at the header → `signed=false`.

**Red protocol:** `SoapHeaderExporterTest` fails to compile because the class is
absent. Capture that failure, then implement.

**Acceptance:**
- New tests pass; full Java suite and `spotlessCheck` pass.
- No new dependency in `build.gradle.kts`.
- `rg -n "getXml\(\)" SoapHeaderExporter.java` shows no logging of the value.

- [x] 1.2

#### Task 1.3: Store and validate the per-profile allow-list in `SecurityProfile`

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/BlankTlsValue.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileTest.java` (modified)

**Steps:**
1. Add `private final List<QName> exposedSoapHeaders;` to `SecurityProfile`; copy it in
   the private constructor from the builder; expose
   `public List<QName> exposedSoapHeaders()` returning the immutable list stored by the
   constructor (copy it once in the constructor so the accessor is cheap and immutable).
2. Add TWO builder fields to `Builder`:
   - `private List<String> exposedSoapHeadersRaw;` — the constructor input, already
     split into individual entries by the caller;
   - `private List<QName> exposedSoapHeaders = List.of();` — the validated result.
   Add `public Builder exposedSoapHeaders(List<String> entries)` that stores a
   defensive copy in `exposedSoapHeadersRaw` and returns `this`. `build()` calls
   `validateExposedSoapHeaders()` before `new SecurityProfile(this)`, and the
   `SecurityProfile` constructor assigns `this.exposedSoapHeaders =
   b.exposedSoapHeaders` (already immutable).
3. Implement `validateExposedSoapHeaders()` on the Builder:
   - null/empty raw → `exposedSoapHeaders = List.of()`, return;
   - more than 16 entries → `IllegalArgumentException` naming
     `CXF_PROFILE_<NAME>_EXPOSED_SOAP_HEADERS` and the 16 cap;
   - parse each entry by hand (no `\s` regex): it must start with `{`, contain a
     matching `}`, have a non-empty namespace and local name, and no second `{` or `}`
     in either part. Reject any namespace code point for which
     `BlankTlsValue.isWhiteSpaceCodePoint(cp)` is true (the exact Rust
     `char::is_whitespace` set), and any `,` in the namespace. The local name must
     match `[A-Za-z_][A-Za-z0-9._-]*` using an explicit ASCII char predicate;
   - change `BlankTlsValue.isWhiteSpaceCodePoint(int)` from `private` to
     package-private so both validators share one Unicode `White_Space` definition;
   - a namespace in `SoapHeaderExporter.RESERVED_NAMESPACES` is rejected naming the
     namespace as reserved;
   - exact duplicate QName and any pair whose local names are equal under
     `equalsIgnoreCase` are rejected (message explains the exchange key is
     `CxfSoapHeader.<localName>` and lookup is case-insensitive);
   - assign the immutable `List<QName>` in configuration order.
4. `SecurityProfileStore` parses the env var in Task 1.4; do not touch it here.

**Tests:** (JUnit 5, `SecurityProfileTest`)
- `exposedSoapHeaders_parsesThePartnersQnames`: builder with both partner QNames →
  accessor returns two `QName`s in order with the exact namespaces/local names.
- `exposedSoapHeaders_absentIsEmpty`: default builder → empty list.
- `exposedSoapHeaders_rejectsEachReservedNamespace`: parameterized over the seven
  reserved namespaces with local `Security` → each `build()` throws
  `IllegalArgumentException` whose message contains `reserved`.
- `exposedSoapHeaders_rejectsMalformedEntries`: parameterized over
  `requestHeaderType` (no namespace), `{http://x/}`, `{http://x}bad local`,
  `{http://x,com}requestHeaderType`, `{http://x}a{b}`, a namespace containing U+2003
  and one containing U+00A0 → throws naming the entry.
- `exposedSoapHeaders_rejectsMoreThanSixteen`: 17 distinct QNames → throws naming the
  cap.
- `exposedSoapHeaders_rejectsDuplicateQname`: same entry twice → throws.
- `exposedSoapHeaders_rejectsCaseInsensitiveLocalCollision`:
  `{http://a}Tenant` + `{http://b}tenant` → throws.

**Red protocol:** the new builder method and accessor do not exist; the tests fail
to compile naming them. Capture that, then implement.

**Acceptance:**
- All new tests pass; full Java suite and `spotlessCheck` pass.

- [x] 1.3

#### Task 1.4: Parse the env var, extract in the WSS processor, and dispatch the field

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfileStore.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/WssSecurityProcessor.java` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileStoreTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/WssSecurityProcessorTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherTest.java` (modified)

**Steps:**
1. `SecurityProfileStore`: add
   `.exposedSoapHeaders(parseExposedSoapHeaders(getenv(prefix + "EXPOSED_SOAP_HEADERS")))`
   to the builder chain. The helper returns `List<String>`: `null` for a null/blank
   value, otherwise the entries split on `,` and trimmed; a blank segment raises an
   `IllegalStateException` naming the env var. It never parses QNames; the builder
   (`SecurityProfile.Builder.exposedSoapHeaders(List<String>)`) is the single
   validation point.
2. `WssSecurityProcessor`: change the record to
   `VerifiedInboundMessage(String xml, Optional<SignerIdentity> signer,
   List<cxf_bridge.SoapHeader> soapHeaders)`; update both construction sites (the
   early return uses `List.of()`); after signer extraction and before `domToString`,
   call
   `SoapHeaderExporter.export(doc, profile.exposedSoapHeaders(), results)`.
   Declare `throws Exception` unchanged. The exporter exception is not a
   `WSSecurityException`; do not wrap it.
3. `SoapEndpointPublisher.handleRequestBody`:
   - declare `List<cxf_bridge.SoapHeader> soapHeaders;` next to `securedXml`/`signer`.
     In the existing `if (wssProcessor.canVerifyInbound())` branch, after
     `signer = verified.signer();` add `soapHeaders = verified.soapHeaders();`;
   - add an `else` arm that exports from the pre-WSS `requestDoc`:
     `soapHeaders = SoapHeaderExporter.export(requestDoc,
     profile.exposedSoapHeaders(), null);` — this is design R1's "profiles without
     WSS extract from requestDoc" path, and every header is `signed = false`;
   - add `.addAllSoapHeaders(soapHeaders)` to the `ConsumerRequest.Builder`;
   - in the failure classifier, compute
     `boolean isInvalidSoapHeaders = hasInChain(cause,
     SoapHeaderExporter.SoapHeaderExportException.class);` and add a branch before the
     `isSecurityFailure` branch: `faultClass = FaultClass.SENDER`,
     `faultCode = "soap:Client"`, `faultString = "invalid SOAP header export: " +
     sanitize(cause.getMessage())`. Do not set `isWssFault`, so `fault_status` never
     applies.

**Tests:** (JUnit 5; reuse `driveRequestOutcome`, `envelope11`, `assertSenderFault`)
- `SecurityProfileStoreTest`
  - `exposedSoapHeaders_envVarReachesTheProfile`: env
    `CXF_PROFILE_P1_EXPOSED_SOAP_HEADERS={http://tempuri.org/commonSchema}requestHeaderType`
    → profile accessor has that QName.
  - `exposedSoapHeaders_blankVarIsEmpty`: blank value → accessor empty.
  - `exposedSoapHeaders_reservedNamespaceFailsStartup`: WSSE QName → constructor throws.
- `WssSecurityProcessorTest`
  - `processInbound_exportsAllowListedHeader`: signed SOAP 1.1 envelope with the app
    header next to the inserted `wsse:Security` → `soapHeaders()` size 1, namespace and
    local name correct, `signed=false` (the body-only signature does not cover it).
  - `processInbound_neverExportsSecurity`: same envelope with a raw allow-list that a
    profile cannot build (call the exporter directly) → no `wsse:Security` entry.
- `SoapEndpointPublisherTest` (T1, T3, T4, T5, T7, T10)
  - T1 `allow_listed_header_is_exported`: profile with
    `.exposedSoapHeaders(List.of("{http://tempuri.org/commonSchema}requestHeaderType"))`,
    a signing profile (keystore+truststore+actionsIn Signature), a signed envelope that
    carries `rh:requestHeaderType` → `ConsumerRequest.getSoapHeadersCount() == 1`,
    ns/local/XML correct.
  - T3 `unlisted_header_is_ignored`: two app headers, allow-list one → count 1 and the
    route is invoked.
  - T4 `duplicate_qname_faults`: two identical allow-listed elements → Sender fault
    (SOAP 1.1 status 500), `verify(cxfServerManager, never())`.
  - T5 `size_caps_fault`: one >16,384-byte header, and a 70 KiB aggregate → each a
    Sender fault, never dispatched; assert the fault text carries no header content.
  - T5b `cap_fault_ignores_fault_status_override`: profile with `faultStatus(403)` and
    an oversized header → status 500 (SOAP 1.1), not 403.
  - T5c `exact_limits_reach_the_route`: build one request whose single exported header
    is exactly 16,384 UTF-8 bytes and another whose exported total is exactly 65,536
    bytes (each header below the single cap) → both dispatch, captured
    `ConsumerRequest.getSoapHeadersCount()` matches, and no fault is rendered.
  - T7 `signed_flag_false_without_wss`: no-security profile with the allow-list → one
    header with `signed=false`, route invoked.
  - T10 `several_qnames_per_profile`: profile with both partner QNames and a request
    carrying both → count 2; then a second profile allowing only the first, same
    request → count 1 (assert via a second `driveRequestOutcome` and captured requests;
    no cross-profile leakage).

**Red protocol (wiring mutation).** After the tests are written and compile against the
new seams, snapshot `SoapEndpointPublisher.java`; replace
`.addAllSoapHeaders(soapHeaders)` with `.addAllSoapHeaders(List.of())`; run the new
publisher tests and observe T1/T3/T7/T10 fail on an empty header list; restore the
snapshot and confirm the sha256. Then snapshot `SoapHeaderExporter.java`; make
`coveredBySignResult` return `false` unconditionally; observe T6 (Task 1.5) fail;
restore. Never commit a mutation.

**Acceptance:**
- T1, T3, T4, T5, T5b, T5c, T7, T10 pass; full Java suite and `spotlessCheck` pass.
- No log line or fault message contains a header value (`rg` the changed files for
  `getXml()` usage).

- [x] 1.4

#### Task 1.5: Sign a header-bearing envelope, prove `signed=true`, and generate the Rust fixture

**Files:**
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SignedSoapHeaderEnvelopeFactory.java` (new)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SignedRequestFixtureGeneratorTest.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherTest.java` (modified)
- `crates/camel-test/tests/fixtures/cxf-wss-signed-request-with-header.xml` (new, generated)

**Steps:**
1. Create `SignedSoapHeaderEnvelopeFactory` with
   `static String signSoap11(String soapXml, Path keystore, String alias,
   String signatureAlgorithm, String digestAlgorithm, String headerXml,
   boolean signHeader)`: parse the envelope, insert `headerXml` as the first child of
   the SOAP 1.1 `Header` (creating it when absent) with `wsu:Id="hdr-1"`, then build
   `WSSecSignature` exactly like `ExpiredSignedEnvelopeFactory` (BST direct reference,
   Body Content part) and, when `signHeader` is true, add
   `new WSEncryptionPart("hdr-1", "Element")` to `sign.getParts()`; serialize with
   `SoapEnvelopeHelper.sourceToString`.
2. Add a skipped-by-default generator test
   `generateWithHeader()` to `SignedRequestFixtureGeneratorTest`: when
   `CXF_SIGNED_FIXTURE_JKS` and `CXF_SIGNED_FIXTURE_WITH_HEADER_OUT` are set, sign a
   SOAP 1.2 `sayHello` envelope that carries
   `<rh:requestHeaderType xmlns:rh="http://tempuri.org/commonSchema"><rh:user>alice</rh:user><rh:messageId>m-1</rh:messageId><rh:organisation>28071</rh:organisation></rh:requestHeaderType>`
   with `signHeader=false`, verify it with `WssSecurityProcessor.processInbound` against
   the same JKS, and write it to the output path. Use SHA-256 signature/digest like the
   existing generator. `Assumptions.assumeTrue` when the env vars are absent.
3. Add T6 to `SoapEndpointPublisherTest` and define
   `private static final String HEADER_XML = "<rh:requestHeaderType
   xmlns:rh=\"http://tempuri.org/commonSchema\"><rh:organisation>28071</rh:organisation></rh:requestHeaderType>";`
   in that class. `signed_flag_true_for_covered_header` — build a signing consumer
   profile, sign with
   `SignedSoapHeaderEnvelopeFactory.signSoap11(envelope11(ECHO_ELEMENT), keystorePath,
   TestKeystoreHelper.KEY_ALIAS, "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256",
   "http://www.w3.org/2001/04/xmlenc#sha256", HEADER_XML, true)` using the checked-in
   test keystore, drive the request → the dispatched header has `getSigned() == true`.
4. Run the Java suite, then generate the Rust fixture:

   ```bash
   CXF_SIGNED_FIXTURE_JKS=/home/shared/rust-camel-worktrees/375-cxf-soap-headers/crates/camel-test/tests/fixtures/cxf-wss-test-keystore.jks \
   CXF_SIGNED_FIXTURE_WITH_HEADER_OUT=/home/shared/rust-camel-worktrees/375-cxf-soap-headers/crates/camel-test/tests/fixtures/cxf-wss-signed-request-with-header.xml \
   ./gradlew --no-daemon test --tests 'org.rustcamel.cxf.SignedRequestFixtureGeneratorTest'
   ```

   Confirm the file exists, contains `requestHeaderType`, and that the generator test
   reports the fixture was written.

**Tests:**
- `signedHeaderCoverage_setsSignedTrue` (factory-level or publisher-level): covered
  header → `signed=true`.
- `signedHeaderCoverage_bodyOnlySignatureLeavesHeaderUnsigned`: `signHeader=false` →
  `signed=false` (this is T6's negative control and the fixture's shape).
- `generatedHeaderFixtureVerifiesInbound` (inside the generator, run only when env
  vars are set): `processInbound` accepts the fixture and exports one header with
  `signed=false`.

**Acceptance:**
- T6 passes; full Java suite and `spotlessCheck` pass.
- The fixture XML is committed and has no `wsu:Timestamp`.
- No bridge version string is introduced anywhere.

- [x] 1.5

## Phase 2: Rust exchange mapping, spoofing fix, e2e, docs

### camel-component-cxf / camel-bridge

#### Task 2.1: Add `exposed_soap_headers` config, validation, and env emission

**Files:**
- `crates/components/camel-cxf/src/config.rs` (modified)
- `crates/components/camel-cxf/src/pool.rs` (modified)
- `crates/components/camel-cxf/src/pool_env_test.rs` (modified)
- `crates/services/camel-bridge/src/process.rs` (modified)
- `crates/camel-test/tests/cxf_test.rs` (modified)

**Literal migration:** `CxfProfileEnvVars` is an exhaustive Rust struct. Add
`exposed_soap_headers: vec![],` to every literal in
`crates/services/camel-bridge/src/process.rs` (all `CxfProfileEnvVars` struct literal in the
test module), `crates/components/camel-cxf/src/pool_env_test.rs`, and
`crates/camel-test/tests/cxf_test.rs`. Do not use `..Default::default()` in these
literals.

**Steps:**
1. `CxfSecurityFields`: add
   `#[serde(default)] pub exposed_soap_headers: Vec<String>,` documented as the
   per-profile allow-list of `{namespace}local` QNames (max 16; values are names, not
   PII). Add the field to the manual `Debug` impl.
2. Add a `CxfSecurityFields::validate_exposed_soap_headers(&self, profile: &str) ->
   Result<(), CamelError>` that enforces: at most 16 entries; each entry parses as
   `{ns}local` with `ns` non-empty and free of `{`, `}`, `,`, and `char::is_whitespace`,
   and `local` matching `[A-Za-z_][A-Za-z0-9._-]*` (ASCII only); no reserved namespace
   (WSSE 1.0/1.1, WSU, DS, XENC, SOAP 1.1/1.2); no duplicate QName; no two local names
   equal under `eq_ignore_ascii_case`. Every error names `security.exposed_soap_headers`
   and the profile. Call it from `CxfPoolConfig::validate` for every profile.
3. `CxfProfileEnvVars` (`camel-bridge/src/process.rs`): add
   `pub exposed_soap_headers: Vec<String>` and emit
   `CXF_PROFILE_<NAME>_EXPOSED_SOAP_HEADERS` as the comma-joined list only when
   non-empty.
4. `pool.rs::profile_env_vars_for`: map
   `exposed_soap_headers: p.security.exposed_soap_headers.clone()`.

**Tests:**
- `config.rs` unit tests:
  - `exposed_soap_headers_parse_and_default`: YAML with two partner QNames in a list →
    `validate()` is Ok and the fields round-trip; omitted → empty vec.
  - `exposed_soap_headers_reject_reserved_namespace`: parameterized over the seven
    namespaces → `validate()` is Err whose text names
    `security.exposed_soap_headers` and `reserved`.
  - `exposed_soap_headers_reject_malformed_and_cap`: entries without namespace, with a
    space, with a comma in the namespace, with a non-ASCII local, and 17 entries →
    each Err naming the cap or the entry.
  - `exposed_soap_headers_reject_duplicate_and_case_collision`: duplicate QName and
    `Tenant`/`tenant` → Err.
- `process.rs` test `cxf_profile_env_var_emits_exposed_soap_headers`:
  `to_env_vars()` contains the joined value and omits the key when empty.
- `pool_env_test`: the env-key trace of a profile with the field contains
  `CXF_PROFILE_<NAME>_EXPOSED_SOAP_HEADERS`; no value is logged.

**Red protocol:** write the config tests first. Adding
`security.exposed_soap_headers` to YAML is silently ignored today (the struct has no
such field), so `validate()` returns Ok and the test fails on an empty
`exposed_soap_headers`; after the field and validator exist the test passes. The
`CxfProfileEnvVars` literal updates fail to compile until the field is added.

**Acceptance:**
- `cargo test -p camel-component-cxf -p camel-bridge` green in the scope wrapper.
- `cargo clippy -p camel-component-cxf -p camel-bridge --all-targets -- -D warnings`
  exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 2.1

#### Task 2.2: Map SOAP headers into the exchange and drop spoofed Cxf* HTTP keys

**Files:**
- `crates/components/camel-cxf/src/consumer.rs` (modified)

**Steps:**
1. Add
   `const RESERVED_CXF_HTTP_HEADERS: [&str; 4] = ["cxfrequestid", "cxfoperation",
   "cxfsoapaction", "cxfsecurityprofile"];` near `CXF_PROFILE_METADATA_KEY`.
2. In `build_exchange`, replace the HTTP-header loop body: compute
   `let lower = k.to_ascii_lowercase();` and `continue` when
   `lower.starts_with("cxfsoapheader")` or
   `RESERVED_CXF_HTTP_HEADERS.contains(&lower.as_str())`; otherwise insert the header
   exactly as today. This loop stays before the SOAP mapping so bridge-owned values
   cannot be overwritten.
3. After the HTTP loop, add the SOAP mapping:
   for each `h` in `&req.soap_headers`, skip entries with an empty `local_name`; set
   `CxfSoapHeader.<local_name>` = `h.xml` and `CxfSoapHeaderSigned.<local_name>` =
   `"true"`/`"false"` as `Value::String` on `exchange.input`. Do not log the values.
4. Values come only from the dedicated proto field; `req.headers` is never consulted
   for SOAP values.

**Tests:** (unit tests in `consumer.rs`, extend the existing module)
- T8 `http_spoof_cannot_shadow_soap_or_cxf_headers`: `ConsumerRequest` with
  `operation="bridge-op"`, `security_profile="bridge-profile"`, one `SoapHeader`
  `{ns: "http://tempuri.org/commonSchema", local_name: "requestHeaderType", xml:
  "<rh:requestHeaderType xmlns:rh=\"http://tempuri.org/commonSchema\"><rh:organisation>28071</rh:organisation></rh:requestHeaderType>",
  signed: true}`, and HTTP headers
  `cxfsoapheader.organisation=attacker`, `cxfsoapheader.requestheadertype=spoofed`,
  `cxfoperation=spoofed`, `cxfsecurityprofile=spoofed`, `cxfrequestid=spoofed`,
  `cxfsoapaction=spoofed`, plus mixed-case variants `CxfOperation` and
  `CxfSecurityProfile` → look up every reserved name with `header_ic` (the
  case-insensitive accessor) and assert:
  `CxfOperation == "bridge-op"`, `CxfSecurityProfile == "bridge-profile"`,
  `CxfRequestId`/`CxfSoapAction` keep the bridge values; no
  `cxfsoapheader.organisation` header exists; and `CxfSoapHeader.requestHeaderType`
  holds the proto XML with `CxfSoapHeaderSigned.requestHeaderType == "true"`.
- T8b `http_cannot_inject_soap_header_without_proto_field`: same request but
  `soap_headers: vec![]` and HTTP `cxfsoapheader.requestheadertype=spoofed` → no
  `CxfSoapHeader.requestHeaderType` and no `CxfSoapHeaderSigned.requestHeaderType` key
  exists (only the dedicated field can populate them).
- `soap_headers_map_to_exchange_with_flag`: two headers, `signed` true and false →
  both `CxfSoapHeader.*` and `CxfSoapHeaderSigned.*` set to the expected strings.
- `empty_soap_header_local_name_is_skipped`: an entry with `local_name=""` → no
  `CxfSoapHeader.` key is created.
- `ordinary_http_header_still_maps`: `CustomHeader` is preserved (regression guard).

**Red protocol (security mutation).** Snapshot `consumer.rs`; delete the two `continue`
conditions in the HTTP loop; run T8 and observe it fail (spoofed values overwrite the
bridge-owned ones); restore the snapshot and confirm the sha256.

**Acceptance:**
- New tests pass; `cargo test -p camel-component-cxf` green in the scope wrapper.
- `cargo clippy -p camel-component-cxf --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 2.2

### camel-test

#### Task 2.3: Add the signed application-header e2e leg (T9)

**Files:**
- `crates/camel-test/tests/cxf_test.rs` (modified)
- `crates/camel-test/tests/cxf_test/wss_e2e.rs` (modified)

**Steps:**
1. In `cxf_test.rs`, add
   `fn cxf_wss_signed_request_with_header_fixture() -> PathBuf` returning
   `tests/fixtures/cxf-wss-signed-request-with-header.xml` (mirror the existing
   `cxf_wss_signed_request_fixture` helper).
2. In `wss_e2e.rs`, add
   `#[tokio::test(flavor = "multi_thread")] async fn cxf_native_wss_exports_allow_listed_soap_header()`:
   - `init_tracing(); install_crypto_provider(); let _binary = require_cxf_bridge_binary();`
   - start a WSS consumer with `CxfSecurityFields { exposed_soap_headers:
     vec!["{http://tempuri.org/commonSchema}requestHeaderType".to_string()], ..
     wss_security(None) }` (use `start_wss_consumer` with a distinct profile name);
   - the route sets the `sayHelloResponse` body and sends to `mock:consumed` (copy the
     LEG B route);
   - `post_soap(&base, "text/xml", "sayHello", fixture_xml)` where `fixture_xml` is
     read from `cxf_wss_signed_request_with_header_fixture()`;
   - assert HTTP 200, wait for the mock hit, then assert on the received exchange:
     `CxfSoapHeader.requestHeaderType` contains `organisation` and `28071`,
     `CxfSoapHeaderSigned.requestHeaderType == "false"`, and
     `principal_from_exchange` is still `None` (the app header is data, not auth).
3. Keep the test in the existing `#[path]` module layout so `--test cxf_test` filters
   work.

**Tests:**
- `cxf_native_wss_exports_allow_listed_soap_header`: as above. Execution requires the
  native bridge binary via `CAMEL_CXF_BRIDGE_BINARY_PATH`/`CAMEL_CXF_BRIDGE_BINARY`.
  When the binary is absent, run only the compile gate:
  `cargo test -p camel-test --features integration-tests-cxf --test cxf_test --no-run`
  → exit 0 (the `cxf_test` target is gated by
  `#![cfg(any(feature = "integration-tests", feature = "integration-tests-cxf"))]`, so
  the feature is mandatory), and record "integration-verification-deferred-to-CI" in
  the task result.

**Acceptance:**
- `cargo test -p camel-test --features integration-tests-cxf --test cxf_test --no-run`
  exits 0 in the scope wrapper.
- If the native binary is available,
  `cargo test -p camel-test --features integration-tests-cxf --test cxf_test
  cxf_native_wss_exports_allow_listed_soap_header` passes. Otherwise the deferral is
  explicit and the fixture is committed.
- `cargo clippy -p camel-test --features integration-tests-cxf --all-targets -- -D
  warnings` exits 0.

- [x] 2.3

### Docs

#### Task 2.4: Document the allow-list, exchange headers, caps, and trust limit

**Files:**
- `docs/src/components/ws-soap.md` (modified)

**Steps:**
1. Add one row to the per-profile security table (near `allowed_digest_algorithms` and
   `allow_expired_pinned_certificate`):
   `exposed_soap_headers` | `CXF_PROFILE_<NAME>_EXPOSED_SOAP_HEADERS` |
   `cxf.security.exposed.soap.headers` | _(none)_.
   The third value is the documented property-style mapping name used by the adjacent
   rows, not a new runtime configuration surface.
2. Add a section `### Exposing application SOAP headers` that states:
   - the allow-list is a list of QNames in `{namespace}local` form, at most 16, with
     the partner example;
   - the validation rules: reserved WSSE/WSSE11/WSU/DS/XENC/SOAP namespaces rejected,
     `wsse:Security` never exported, ASCII local-name grammar, no duplicate QNames and
     no local names that differ only by case;
   - exchange header names `CxfSoapHeader.<localName>` and
     `CxfSoapHeaderSigned.<localName>`; the value is serialized XML suitable for
     XPath; unlisted headers are ignored;
   - caps: 16,384 UTF-8 bytes per header and 65,536 bytes total are Sender faults
     before dispatch;
   - trust: `signed=true` only when the verified signature covers the element;
     `signed=false` headers MUST NOT drive authorization unless TLS covers the hop;
   - a YAML example under `[components.cxf.profiles.security]` with both partner
     QNames.
3. Do not add any bridge version number or semver anywhere in the file.
4. Run `cargo xtask schema` from the worktree root with `TMPDIR=/home/shared/tmp`,
   then `git status --short schemas`. If nothing changes, record the evidence:
   `rg -n "CxfSecurityFields|exposed_soap_headers" schemas/` returns no hits (no
   generated schema covers this config surface). Do not hand-edit a schema.

**Tests:** (non-Rust code; executable gates)
- `bridge_version_lint`: `cargo xtask lint-bridge-versions` → exit 0.
- `mdbook_test`: `cd docs && mdbook test` → exit 0.
- `schema_check`: `cargo xtask schema --check` → exit 0 after regeneration.
- `doc_links`: `rg -n "CxfSoapHeader" docs/src/components/ws-soap.md` → both header
  families documented.

**Acceptance:**
- All four gates exit 0.
- The docs describe capability only (no version number).
- Schema drift evidence is recorded in the task result.

- [x] 2.4

## Final verification

After all tasks: `cargo build --workspace`, `cargo fmt --check --all`, affected-crate
clippy, the `camel-component-cxf` / `camel-bridge` test batteries, `feature_profiles`,
the xtask lints, `schema --check`, `mdbook test`, and `openspec validate` — all under
the containment rules above, with every exit code recorded in the final report. The
holistic review runs on the complete diff.
