package com.rustcamel.bench;

import java.io.ByteArrayInputStream;
import java.io.StringWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.time.Instant;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;

import javax.xml.XMLConstants;
import javax.xml.parsers.SAXParserFactory;
import javax.xml.transform.Templates;
import javax.xml.transform.sax.SAXSource;
import javax.xml.transform.stream.StreamResult;

import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;

import net.sf.saxon.Configuration;
import net.sf.saxon.TransformerFactoryImpl;
import net.sf.saxon.lib.Feature;

import org.apache.camel.CamelContext;
import org.apache.camel.builder.RouteBuilder;
import org.apache.camel.impl.event.RouteStartedEvent;
import org.apache.camel.spi.CamelEvent;
import org.apache.camel.support.EventNotifierSupport;
import org.xml.sax.InputSource;

/**
 * T4a XSLT bridge route + marker emitter (Pair A — hardcoded Java DSL)
 * for camel-quarkus-dsl (bd Task 4). Same route shape as the standalone
 * fixture: timer → setBody(1KB XML) → XSLT(identity-transform) →
 * process(emit BENCH_LATENCY).
 *
 * Engine pinning (spec F1 — Saxon-HE 12.5): camel-quarkus-xslt drags
 * in camel-quarkus-support-xalan which, under native-image AOT, locks
 * the JAXP TransformerFactory to Xalan (no reflect-config or system
 * property can override it — ServiceLoader resolution wins at AOT
 * time). Mirror the production bridges/xml/ pattern: drop
 * camel-quarkus-xslt entirely, keep camel-quarkus-core +
 * camel-quarkus-timer for routing, and invoke Saxon-HE 12.5 directly
 * via `new TransformerFactoryImpl(config)` inside the route's
 * processor.
 *
 * Native-image strategy — runtime lazy compilation (mirrors the
 * production bridge): the stylesheet is compiled on first transform
 * via {@link #templates()}, cached in an {@code AtomicReference},
 * exactly as {@code bridges/xml/XsltTransformerService} caches
 * compiled Templates in a {@code ConcurrentHashMap}. There is
 * deliberately NO {@code --initialize-at-build-time=com.rustcamel.
 * bench.BenchRoute}: baking the route class at build time pulls a
 * fully-constructed Saxon {@code Configuration} into the image heap,
 * and that Configuration's {@code commonResolver} references an
 * {@code org.xmlresolver.Resolver} which is (correctly) marked
 * {@code --initialize-at-run-time} — GraalVM rejects a run-time-init
 * object embedded in a build-time heap constant. Compiling at runtime
 * keeps Saxon + xmlresolver entirely out of the build heap.
 *
 * <p>The per-tick transform feeds Saxon a {@code SAXSource} carrying a
 * pre-instantiated {@code XMLReader} (see {@link #secureSaxSource}) so
 * that Saxon never calls {@code Configuration.getSourceParser()} →
 * external Xerces {@code ObjectFactory.createObject(
 * "XIncludeAwareParserConfiguration")}, the {@code Class.forName}
 * chain that fails under native-image regardless of reflect-config.
 *
 * Stylesheet source: classpath resource {@code /identity-transform.xsl}
 * (the JVM sibling's src/main/resources/ copy, shared with the native
 * subproject via sourceSets).
 *
 * Marker + per-tick patterns mirror the standalone App.java; see that
 * file's docstring for the full spec rationale (§4.6, §4.8, §4.10).
 */
@ApplicationScoped
public class BenchRoute extends RouteBuilder {

    @Inject
    CamelContext context;

    private static final AtomicBoolean MARKER_EMITTED = new AtomicBoolean(false);
    private static final AtomicLong TICK_COUNTER = new AtomicLong(0);

    /**
     * Engine class name for the BENCH_ENGINE marker. Saxon's
     * {@code TransformerFactoryImpl} name is a compile-time constant of
     * the pinned Saxon-HE 12.5 dependency (spec F1) — reading it does
     * NOT construct a Saxon {@link Configuration}, so it stays out of
     * the native-image build heap.
     */
    private static final String ENGINE_CLASS_NAME =
            TransformerFactoryImpl.class.getName();

