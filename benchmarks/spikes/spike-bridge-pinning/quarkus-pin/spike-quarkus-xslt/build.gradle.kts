// Spike 1B Quarkus native side scratch subproject. Mirrors the
// structure of benchmarks/scenarios/t2-realistic-eip/camel-quarkus
// (camel-quarkus-bom 3.20.0, camel-quarkus version 3.20.0) but adds
// `camel-quarkus-xslt` + `camel-quarkus-validator` to expose the
// Saxon-HE and Xerces-J versions a T4a/T4b Quarkus native fixture
// would resolve against the runtimeClasspath. Throwaway — committed
// for the version report then deleted (or kept as evidence).
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
    // The two T4-relevant extensions:
    implementation("org.apache.camel.quarkus:camel-quarkus-xslt")
    implementation("org.apache.camel.quarkus:camel-quarkus-validator")
    // Explicit XSLT/XSD engine pinning, matching the bridge side
    // (bridges/xml/build.gradle.kts). Without these, camel-xslt /
    // camel-validator resolve to the JDK 21 bundled engines and the
    // tax comparison is invalid per spec §4.8.
    implementation("net.sf.saxon:Saxon-HE:12.5")
    implementation("xerces:xercesImpl:2.12.2")
    implementation("xml-apis:xml-apis:1.4.01")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}
