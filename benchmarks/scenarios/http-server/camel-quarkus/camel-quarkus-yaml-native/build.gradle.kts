// camel-quarkus-yaml-native — T3 native-image variant of
// camel-quarkus-yaml. Shares Java source and resources with
// the JVM sibling. Same source-shared pattern as the dsl-
// native subproject. The native-image runtime needs both
// `quarkus.native.resources.includes=camel/routes.yaml`
// (Substrate VM enumeration) AND
// `camel.main.routesIncludePattern=classpath:camel/routes.yaml`
// (override the default wildcard that Substrate VM cannot
// enumerate). See application.properties for both keys.

plugins {
    java
    id("io.quarkus") version "3.20.0"
}

version = "1.0.0"

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
    implementation("org.apache.camel.quarkus:camel-quarkus-platform-http")
    implementation("org.apache.camel.quarkus:camel-quarkus-core")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}

// The shared resources dir brings in camel/routes.yaml AND the
// JVM sibling's application.properties. The local
// application.properties carries the native-specific keys —
// the local copy must win so the native runner embeds
// routes.yaml and finds it at runtime. The JVM sibling
// doesn't need the native keys.
tasks.named<ProcessResources>("processResources") {
    duplicatesStrategy = DuplicatesStrategy.EXCLUDE
}
