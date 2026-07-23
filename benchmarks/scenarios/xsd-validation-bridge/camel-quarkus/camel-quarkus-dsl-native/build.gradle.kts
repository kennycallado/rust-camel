// camel-quarkus-dsl-native — T4b native-image variant of
// camel-quarkus-dsl (bd Task 4). Shares Java source with the JVM sibling.

plugins {
    java
    id("io.quarkus") version "3.20.0"
}

version = "1.0.0"

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
    implementation("org.apache.camel.quarkus:camel-quarkus-validator")
    implementation("org.apache.camel.quarkus:camel-quarkus-timer")
    implementation("org.apache.camel.quarkus:camel-quarkus-core")
    implementation("net.sf.saxon:Saxon-HE:12.5")
    implementation("xerces:xercesImpl:2.12.2")
    implementation("xml-apis:xml-apis:1.4.01")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}
