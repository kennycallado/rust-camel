// camel-quarkus-yaml JVM T3 subproject (bd Task 3). The
// route is authored in src/main/resources/camel/routes.yaml
// and loaded via `quarkus.camel.routes-include-pattern` (set
// in application.properties). Mirrors the t2-realistic-eip
// / v1 camel-quarkus-yaml structure.

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
    // camel-quarkus-yaml-dsl: required for the YAML route
    // parser to load routes.yaml at startup. Pair B's whole
    // point is "route authored in YAML, parsed at runtime".
    implementation("org.apache.camel.quarkus:camel-quarkus-yaml-dsl")
    implementation("org.apache.camel.quarkus:camel-quarkus-platform-http")
    implementation("org.apache.camel.quarkus:camel-quarkus-core")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}