    /**
     * Lazily-compiled Saxon {@link Templates}, populated on first
     * transform at RUNTIME. Mirrors {@code bridges/xml/
     * XsltTransformerService}'s {@code ConcurrentHashMap} stylesheet
     * cache: the production bridge never bakes Templates at build time.
     *
     * <p>This is the load-bearing native-image decision. Baking the
     * Templates into the image heap (the prior approach, via
     * {@code --initialize-at-build-time=BenchRoute}) dragged a
     * fully-constructed Saxon {@code Configuration} into the build-time
     * heap; that Configuration's {@code commonResolver} field holds an
     * {@code org.xmlresolver.Resolver}, which is marked
     * {@code --initialize-at-run-time}. GraalVM rejects a run-time-init
     * object embedded in a build-time-init heap constant
     * (UnsupportedFeatureException). Compiling at runtime — exactly as
     * the production bridge does — keeps the entire Saxon +
     * xmlresolver graph out of the build heap and makes the
     * run-time-init markers consistent.
     */
    private final AtomicReference<Templates> templatesRef =
            new AtomicReference<>();

    private Templates templates() {
        var cached = templatesRef.get();
        if (cached != null) {
            return cached;
        }
        try {
            var config = new Configuration();
            config.setBooleanProperty(Feature.ALLOW_EXTERNAL_FUNCTIONS, false);
            config.setBooleanProperty(Feature.DTD_VALIDATION, false);
            // Mirrors bridges/xml/XsltTransformerService.java
            // getOrCompileTemplates() (lines 156-172): same
            // Configuration flags, same TransformerFactoryImpl direct
            // construction, same secure-processing hardening. Bypasses
            // JAXP ServiceLoader resolution entirely — Xalan is never
            // on the call path.
            var factory = new TransformerFactoryImpl(config);
            factory.setFeature(XMLConstants.FEATURE_SECURE_PROCESSING, true);
            factory.setAttribute(XMLConstants.ACCESS_EXTERNAL_DTD, "");
            factory.setAttribute(XMLConstants.ACCESS_EXTERNAL_STYLESHEET, "");
            // Mirrors bridges/xml: deny any xsl:import()/xsl:include()/document()
            // resolution at stylesheet-compile time. Belt-and-suspenders with
            // ALLOW_EXTERNAL_FUNCTIONS=false above.
            factory.setURIResolver((href, base) -> {
                throw new javax.xml.transform.TransformerException(
                        "External URI access denied: " + href);
            });
            final Templates compiled;
            try (var in = BenchRoute.class.getResourceAsStream("/identity-transform.xsl")) {
                if (in == null) {
                    throw new IllegalStateException(
                            "identity-transform.xsl not on classpath");
                }
                var stylesheetBytes = in.readAllBytes();
                // Use secureSaxSource for stylesheet compile too — same
                // discipline as bridges/xml getOrCompileTemplates() line 170
                // (both document parse AND stylesheet parse go through the
                // hardened XMLReader; never a bare StreamSource).
                compiled = factory.newTemplates(secureSaxSource(stylesheetBytes));
            }
            // Idempotent publish: first writer wins; concurrent racers
            // discard their duplicate and reuse the winner.
            return templatesRef.compareAndSet(null, compiled)
                    ? compiled
                    : templatesRef.get();
        } catch (Exception e) {
            throw new RuntimeException("failed to compile stylesheet", e);
        }
    }

