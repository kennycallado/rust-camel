plugins {
    java
    id("io.quarkus") version "3.20.0"
    id("com.diffplug.spotless") version "6.25.0"
}

version = project.findProperty("version")?.toString() ?: "0.1.0"

repositories {
    mavenCentral()
}

val quarkusVersion = "3.20.0"

dependencies {
    implementation(enforcedPlatform("io.quarkus.platform:quarkus-bom:$quarkusVersion"))
    implementation("io.quarkus:quarkus-grpc")
    implementation("io.quarkus:quarkus-arc")

    // XSD — Xerces-J JAXP reference impl
    implementation("xerces:xercesImpl:2.12.2")
    implementation("xml-apis:xml-apis:1.4.01")

    // XSLT — Saxon-HE 12.x (MPL-2.0)
    implementation("net.sf.saxon:Saxon-HE:12.5")

    testImplementation("io.quarkus:quarkus-junit5")
    testImplementation("io.grpc:grpc-testing")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}

spotless {
    java {
        target("src/**/*.java")
        googleJavaFormat("1.24.0")
    }
}

tasks.wrapper {
    gradleVersion = "8.10"
    distributionType = org.gradle.api.tasks.wrapper.Wrapper.DistributionType.BIN
}
