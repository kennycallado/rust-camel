// camel-quarkus-dsl JVM split-aggregate subproject (OpenSpec change
// bench-missing-cells task 2.4). Same build pattern as the t2-json
// camel-quarkus-dsl subproject (Quarkus 3.20.0 / camel-quarkus 3.20.0
// BOMs, Java 21); the split-aggregate routes (outer: timer ->
// process(build canonical array + BENCH_INPUT_SHA256) -> split(jsonpath
// "$") -> to(direct:agg-in); agg: setHeader(constant correlation) ->
// aggregate(completionSize=100, list-append) -> process(assert) ->
// marker) live in src/main/java/com/rustcamel/bench/BenchRoute.java and
// produce the marker `BENCH_ROUTE_READY items=100`.
//
// DIFFERENCES vs t2-json: camel-quarkus-jsonpath is kept (the
// `jsonpath("$")` split expression) but camel-quarkus-jackson is NOT
// needed — the body stays the canonical array STRING (no
// unmarshal/marshal; jsonpath evaluates the string directly and the
// splitter iterates the resulting List). aggregate/split/setHeader are
// camel-quarkus-core processors; `direct:` is a core component. JUnit
// is here so the canonical-array parity test (CanonicalArrayTest,
// task 2.4) runs in JVM mode; the -native sibling does not need it.

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
    implementation("org.apache.camel.quarkus:camel-quarkus-direct")
    implementation("org.apache.camel.quarkus:camel-quarkus-core")
    // camel-quarkus-jsonpath: the `jsonpath("$")` split expression
    implementation("org.apache.camel.quarkus:camel-quarkus-jsonpath")
    // camel-core-languages provides the Simple language used by the
    // final `log` step's `${exchangeProperty.CamelAggregatedSize}`
    // interpolation and the aggregation correlation expression.
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
