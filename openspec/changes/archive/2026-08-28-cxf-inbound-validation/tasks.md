# Tasks: cxf-inbound-validation

## Task 1: validateInboundActions + property tests

- **Files**:
  - `bridges/cxf/src/main/java/org/rustcamel/cxf/SecurityProfile.java` (modified)
  - `bridges/cxf/src/test/java/org/rustcamel/cxf/SecurityProfileTest.java` (modified)
  - `bridges/cxf/src/test/java/org/rustcamel/cxf/WssSecurityProcessorIntegrationTest.java` (modified — consequential arrange fix: consumerRefusesPartsProfile gains truststore)

- **Command** (run from repo root, `BRIDGE=.worktrees/cxf-inbound-validation/bridges/cxf`):
  `docker run --rm --user=0:0 --volume="$PWD/$BRIDGE":/project:z --workdir=/project --env=GRADLE_USER_HOME=/project/.gradle-docker-cache --tmpfs=/tmp:rw,exec --entrypoint bash quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25 -c 'HOST_UID=$(stat -c "%u" /project); java -cp gradle/wrapper/gradle-wrapper.jar org.gradle.wrapper.GradleWrapperMain spotlessCheck test --no-daemon; ST=${PIPESTATUS[0]}; chown -R ${HOST_UID}:${HOST_UID} /project 2>/dev/null; exit $ST'`
  Expected (final): exit 0, 142 tests, 0 failures.
- **Steps**:
  1. In `Builder`, add `validateInboundActions()` mirroring
     `validateOutboundActions()` (SecurityProfile.java:494-516): return when
     `!hasText(securityActionsIn)`; throw `IllegalArgumentException` when
     `containsAction(raw, "Signature") && !hasText(truststorePath)` —
     message names `cxf.truststore.path`, states the manual-consumer-only
     keystore fallback, and that it may point at the same JKS; throw when
     `containsAction(raw, "Encrypt") && !hasText(keystorePath)` — message
     names `cxf.keystore.path`.
  2. Call it from `build()` immediately before `validateOutboundActions()`.
  3. Extend the `validateOutboundActions` javadoc note (or add an
     equivalent one) so both validators read as one discipline.
- **Tests** (name / arrange / act / assert), all in
  `SecurityProfileTest`, patterned on the existing out-actions validation
  tests:
  - `actionsInSignatureWithoutTruststoreIsRejected` / builder with
    keystore + `actionsIn("Signature")`, no truststore / `build()` /
    `IllegalArgumentException`, message contains all of `cxf.truststore.path`,
    `manual consumer`, and `same JKS` (exact contract wording, not just
    keyword `manual`).
  - `actionsInEncryptWithoutKeystoreIsRejected` / builder with truststore +
    `actionsIn("Encrypt")`, no keystore / `build()` /
    `IllegalArgumentException`, message contains `cxf.keystore.path`.
  - `blankActionsInIsExempt` / builder with no security material,
    `actionsIn` never set / `build()` / succeeds; `createInInterceptor()`
    returns `null`.
  - `whitespaceActionsInIsExempt` / same but `actionsIn("   ")` / `build()`
    / succeeds; `createInInterceptor()` returns `null`.
  - `actionsInSignatureOnlyWithTruststoreBuilds` / builder with truststore
    only + `actionsIn("Signature")` / `build()` then
    `createInInterceptor()` / succeeds; assert
    `assertInstanceOf(WSS4JInInterceptor.class, interceptor)` and
    `assertEquals("Signature", props.get("action"))` via
    `getProperties()` (idiom: SecurityProfileTest:472).
  - `actionsInEncryptOnlyWithKeystoreBuilds` / builder with keystore only +
    `actionsIn("Encrypt")` / `build()` then `createInInterceptor()` /
    succeeds; `assertEquals("Encrypt", props.get("action"))` same idiom.
  - `actionsInBothWithBothMaterialsBuilds` / builder with truststore +
    keystore + `actionsIn("Signature Encrypt")` / `build()` then
    `createInInterceptor()` / succeeds;
    `assertEquals("Signature Encrypt", props.get("action"))` same idiom.
  - Red-first: write the two rejection tests before the validator; confirm
    they fail — build() succeeds today AND the arrange's
    `createInInterceptor()` returns `null` (the silent bypass itself,
    degradation #1 in the proposal) — then implement until green.
- **Acceptance criteria**:
  - `mvn`-equivalent Gradle `test` green: prior suite (135) + 7 new = 142,
    `spotlessCheck` green.
  - Red evidence for both rejection tests recorded in the report.
  - No behavior change for blank `actions.in` profiles.

## Task 2: README contract + legacy example

- **Files**:
  - `bridges/cxf/README.md` (modified)

- **Command** (from worktree root): `grep -Fn "actions.in" bridges/cxf/README.md`
  Expected (final): exactly 5 hits, all consistent with the build-time
  contract — Properties-table row, inbound paragraph, CAT112 example, and
  the outbound/validation mentions; no contradictions.
- **Steps**:
  1. The `cxf.security.actions.in` row in the Properties table and the
     inbound paragraph: state the
     build-time contract — explicit Signature requires
     `cxf.truststore.path`; explicit Encrypt requires `cxf.keystore.path`;
     blank keeps the default (`Signature`) which the interceptor
     materializes only with a truststore (manual consumers keep the
     keystore fallback).
  2. Legacy CAT112/Baleares example (~L51-62): add
     `cxf.truststore.path=/etc/camel/keystore.jks` (same JKS,
     self-anchored) so the example stays valid.
- **Tests**: `spotlessCheck` N/A (markdown); verify no other README claims
  contradict the new contract (grep `actions.in` in README — every hit
  consistent).
- **Acceptance criteria**:
  - Example builds under the new validation (semantically: has truststore).
  - No contradiction between table row, paragraph, and example.

- [x] 1
- [x] 2
