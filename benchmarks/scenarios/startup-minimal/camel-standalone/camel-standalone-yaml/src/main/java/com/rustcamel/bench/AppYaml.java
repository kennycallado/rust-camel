package com.rustcamel.bench;

import org.apache.camel.main.Main;

/**
 * Pair B entrypoint -- loads the SAME logical route as App.java but
 * authored in routes.yaml and parsed at runtime via camel-yaml-dsl,
 * matching what the rust-camel CLI+YAML contender actually pays for
 * (route-file parsing), unlike App.java's hardcoded Java-DSL route.
 * No self-instrumentation -- see App.java's docstring.
 *
 * <p>The {@code routesIncludePattern} is set explicitly because Camel
 * Main 4.x does NOT auto-discover routes.yaml at the classpath root
 * when running from a fat-jar (jar-with-dependencies) -- the YAML
 * file ends up nested inside the jar and the default discovery
 * mechanism doesn't scan inside the archive for it.
 */
public final class AppYaml {
    private AppYaml() {
    }

    public static void main(String[] args) throws Exception {
        Main main = new Main();
        main.configure()
                .withRoutesIncludePattern("classpath:routes.yaml");
        main.run(args);
    }
}
