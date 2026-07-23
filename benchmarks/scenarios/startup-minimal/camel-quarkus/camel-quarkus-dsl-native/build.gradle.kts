// camel-quarkus-dsl-native — native-image variant of camel-quarkus-dsl
// (bd rc-p9ki Task 2). Shares Java source and resources with the JVM
// sibling; the native-image build is driven entirely by -D flags at
// invocation time, so this build file is intentionally a near-copy of
// ../camel-quarkus-dsl/build.gradle.kts plus a sourceSets block that
// pulls in the sibling's src/main/. Fairness contract: JVM and native
// run the SAME route code (spec §4.2 / Risk #11).

plugins {
    java
    id("io.quarkus") version "3.20.0"
}

version = "1.0.0"

// Source-shared with JVM sibling — NOT duplicated. The java + resources
// srcDirs point at ../camel-quarkus-dsl so any change to the JVM route
// lands in the native image on the next build (no copy step).
sourceSets {
    main {
        java.srcDir("../camel-quarkus-dsl/src/main/java")
        resources.srcDir("../camel-quarkus-dsl/src/main/resources")
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
    implementation("org.apache.camel.quarkus:camel-quarkus-timer")
    implementation("org.apache.camel.quarkus:camel-quarkus-core")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}
