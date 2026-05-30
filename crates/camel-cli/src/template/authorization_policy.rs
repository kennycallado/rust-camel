use super::{TemplateFile, cargo_toml, gitignore, plugin_toml, readme_md, to_pascal_case};

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
            content: readme_md(plugin_name, "authorization-policy"),
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
