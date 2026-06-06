use super::{TemplateFile, cargo_toml, gitignore, plugin_toml, to_pascal_case};

pub fn authorization_policy_files(plugin_name: &str) -> Vec<TemplateFile> {
    vec![
        TemplateFile {
            path: "Cargo.toml".to_string(),
            content: cargo_toml(plugin_name),
        },
        TemplateFile {
            path: "src/lib.rs".to_string(),
            content: lib_rs(plugin_name),
        },
        TemplateFile {
            path: "Camel.plugin.toml".to_string(),
            content: plugin_toml(plugin_name, "authorization-policy"),
        },
        TemplateFile {
            path: "wit/camel-plugin.wit".to_string(),
            content: camel_wit::PLUGIN_WIT.to_string(),
        },
        TemplateFile {
            path: "README.md".to_string(),
            content: authorization_policy_readme_md(plugin_name),
        },
        TemplateFile {
            path: ".gitignore".to_string(),
            content: gitignore().to_string(),
        },
    ]
}

fn lib_rs(plugin_name: &str) -> String {
    let plugin_type = to_pascal_case(plugin_name);
    format!(
        "use bindings::camel::plugin::types::{{WasmError, WasmExchange}};\nuse bindings::Guest;\n\nmod bindings {{\n    wit_bindgen::generate!({{\n        world: \"authorization-policy\",\n        path: \"../wit\",\n    }});\n}}\n\nstruct {plugin_type};\n\nimpl Guest for {plugin_type} {{\n    fn init(config: Vec<(String, String)>) -> Result<(), String> {{\n        let _ = config;\n        Ok(())\n    }}\n\n    fn evaluate(exchange: WasmExchange) -> Result<Option<String>, WasmError> {{\n        let roles_prop = bindings::camel::plugin::host::get_property(\"camel.auth.roles\");\n        match roles_prop {{\n            Some(roles_json) if roles_json.contains(\"admin\") => Ok(None),\n            _ => Ok(Some(\"admin role required\".into())),\n        }}\n    }}\n}}\n\nbindings::export!({plugin_type} with_types_in bindings);\n"
    )
}

fn authorization_policy_readme_md(plugin_name: &str) -> String {
    format!(
        r#"# {plugin_name}

WASM authorization-policy plugin for Camel.

## Build

```bash
camel plugin build
```

## Register from `Camel.toml`

```toml
[permissions.providers.{plugin_name}]
provider = "wasm"
path = "plugins/{plugin_name}.wasm"

# Optional runtime limits — defaults: 30s timeout, 50 MiB memory.
[permissions.providers.{plugin_name}.limits]
timeout-secs = 5
max-memory = 10485760
```

## Files

- `src/lib.rs`: policy entrypoint implementing `init(...)` and `evaluate(...)`
- `wit/`: WIT definitions used for guest bindings generation
- `Camel.plugin.toml`: plugin metadata for Camel
"#
    )
}
