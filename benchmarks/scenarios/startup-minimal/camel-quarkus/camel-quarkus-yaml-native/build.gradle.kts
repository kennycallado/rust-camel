import org.gradle.api.file.DuplicatesStrategy
import org.gradle.language.jvm.tasks.ProcessResources

// camel-quarkus-yaml-native — native-image variant of camel-quarkus-yaml
// (bd rc-p9ki Task 2). Shares Java source and resources (including
// camel/routes.yaml) with the JVM sibling. The native subproject adds
// its own application.properties with the two NixOS+Substrate-VM keys
// (quarkus.native.resources.includes + camel.main.routesIncludePattern)
// documented in benchmarks/spike-results.md; these are required for the
// route YAML to be discoverable at native runtime. Fairness contract:
// JVM and native run the SAME route YAML.

plugins {
    java
    id("io.quarkus") version "3.20.0"
}

version = "1.0.0"

// Source-shared with JVM sibling — NOT duplicated. The YAML JVM sibling
// currently has no Java sources (routes live in camel/routes.yaml), but
// the java.srcDir is declared for parity: if a Java source is added to
// the sibling later, the native image picks it up without code change.
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
