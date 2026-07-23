// camel-quarkus-dsl-native — T3 native-image variant of
// camel-quarkus-dsl (bd Task 3). Shares Java source and
// resources with the JVM sibling, EXCEPT BenchRoute.java,
// which is overridden by a local copy that serves the route
// over `platform-http:` instead of `jetty:`.
//
// HTTP-component divergence (e_opus Experiment 3): raw
// `camel-jetty` bypasses Quarkus' managed Vert.x/Netty HTTP
// layer, costing +18.8% throughput in native mode
// (jetty native 32,770 → platform-http native 38,935 req/s).
// `platform-http` is Quarkus' recommended HTTP component and
// backs onto the same Vert.x/Netty stack Quarkus already
// manages. The JVM sibling stays on `jetty:` for a fair
// comparison with camel-standalone-dsl (which is also jetty),
// so the native variant overrides only this one source file.

plugins {
    java
    id("io.quarkus") version "3.20.0"
}

version = "1.0.0"

// Source-shared with the JVM sibling, EXCEPT BenchRoute.java
// (overridden by the local NativeBenchRoute.java with the
// platform-http URI).
//
// History: an earlier draft used
// `sourceSets { main { java { srcDir(".../src/main/java");
// exclude("**/BenchRoute.java") } } }` — the shared sibling
// has only ONE Java file (BenchRoute.java), so the exclude
// dropped everything and the compile produced zero classes.
// The override is now a self-contained local file in
// `src/main/java/com/rustcamel/bench/NativeBenchRoute.java`,
// so the default sourceSets apply and no explicit
// configuration is needed here.
//
// Local `src/main/resources/application.properties` carries
// the native-specific keys (`quarkus.http.port=8080` for
// platform-http + `quarkus.native.additional-build-args` for
// the Mandrel CE heap). The JVM sibling's properties are
// intentionally NOT inherited — they would override
// `quarkus.http.port` and drop `quarkus.native.*`.

repositories {
    mavenCentral()
}

val quarkusVersion = "3.20.0"
val camelQuarkusVersion = "3.20.0"

dependencies {
    implementation(enforcedPlatform("io.quarkus.platform:quarkus-bom:$quarkusVersion"))
    implementation(enforcedPlatform("org.apache.camel.quarkus:camel-quarkus-bom:$camelQuarkusVersion"))
    implementation("io.quarkus:quarkus-arc")
    implementation("org.apache.camel.quarkus:camel-quarkus-platform-http")
    implementation("org.apache.camel.quarkus:camel-quarkus-core")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}
