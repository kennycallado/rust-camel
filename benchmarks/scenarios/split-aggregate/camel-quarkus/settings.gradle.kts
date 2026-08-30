rootProject.name = "split-aggregate-camel-quarkus"
// v3.5 pattern: the -native subprojects are the MEASURED artifacts and
// srcDir-share with their JVM siblings (source holders). Same as the
// t2-json scenario (task 2.2): the JVM subprojects are ALSO included so
// the canonical-array parity tests (CanonicalArrayTest, task 2.4) run
// via
//   ./gradlew :camel-quarkus-dsl:test :camel-quarkus-yaml:test
// JVM mode stays unmeasured (v3.5 rule) — inclusion is test-only; the
// natives still build the exact same shared route/bean sources.
include("camel-quarkus-dsl", "camel-quarkus-yaml")
include("camel-quarkus-dsl-native", "camel-quarkus-yaml-native")
