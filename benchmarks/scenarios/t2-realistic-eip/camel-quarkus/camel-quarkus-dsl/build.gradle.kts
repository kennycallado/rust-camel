// camel-quarkus-dsl JVM T2 subproject (bd rc-p9ki Task 3). Same dependency
// set as the v1 camel-quarkus-dsl subproject (T1) — the T2 route shape
// (timer -> setBody -> setHeader -> filter -> choice -> log) is
// implemented in src/main/java/com/rustcamel/bench/BenchRoute.java
// and produces the marker `BENCH_ROUTE_READY body=pong-bench` on stdout
// (the post-choice body proves the when-branch executed, not otherwise).

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
    // camel-core-languages provides the `simple()` predicate language
    // used by .filter(simple(...)) and .when(simple(...)) in BenchRoute.
    implementation("org.apache.camel:camel-core-languages:${camelQuarkusVersion.removeSuffix(".0")}")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}
