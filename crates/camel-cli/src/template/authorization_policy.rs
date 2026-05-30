use super::TemplateFile;

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
            content: plugin_toml(plugin_name),
        },
        TemplateFile {
            path: "wit/camel-plugin.wit".to_string(),
            content: camel_wit::PLUGIN_WIT.to_string(),
        },
        TemplateFile {
            path: "README.md".to_string(),
            content: readme_md(plugin_name),
        },
        TemplateFile {
            path: ".gitignore".to_string(),
            content: gitignore().to_string(),
        },
    ]
}

fn cargo_toml(plugin_name: &str) -> String {
    format!(
        "[package]\nname = \"{plugin_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nwit-bindgen = \"0.57\"\n"
    )
}

fn lib_rs(plugin_name: &str) -> String {
    let plugin_type = to_pascal_case(plugin_name);
    format!(
        "use bindings::camel::plugin::types::{{WasmError, WasmExchange}};\nuse bindings::Guest;\n\nmod bindings {{\n    wit_bindgen::generate!({{\n        world: \"authorization-policy\",\n        path: \"../wit\",\n    }});\n}}\n\nstruct {plugin_type};\n\nimpl Guest for {plugin_type} {{\n    fn init(config: Vec<(String, String)>) -> Result<(), String> {{\n        let _ = config;\n        Ok(())\n    }}\n\n    fn evaluate(exchange: WasmExchange) -> Result<Option<String>, WasmError> {{\n        let roles_prop = bindings::camel::plugin::host::get_property(\"camel.auth.roles\");\n        match roles_prop {{\n            Some(roles_json) if roles_json.contains(\"admin\") => Ok(None),\n            _ => Ok(Some(\"admin role required\".into())),\n        }}\n    }}\n}}\n\nbindings::export!({plugin_type} with_types_in bindings);\n"
    )
}

fn plugin_toml(plugin_name: &str) -> String {
    format!(
        "name = \"{plugin_name}\"\nversion = \"0.1.0\"\ntype = \"authorization-policy\"\nentry = \"{plugin_name}.wasm\"\n"
    )
}

fn readme_md(plugin_name: &str) -> String {
    format!(
        "# {plugin_name}\n\nWASM authorization-policy plugin for Camel.\n\n## Build\n\n```bash\ncamel plugin build\n```\n\n## Files\n\n- `src/lib.rs`: plugin entrypoint implementing authorization-policy Guest methods\n- `wit/`: WIT definitions used for guest bindings generation\n- `Camel.plugin.toml`: plugin metadata for Camel\n"
    )
}

fn gitignore() -> &'static str {
    "target\nplugins/\n"
}

fn to_pascal_case(name: &str) -> String {
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            let mut out = String::new();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                out.extend(chars.flat_map(|c| c.to_lowercase()));
            }
            out
        })
        .collect()
}
