package com.rustcamel.bench;

import java.util.ArrayList;
import java.util.List;
import io.quarkus.runtime.annotations.RegisterForReflection;
import org.apache.camel.AggregationStrategy;
import org.apache.camel.Exchange;

/**
 * List-append {@link AggregationStrategy} for the split-aggregate YAML
 * routes (OpenSpec change bench-missing-cells task 2.4) — the README's
 * `collect_all` strategy. Referenced from camel/routes.yaml via the
 * documented Camel 4 XML/YAML `#class:` form:
 *
 * <pre>aggregationStrategy: "#class:com.rustcamel.bench.ListAppendStrategy"</pre>
 *
 * <p>Stateless and instantiation-friendly (public no-arg constructor).
 * Per-family duplication of the standalone-yaml sibling's class is
 * deliberate (pairing classpath isolation).
 */
// #class: lookup is reflective: native-image strips the class unless
// registered. Found at the first container run (2026-08-31): the
// YAML-native runner died with ClassNotFoundException before its
// marker; the DSL sibling instantiates directly and never tripped.
@RegisterForReflection
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
