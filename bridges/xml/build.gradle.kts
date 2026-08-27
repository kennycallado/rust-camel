plugins {
    java
    id("io.quarkus") version "3.39.1"
    id("com.diffplug.spotless") version "6.25.0"
}

version = project.findProperty("version")?.toString() ?: "0.6.0"

repositories {
    mavenCentral()
}

val quarkusVersion = "3.39.1"

dependencies {
    implementation(enforcedPlatform("io.quarkus.platform:quarkus-bom:$quarkusVersion"))
    implementation("io.quarkus:quarkus-grpc")
    implementation("io.quarkus:quarkus-arc")
    // Required for Quarkus to parse application.yml. Without this extension the
    // YAML file is an inert resource and ALL yml-only config is silently ignored.
    implementation("io.quarkus:quarkus-config-yaml")

    // XSD — Xerces-J JAXP reference impl
    implementation("xerces:xercesImpl:2.12.2")
    implementation("xml-apis:xml-apis:1.4.01")

    // XSLT — Saxon-HE 12.x (MPL-2.0)
    implementation("net.sf.saxon:Saxon-HE:12.5")

    // Saxon pulls xmlresolver 5.x, whose optional Jing (RELAX NG) adapter
    // classes are hard-linked by GraalVM 25 at image build time. The
    // optional dep must exist on the classpath or native-image analysis
    // fails with NoClassDefFoundError: com/thaiopensource/validate/.
    // Exclusions keep our own pins (Saxon 12.5, xml-apis 1.4.01) intact.
    implementation("org.relaxng:jing:20220510") {
        exclude(group = "net.sf.saxon")
        exclude(group = "xml-apis")
    }

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
        googleJavaFormat("1.34.1")
    }
}

tasks.wrapper {
    gradleVersion = "8.10"
    distributionType = org.gradle.api.tasks.wrapper.Wrapper.DistributionType.BIN
}
