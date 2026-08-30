// camel-quarkus-yaml JVM split-aggregate subproject (OpenSpec change
// bench-missing-cells task 2.4). Same build pattern as the t2-json
// camel-quarkus-yaml subproject; the split-aggregate routes live in
// src/main/resources/camel/routes.yaml, discovered via Camel Quarkus's
// default classpath:camel/* scan. The two custom route steps (build the
// canonical array + BENCH_INPUT_SHA256, assert the aggregated
// completion) are CDI-produced named beans in
// src/main/java/com/rustcamel/bench/BenchBeans.java, referenced by
// `process: ref:` from the YAML route; the list-append aggregation
// strategy is referenced via the documented `#class:` form
// (ListAppendStrategy, same package).
//
// DIFFERENCES vs t2-json: camel-quarkus-jsonpath is kept (the
// `jsonpath("$")` split expression) but camel-quarkus-jackson is NOT
// needed — the body stays the canonical array STRING (no
// unmarshal/marshal). JUnit is here so the canonical-array parity test
// (CanonicalArrayTest, task 2.4) runs in JVM mode; the -native sibling
// does not need it.

plugins {
    java
    id("io.quarkus") version "3.20.0"
}

version = "1.0.0"

repositories {
    mavenCentral()
}

val quarkusVersion = "3.20.0"
val camelQuarkusVersion = "3.20.0"

dependencies {
    implementation(enforcedPlatform("io.quarkus.platform:quarkus-bom:$quarkusVersion"))
    implementation(enforcedPlatform("org.apache.camel.quarkus:camel-quarkus-bom:$camelQuarkusVersion"))
    implementation("io.quarkus:quarkus-arc")
    implementation("org.apache.camel.quarkus:camel-quarkus-yaml-dsl")
    implementation("org.apache.camel.quarkus:camel-quarkus-timer")
    implementation("org.apache.camel.quarkus:camel-quarkus-direct")
    // camel-quarkus-jsonpath: the `jsonpath("$")` split expression
    implementation("org.apache.camel.quarkus:camel-quarkus-jsonpath")
    // camel-core-languages provides the Simple language used by the
    // aggregation correlation expression and the final `log` step's
    // `${exchangeProperty.CamelAggregatedSize}` interpolation.
    implementation("org.apache.camel:camel-core-languages:${camelQuarkusVersion.removeSuffix(".0")}")
    testImplementation("org.junit.jupiter:junit-jupiter")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}

tasks.named<Test>("test") {
    useJUnitPlatform()
}
