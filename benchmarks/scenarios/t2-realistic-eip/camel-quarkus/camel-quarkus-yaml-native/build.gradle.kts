import org.gradle.api.file.DuplicatesStrategy
import org.gradle.language.jvm.tasks.ProcessResources

// camel-quarkus-yaml-native — T2 native-image variant of
// camel-quarkus-yaml (bd rc-p9ki Task 3). Shares Java source and
// resources (including camel/routes.yaml) with the JVM sibling.
// Mirrors the v1 camel-quarkus-yaml-native subproject's two
// NixOS+Substrate-VM keys (see
// benchmarks/spike-results.md "NixOS + native-image resource
// discovery" and the v1 fixture's src/main/resources/
// application.properties).
//
// Both keys are load-bearing:
// - quarkus.native.resources.includes registers camel/routes.yaml
//   with Substrate VM at build time so getResource("camel/routes.yaml")
//   (singular) works in native mode.
// - camel.main.routesIncludePattern overrides the DEFAULT
//   `classpath:camel/*,...` pattern. Substrate VM cannot enumerate
//   directory contents, so the wildcard `camel/*` returns no
//   resources at native runtime even when the file is embedded.
//   Pointing the pattern at the specific file bypasses the
//   wildcard and the route is discovered.

plugins {
    java
    id("io.quarkus") version "3.20.0"
}

version = "1.0.0"

// Source-shared with JVM sibling — NOT duplicated. The YAML JVM
// sibling has no Java sources (routes live in camel/routes.yaml),
// but the java.srcDir is declared for parity: if a Java source is
// added to the sibling later, the native image picks it up
// without code change.
sourceSets {
    main {
        java.srcDir("../camel-quarkus-yaml/src/main/java")
        resources.srcDir("../camel-quarkus-yaml/src/main/resources")
    }
}

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
    implementation("org.apache.camel:camel-core-languages:${camelQuarkusVersion.removeSuffix(".0")}")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}

// The shared resources dir brings in camel/routes.yaml AND the JVM
// sibling's application.properties (banner-only). The native
// subproject's own src/main/resources/application.properties carries
// the banner line PLUS the two native-specific keys — the local copy
// must win so the native runner embeds routes.yaml and finds it at
// runtime. EXCLUDE = first srcDir encountered wins; default
// `src/main/resources` is added by the java plugin before the
// sourceSets block above, so the local file is seen first.
tasks.named<ProcessResources>("processResources") {
    duplicatesStrategy = DuplicatesStrategy.EXCLUDE
}