    @Override
    public void configure() {
        // Resolve shared payload from system properties (matches
        // the standalone fixture's contract; the smoke + harness
        // inject worktree-relative paths).
        Path payloadPath = Path.of(System.getProperty(
                "bench.payload",
                "../../../shared/bench-payload.xml"));
        Path latencyFile = Path.of(System.getProperty(
                "bench.latency_file",
                "/tmp/v3-protocol-b-t4a-camel-quarkus-dsl-native.log"));

        final String payload;
        try {
            payload = Files.readString(payloadPath, StandardCharsets.UTF_8);
            Files.createDirectories(latencyFile.getParent());
            Files.writeString(latencyFile, "",
                    StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING);
        } catch (Exception e) {
            throw new RuntimeException("failed to initialize bench assets", e);
        }

        // Engine pinning verification — the harness greps this to
        // confirm Saxon (not Xalan) ran. Emitted at runtime (after
        // the static init has populated ENGINE_CLASS_NAME).
        //
        // BENCH_ENGINE is printed to stdout once at route startup. The
        // harness captures all cell stdout in its main log; grep
        // 'BENCH_ENGINE' in benchmarks/results/v3-*.log to verify
        // Saxon-HE is the active engine. Not captured per-cell in
        // m2-summary.json because the engine is a per-cell-variant
        // constant, not a per-measurement variable.
        System.out.println("BENCH_ENGINE " + ENGINE_CLASS_NAME);
        System.out.flush();

        // Install the marker notifier BEFORE context.start() fires
        // RouteStarted.
        context.getManagementStrategy().addEventNotifier(
            new EventNotifierSupport() {
                @Override
                public void notify(CamelEvent event) {
                    if (MARKER_EMITTED.get()) {
                        return;
                    }
                    if (event instanceof RouteStartedEvent) {
                        if (MARKER_EMITTED.compareAndSet(false, true)) {
                            long unixMs = Instant.now().toEpochMilli();
                            System.out.println("BENCH_ROUTE_READY " + unixMs);
                        }
                    }
                }

                @Override
                public boolean isEnabled(CamelEvent event) {
                    return event.getType() == CamelEvent.Type.RouteStarted;
                }
            }
        );

        from("timer:bench?period=10ms&repeatCount=10000")
            .routeId("bench-xslt")
            .setBody(constant(payload))
            // Records t_start immediately before the Saxon-invoking
            // processor. Long (boxed) so it round-trips through
            // exchange property type erasure. Preserved verbatim from
            // the camel-quarkus-xslt variant (commit 33e91282) — the
            // timing boundary is unchanged.
            .process(exchange -> {
                exchange.setProperty("BenchStart", System.nanoTime());
            })
            // Direct Saxon invocation — bypasses camel-quarkus-xslt
            // entirely. Per-tick work: newTransformer() against the
            // pre-compiled (build-time-baked) Templates + transform(
            // SAXSource, StreamResult). Equivalent in workload to
            // the standalone fixture's `xslt:file://...` step (same
            // engine, same stylesheet, same payload).
            //
            // Native-image parser wiring — mirrors bridges/xml/
            // XsltTransformerService.transform() (line 116-117): the
            // source MUST be a SAXSource carrying a *pre-instantiated*
            // XMLReader. When Saxon receives a SAXSource whose
            // getXMLReader() is non-null it parses with that reader
            // and NEVER calls Configuration.getSourceParser() →
            // loadParser(), so the external Xerces ObjectFactory /
            // Class.forName("XIncludeAwareParserConfiguration") chain
            // (which does not resolve under native-image AOT even with
            // reflect-config) is never reached. A bare StreamSource
            // has no reader, forcing Saxon to allocate one via the
            // failing ObjectFactory path — that was the prior break.
            // The XMLReader is built once here (not in the build-time
            // static initializer) so SAXParserFactory feature
            // negotiation runs at RUNTIME, where the substituted
            // Xerces honours the secure-processing features.
            .process(exchange -> {
                var transformer = templates().newTransformer();
                var xml = exchange.getIn().getBody(String.class);
                var out = new StringWriter();
                transformer.transform(
                        secureSaxSource(xml.getBytes(StandardCharsets.UTF_8)),
                        new StreamResult(out));
                exchange.getIn().setBody(out.toString());
            })
            .process(exchange -> {
                long id = TICK_COUNTER.incrementAndGet();
                long tEnd = System.nanoTime();
                Long tStart = exchange.getProperty("BenchStart", Long.class);
                long durationNs = tEnd - tStart;
                String line = "BENCH_LATENCY " + id + " " + durationNs + "\n";
                try {
                    Files.writeString(latencyFile, line,
                            StandardCharsets.UTF_8,
                            StandardOpenOption.APPEND);
                } catch (Exception e) {
                    // Swallow — harness detects missing records.
                }
            })
            .log("BENCH_XSLT_TICK id=${header.CamelTimerCounter}");
    }

