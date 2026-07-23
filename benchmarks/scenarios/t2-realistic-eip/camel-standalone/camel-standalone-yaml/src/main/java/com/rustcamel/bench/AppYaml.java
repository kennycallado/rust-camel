package com.rustcamel.bench;

import org.apache.camel.main.Main;

/**
 * Pair B entrypoint for the T2 scenario (bd rc-p9ki Task 3). Mirrors
 * the v1 `startup-minimal` AppYaml.java (at
 * {@code benchmarks/scenarios/startup-minimal/camel-standalone/camel-
 * standalone-yaml/src/main/java/com/rustcamel/bench/AppYaml.java})
 * but loads the T2 routes.yaml which implements the spec §4.1 T2
 * route: timer -> set_body -> set_header -> filter -> choice ->
 * log. Same logical route as {@code App.java} but authored in YAML
 * and parsed at runtime via {@code camel-yaml-dsl}.
 *
 * <p>Pair B is language-subsystem-equivalent to rust-camel-cli
 * (both sides use {@code ${body}} / {@code ${header.X}} Simple —
 * see {@code rust-camel-cli/routes/t2-realistic-eip.yaml}). Pair A
 * (rust-camel-lib) is NOT language-subsystem-equivalent (closure
 * predicates — see that fixture's source comment).
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
