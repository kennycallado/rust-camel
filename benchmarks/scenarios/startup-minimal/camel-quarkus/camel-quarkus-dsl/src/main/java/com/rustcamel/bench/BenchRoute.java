package com.rustcamel.bench;

import org.apache.camel.builder.RouteBuilder;

/**
 * Pair A — hardcoded Java-DSL route for the startup-minimal benchmark
 * scenario (bd rc-f3g9). No self-instrumentation — timing/RSS are
 * captured by the harness from outside this process (single clock,
 * bare-metal, GNU time -v), not self-reported. Only the ready marker
 * is printed.
 *
 * `delay=0` (oracle e_gpt Fix 3): Camel's timer defaults to 1000ms
 * initial delay. Without it the measured cold-start includes ~1s of
 * idle wait that has nothing to do with startup. `repeatCount=1&delay=0`
 * = fire once immediately on route start.
 */
public class BenchRoute extends RouteBuilder {
    @Override
    public void configure() {
        from("timer:bench?repeatCount=1&delay=0")
                .log("BENCH_ROUTE_READY");
    }
}
