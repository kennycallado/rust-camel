// camel-quarkus-dsl JVM T3 subproject (bd Task 3). Same dependency
// set as the t2-realistic-eip / v1 camel-quarkus-dsl subprojects —
// the T3 route shape (HTTP server on :8080/bench → setBody(pong))
// is implemented in src/main/java/com/rustcamel/bench/BenchRoute.java.
//
// The marker `BENCH_ROUTE_READY <unix_ms>` is emitted by a
// RouteStarted event notifier installed via a CDI bean
// (`RouteStartedMarker`) — fires AFTER the `camel-quarkus-http`
// (jetty) consumer has bound the listener and the route is
// fully started, satisfying the spec §4.10 listener-bound
// (not first-request) criterion.

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
    // camel-jetty (camel 4.x, the underlying component that
    // provides the `jetty:` server-side scheme). camel-quarkus
    // 3.20's `camel-quarkus-http` extension is CLIENT-side
    // (HTTP Client 5.x), not server-side — and the
    // `camel-quarkus-jetty` extension that wraps
    // `camel-jetty` for Quarkus is not published in 3.20.
    // Pulling `camel-jetty` directly is the documented
    // workaround (the camel-quarkus extensions are
    // optional, not required). NOTE: this is the camel 4.x
    // camel-jetty artifact, NOT a camel-quarkus one — the
    // BOM coordinates the camel 4.x and camel-quarkus 3.20
    // versions independently.
    implementation("org.apache.camel:camel-jetty:4.8.0")
    implementation("org.apache.camel.quarkus:camel-quarkus-core")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}