    /**
     * Builds a {@link SAXSource} that carries a fully-configured,
     * pre-instantiated {@link org.xml.sax.XMLReader}. This is the
     * load-bearing native-image fix and mirrors {@code bridges/xml/
     * XsltTransformerService.secureSaxSource()} verbatim.
     *
     * <p>Because the returned SAXSource's {@code getXMLReader()} is
     * non-null, Saxon's {@code Controller.makeSourceTree()} parses
     * with this reader directly and never calls {@code Configuration.
     * getSourceParser()} → {@code loadParser()}. That short-circuits
     * the external Xerces {@code ObjectFactory.createObject(...)} →
     * {@code Class.forName("XIncludeAwareParserConfiguration")} chain
     * that fails under GraalVM native-image. A plain StreamSource
     * would omit the reader and force Saxon down the failing path.
     *
     * <p>Runs at RUNTIME (inside the route processor), so SAX feature
     * negotiation executes against the substituted native-image
     * Xerces, which accepts the secure-processing features. Doing the
     * same work in the build-time static initializer would trip
     * SAXNotRecognizedException under the AOT Xerces substitution.
     *
     * <p><b>The factory is the external xercesImpl
     * ({@code org.apache.xerces.jaxp.SAXParserFactoryImpl}), constructed
     * DIRECTLY — not via {@code SAXParserFactory.newInstance()}.</b> This
     * is load-bearing under native-image. {@code newInstance()} runs
     * JAXP {@code FactoryFinder} service resolution which, in the closed
     * world, resolves to the JDK-internal parser
     * ({@code com.sun.org.apache.xerces.internal...SAXParserImpl}).
     * Saxon 12.5's {@code ActiveSAXSource.deliver()} then calls
     * {@code reader.setProperty(
     * "http://xml.org/sax/properties/lexical-handler", …)}; the
     * JDK-internal reader rejects that property with
     * SAXNotRecognizedException, and formatting that error needs the
     * {@code com.sun.org.apache.xerces.internal.impl.msg.SAXMessages}
     * bundle — absent from the image — surfacing instead as a masking
     * MissingResourceException. The external xercesImpl reader accepts
     * the lexical-handler property, so binding the factory explicitly to
     * xercesImpl (exactly the reader the production bridge uses) avoids
     * the whole cascade.
     */
    private static SAXSource secureSaxSource(byte[] xmlBytes) throws Exception {
        SAXParserFactory spf = new org.apache.xerces.jaxp.SAXParserFactoryImpl();
        spf.setNamespaceAware(true);
        spf.setFeature(XMLConstants.FEATURE_SECURE_PROCESSING, true);
        spf.setFeature(
                "http://apache.org/xml/features/nonvalidating/load-external-dtd", false);
        spf.setFeature(
                "http://xml.org/sax/features/external-general-entities", false);
        spf.setFeature(
                "http://xml.org/sax/features/external-parameter-entities", false);
        spf.setFeature("http://apache.org/xml/features/disallow-doctype-decl", true);
        var reader = spf.newSAXParser().getXMLReader();
        // Cap entity expansion at 100 (bridges/xml line 185 — same property
        // URL). Belt-and-suspenders for future stylesheet changes that might
        // re-introduce DOCTYPE; disallow-doctype-decl above is the primary
        // guard, this limits blast radius if that primary ever regresses.
        reader.setProperty(
                "http://www.oracle.com/xml/jaxp/properties/entityExpansionLimit", 100);
        // Deny any external entity resolution — return an empty source.
        reader.setEntityResolver(
                (publicId, systemId) ->
                        new InputSource(new ByteArrayInputStream(new byte[0])));
        return new SAXSource(reader, new InputSource(new ByteArrayInputStream(xmlBytes)));
    }
}
