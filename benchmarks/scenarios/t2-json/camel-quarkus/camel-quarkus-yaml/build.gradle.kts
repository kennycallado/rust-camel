// camel-quarkus-yaml JVM t2-json subproject (OpenSpec change
// bench-missing-cells task 2.2). Same build pattern as the
// t2-realistic-eip camel-quarkus-yaml subproject; the t2-json route
// lives in src/main/resources/camel/routes.yaml, discovered via Camel
// Quarkus's default classpath:camel/* scan. The three custom route
// steps (build canonical body + BENCH_INPUT_SHA256, insert the bench
// member, assert output length) are CDI-produced named beans in
// src/main/java/com/rustcamel/bench/BenchBeans.java, referenced by
// `process: ref:` from the YAML route.
//
// ADDED vs t2-realistic-eip: camel-quarkus-jsonpath (the
// `$[?(@.id == 'bench')]` predicate) and camel-quarkus-jackson
// (unmarshal/marshal to the Jackson tree model). JUnit is here so the
// canonical-body parity test (CanonicalBodyTest, task 2.2) runs in
// JVM mode; the -native sibling does not need it.

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
    // camel-quarkus-jsonpath: the `$[?(@.id == 'bench')]` filter predicate
    implementation("org.apache.camel.quarkus:camel-quarkus-jsonpath")
    // camel-quarkus-jackson: unmarshal/marshal JSON (Jackson tree model)
    implementation("org.apache.camel.quarkus:camel-quarkus-jackson")
    // camel-core-languages provides the Simple language used by the
    // final `log` step's `${header.benchOutLen}` interpolation.
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
