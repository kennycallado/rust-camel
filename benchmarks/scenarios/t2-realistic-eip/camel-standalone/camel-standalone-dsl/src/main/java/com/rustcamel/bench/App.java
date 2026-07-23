package com.rustcamel.bench;

import org.apache.camel.builder.RouteBuilder;
import org.apache.camel.main.Main;

/**
 * Pair A entrypoint (hardcoded Java-DSL route) for the T2 scenario
 * (bd rc-p9ki Task 3). Mirrors the v1 `startup-minimal` App.java
 * (at {@code benchmarks/scenarios/startup-minimal/camel-standalone/
 * camel-standalone-dsl/src/main/java/com/rustcamel/bench/App.java})
 * but implements the spec §4.1 T2 route: timer -> set_body ->
 * set_header -> filter(simple) -> choice.when(simple)/otherwise ->
 * log with body interpolation.
 *
 * <p>The T2 marker {@code BENCH_ROUTE_READY body=pong-bench} carries
 * the post-choice body so a wrong-branch run (otherwise -> {@code
 * pong-other}) is observable, not silent. The harness greps stdout
 * for this exact string.
 *
 * <p>No self-instrumentation -- per the final arbitrated measurement
 * design (v1 plan, e_gpt), timing and RSS are captured by the
 * harness from OUTSIDE this process. This process only prints the
 * ready marker.
 *
 * <p>{@code delay=0} on the timer endpoint (oracle e_gpt Fix 3):
 * Camel's timer defaults to a 1000ms initial delay before first
 * fire. Without {@code delay=0} the measured cold-start includes
 * ~1s of idle wait that has nothing to do with startup.
 */
public final class App {
    private App() {
    }

    public static void main(String[] args) throws Exception {
        Main main = new Main();
        main.configure().addRoutesBuilder(new RouteBuilder() {
            @Override
            public void configure() {
                from("timer:bench?repeatCount=1&delay=0")
                        .setBody(constant("ping"))
                        .setHeader("source", constant("bench"))
                        .filter(simple("${body} == 'ping'"))
                        .choice()
                            .when(simple("${header.source} == 'bench'"))
                                .setBody(constant("pong-bench"))
                            .otherwise()
                                .setBody(constant("pong-other"))
                        .endChoice()
                        .end()
                        .log("BENCH_ROUTE_READY body=${body}");
            }
        });
        main.run(args);
    }
}
