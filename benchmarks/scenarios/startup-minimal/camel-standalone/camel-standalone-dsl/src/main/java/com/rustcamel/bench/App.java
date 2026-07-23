package com.rustcamel.bench;

import org.apache.camel.builder.RouteBuilder;
import org.apache.camel.main.Main;

/**
 * Pair A entrypoint (hardcoded Java-DSL route) for the startup-minimal
 * benchmark scenario (bd rc-f3g9). No self-instrumentation -- per the final
 * arbitrated measurement design, timing and RSS are captured by the harness
 * from OUTSIDE this process (single clock, bare-metal execution, GNU time
 * -v), not self-reported. This process only needs to print the ready marker.
 *
 * {@code delay=0} on the timer endpoint (oracle e_gpt Fix 3): Camel's timer
 * defaults to a 1000ms initial delay before first fire. Without {@code delay=0}
 * the measured cold-start includes ~1s of idle wait that has nothing to
 * do with startup. {@code repeatCount=1} + {@code delay=0} = fire once immediately.
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
                        .log("BENCH_ROUTE_READY");
            }
        });
        main.run(args);
    }
}
