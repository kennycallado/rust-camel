// camel-quarkus-dsl-native — T4a native-image variant of
// camel-quarkus-dsl (bd Task 4). Shares Java source and resources
// with the JVM sibling via sourceSets (same pattern as T3).
//
// Fairness contract: JVM (builds but not measured) and native run the
// SAME route code (the source-shared java srcDir ensures this — the
// build is just a re-package with native-image flags at invocation
// time).

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
    // Routing (timer + core Camel DSL): kept. camel-quarkus-xslt is
    // intentionally ABSENT — it drags in camel-quarkus-support-xalan
    // which under native-image AOT locks the JAXP TransformerFactory
    // to Xalan. The BenchRoute invokes Saxon directly inside a
    // processor (mirrors bridges/xml/XsltTransformerService.java).
    implementation("org.apache.camel.quarkus:camel-quarkus-timer")
    implementation("org.apache.camel.quarkus:camel-quarkus-core")
    // Engine pinning (spec F1): Saxon-HE 12.5 + Xerces-J, identical
    // to bridges/xml/ and the JVM sibling.
    implementation("net.sf.saxon:Saxon-HE:12.5")
    implementation("xerces:xercesImpl:2.12.2")
    implementation("xml-apis:xml-apis:1.4.01")
    // Jing: required at native-image build time because Saxon-HE 12.5's
    // transitive xmlresolver:5.2.2 ships org.xmlresolver.utils.SaxProducer
    // which extends com.thaiopensource.validate.ValidationDriver$SaxProducer
    // (a Jing class). Saxon-HE's pom excludes Jing; without it native-image
    // analysis aborts with NoClassDefFoundError during heap scanning.
    // The class is linked but never invoked at runtime for our identity
    // transform workload (zero SGML/TR9401 catalog access).
    implementation("org.relaxng:jing:20220510") {
        exclude(group = "xml-apis", module = "xml-apis")
    }
    // Pin httpclient5 to 5.1.3 (matches bridges/xml/): Saxon's transitive
    // xmlresolver uses HttpClientBuilder which references Brotli decoding.
    // In 5.4.x (camel-quarkus-bom's enforced-platform default) this is
    // `org.brotli:dec`; in 5.1.3 it is brotli4j (service-loaded, lazily).
    // The enforced platform uprates our 5.1.3 pin to 5.4.2, so we add
    // the missing Brotli decoder directly (not invoked at runtime for
    // our identity-transform workload).
    implementation("org.apache.httpcomponents.client5:httpclient5:5.1.3")
    implementation("org.brotli:dec:0.1.2")
}

java {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
}
