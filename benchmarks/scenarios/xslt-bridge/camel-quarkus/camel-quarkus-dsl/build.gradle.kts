// camel-quarkus-dsl JVM T4a subproject (bd Task 4). Uses
// camel-quarkus-xslt + explicit Saxon-HE 12.5 dep to mirror the
// engine pinning of bridges/xml (spec §4.8). The same Java source
// is compiled into both JVM and native variants (source-shared
// via camel-quarkus-dsl-native below).
//
// The marker `BENCH_ROUTE_READY <unix_ms>` is emitted by a
// RouteStarted event notifier installed via a CDI bean inside
// BenchRoute.configure() — fires after the timer consumer is
// registered and the route is fully started (Task 3 pattern).

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
    // camel-quarkus-xslt: Quarkus-packaged XSLT component (in-process
    // Saxon-HE 12.5, NOT a bridge sidecar — the Apache Camel
    // counterpart to rust-camel's bridges/xml XSLT delegation).
    implementation("org.apache.camel.quarkus:camel-quarkus-xslt")
    implementation("org.apache.camel.quarkus:camel-quarkus-timer")
    implementation("org.apache.camel.quarkus:camel-quarkus-core")
    // Explicit engine pinning per spec §4.8 + Task 1 Spike 1B.
    // camel-quarkus-xslt does NOT pull Saxon-HE transitively; without
    // this the JDK 21 bundled engine is used and the tax comparison
    // is invalid.
    implementation("net.sf.saxon:Saxon-HE:12.5")
    implementation("xerces:xercesImpl:2.12.2")
    implementation("xml-apis:xml-apis:1.4.01")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}
