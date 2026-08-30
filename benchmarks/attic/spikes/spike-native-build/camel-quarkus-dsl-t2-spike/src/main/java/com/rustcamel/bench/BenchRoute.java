package com.rustcamel.bench;

import org.apache.camel.builder.RouteBuilder;

/**
 * Spike 1B — T2 EIP route exercised under Quarkus native-image build.
 * (bd rc-p9ki Task 1.) Same logical route as the rust-camel YAML spike
 * (spec §4.1): timer -> set_body -> set_header -> filter(simple) ->
 * choice.when(simple)/otherwise -> log with body interpolation.
 *
 * Marker is `BENCH_ROUTE_READY body=pong-bench` — the exact grep
 * target the plan mandates. Body interpolation via Simple language
 * (`${body}`) must work in native mode; this is what the spike
 * validates.
 */
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
