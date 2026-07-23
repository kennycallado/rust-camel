// T2 scenario route for camel-quarkus-dsl (JVM, Pair A, bd rc-p9ki Task 3).
// Mirrors the v1 BenchRoute (at
// benchmarks/scenarios/startup-minimal/camel-quarkus/camel-quarkus-dsl/
// src/main/java/com/rustcamel/bench/BenchRoute.java) but implements
// the spec §4.1 T2 route: timer -> setBody -> setHeader -> filter ->
// choice.when/otherwise -> log. The marker `BENCH_ROUTE_READY
// body=pong-bench` carries the post-choice body so a wrong-branch
// run (otherwise -> `pong-other`) is observable, not silent.

package com.rustcamel.bench;

import org.apache.camel.builder.RouteBuilder;

public class BenchRoute extends RouteBuilder {
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
}
