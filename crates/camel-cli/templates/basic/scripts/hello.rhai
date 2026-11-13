// Example script step: the route loads this file via `source_file`.
// A `script:` step is a mutating Rhai expression, so assign directly into
// `headers` (set_header()/set_property() are read-only-mode only).
headers["greeting"] = "hello from rhai";
