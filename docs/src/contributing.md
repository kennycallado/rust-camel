# Documentation workflow

The README is the concise project landing page, this book provides narrative
guidance, docs.rs hosts API contracts, and runnable examples are the preferred
source for Rust and YAML snippets.

Use named `ANCHOR` regions and mdBook `include` directives when a snippet can
come from a real example. Before submitting documentation changes, run:

```console
mdbook build docs
mdbook test docs
cargo check -p hello-world -p config-basic
cargo test -p camel-dsl --test documentation_examples
```

The Pages workflow publishes `docs/book` from `main`; generated HTML is not
committed. A repository administrator must select **GitHub Actions** as the
Pages build source once in **Settings → Pages**. The structure leaves the
output directory disposable so future release-versioned books can be staged
without changing source chapter URLs.
