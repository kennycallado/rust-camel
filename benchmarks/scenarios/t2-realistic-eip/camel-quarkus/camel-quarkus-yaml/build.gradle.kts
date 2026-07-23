// camel-quarkus-yaml JVM T2 subproject (bd rc-p9ki Task 3). The T2
// route shape (timer -> setBody -> setHeader -> filter -> choice -> log)
// lives in src/main/resources/camel/routes.yaml; the JVM harness loads
// it via Camel Quarkus's default classpath:camel/* discovery. The
// marker `BENCH_ROUTE_READY body=pong-bench` carries the post-choice
// body so a wrong-branch run (otherwise -> `pong-other`) is
// observable, not silent.

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
    // camel-core-languages provides the `simple()` predicate language
    // used by the .filter and .choice/when steps in routes.yaml.
    implementation("org.apache.camel:camel-core-languages:${camelQuarkusVersion.removeSuffix(".0")}")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}
