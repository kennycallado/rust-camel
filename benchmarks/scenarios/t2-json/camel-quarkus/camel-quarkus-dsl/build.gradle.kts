// camel-quarkus-dsl JVM t2-json subproject (OpenSpec change
// bench-missing-cells task 2.2). Same build pattern as the
// t2-realistic-eip camel-quarkus-dsl subproject (Quarkus 3.20.0 /
// camel-quarkus 3.20.0 BOMs, Java 21); the t2-json route
// (timer -> process(build canonical body + BENCH_INPUT_SHA256) ->
// unmarshal json -> filter jsonpath -> process(insert bench member) ->
// marshal json -> process(assert len == size+13) -> marker) lives in
// src/main/java/com/rustcamel/bench/BenchRoute.java and produces the
// marker `BENCH_ROUTE_READY bytes=<n>`.
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
    implementation("org.apache.camel.quarkus:camel-quarkus-timer")
    implementation("org.apache.camel.quarkus:camel-quarkus-core")
    // camel-quarkus-jsonpath: the `$[?(@.id == 'bench')]` filter predicate
    implementation("org.apache.camel.quarkus:camel-quarkus-jsonpath")
    // camel-quarkus-jackson: unmarshal/marshal JSON (Jackson tree model)
    implementation("org.apache.camel.quarkus:camel-quarkus-jackson")
    // camel-core-languages provides the `jsonpath()` DSL helper used by
    // .filter(jsonpath(...)) in BenchRoute.
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
