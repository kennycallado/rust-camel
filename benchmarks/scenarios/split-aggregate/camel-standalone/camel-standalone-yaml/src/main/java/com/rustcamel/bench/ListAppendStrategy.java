package com.rustcamel.bench;

import java.util.ArrayList;
import java.util.List;
import org.apache.camel.AggregationStrategy;
import org.apache.camel.Exchange;

/**
 * List-append {@link AggregationStrategy} for the split-aggregate YAML
 * routes (OpenSpec change bench-missing-cells task 2.4) — the README's
 * `collect_all` strategy. Referenced from routes.yaml via the
 * documented Camel 4 XML/YAML {@code #class:} form:
 *
 * <pre>aggregationStrategy: "#class:com.rustcamel.bench.ListAppendStrategy"</pre>
 *
 * <p>Stateless and instantiation-friendly (public no-arg constructor)
 * so both the standalone Main registry and the Quarkus YAML route can
 * build it without registry lookups.
 */
public class ListAppendStrategy implements AggregationStrategy {

    /// The first fragment seeds a fresh list; every later fragment
    /// appends to the accumulating exchange's body. Defensive copy so
    /// exchange reuse can never alias the accumulated list.
    @Override
    public Exchange aggregate(Exchange oldExchange, Exchange newExchange) {
        List<Object> items = oldExchange == null
                ? new ArrayList<>()
                : new ArrayList<>(oldExchange.getMessage().getBody(List.class));
        items.add(newExchange.getMessage().getBody(String.class));
        Exchange acc = oldExchange == null ? newExchange : oldExchange;
        acc.getMessage().setBody(items);
        return acc;
    }
}
