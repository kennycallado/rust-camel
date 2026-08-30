// camel-quarkus-dsl-native — t2-json native-image variant of
// camel-quarkus-dsl (OpenSpec change bench-missing-cells task 2.2).
// Shares Java source with the JVM sibling; the native-image build is
// driven by `-D` flags at invocation time, so this build file is a
// near-copy of ../camel-quarkus-dsl/build.gradle.kts plus a
// sourceSets block pointing at the sibling. Fairness contract: JVM and
// native run the SAME route code. The dependency set adds
// camel-quarkus-jsonpath + camel-quarkus-jackson (t2-json needs them;
// see the JVM sibling's build file).

plugins {
    java
    id("io.quarkus") version "3.20.0"
}

version = "1.0.0"

// Source-shared with JVM sibling — NOT duplicated. The java +
// resources srcDirs point at ../camel-quarkus-dsl so any change to
// the JVM route lands in the native image on the next build (no
// copy step).
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
    // camel-quarkus-jsonpath: the `$[?(@.id == 'bench')]` filter predicate
    implementation("org.apache.camel.quarkus:camel-quarkus-jsonpath")
    // camel-quarkus-jackson: unmarshal/marshal JSON (Jackson tree model)
    implementation("org.apache.camel.quarkus:camel-quarkus-jackson")
    implementation("org.apache.camel:camel-core-languages:${camelQuarkusVersion.removeSuffix(".0")}")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}
