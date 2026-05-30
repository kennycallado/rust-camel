use super::{TemplateFile, cargo_toml, gitignore, plugin_toml, readme_md, to_pascal_case};

pub fn bean_files(plugin_name: &str) -> Vec<TemplateFile> {
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
            content: plugin_toml(plugin_name, "bean"),
        },
        TemplateFile {
            path: "README.md".to_string(),
            content: readme_md(plugin_name, "bean"),
        },
        TemplateFile {
            path: ".gitignore".to_string(),
            content: gitignore().to_string(),
        },
        TemplateFile {
            path: "wit/camel-bean.wit".to_string(),
            content: camel_bean_wit().to_string(),
        },
        TemplateFile {
            path: "wit/camel-plugin.wit".to_string(),
            content: camel_plugin_wit().to_string(),
        },
    ]
}

fn lib_rs(plugin_name: &str) -> String {
    let plugin_type = to_pascal_case(plugin_name);
    format!(
        "use bindings::camel::plugin::types::{{WasmBody, WasmError, WasmExchange}};\nuse bindings::Guest;\n\nmod bindings {{\n    wit_bindgen::generate!({{\n        world: \"bean\",\n        path: \"../wit\",\n    }});\n}}\n\nstruct {plugin_type};\n\nimpl Guest for {plugin_type} {{\n    fn init() -> Result<(), String> {{\n        Ok(())\n    }}\n\n    fn methods() -> Vec<String> {{\n        vec![\"hello\".into()]\n    }}\n\n    fn invoke(method: String, mut exchange: WasmExchange) -> Result<WasmExchange, WasmError> {{\n        match method.as_str() {{\n            \"hello\" => {{\n                let text = match &exchange.input.body {{\n                    WasmBody::Text(s) => s.clone(),\n                    _ => String::new(),\n                }};\n                exchange.input.body = WasmBody::Text(format!(\"Hello from {plugin_name}: {{text}}\"));\n                Ok(exchange)\n            }}\n            _ => Err(WasmError::ProcessorError(format!(\"unknown method: {{method}}\"))),\n        }}\n    }}\n}}\n\nbindings::export!({plugin_type} with_types_in bindings);\n"
    )
}

fn camel_bean_wit() -> &'static str {
    camel_wit::BEAN_WIT
}

fn camel_plugin_wit() -> &'static str {
    camel_wit::PLUGIN_WIT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bean_files_contains_expected_paths() {
        let files = bean_files("acme-bean");
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();

        assert_eq!(files.len(), 7);
        assert!(paths.contains(&"Cargo.toml"));
        assert!(paths.contains(&"src/lib.rs"));
        assert!(paths.contains(&"Camel.plugin.toml"));
        assert!(paths.contains(&"README.md"));
        assert!(paths.contains(&".gitignore"));
        assert!(paths.contains(&"wit/camel-bean.wit"));
        assert!(paths.contains(&"wit/camel-plugin.wit"));
    }

    #[test]
    fn cargo_template_contains_workspace_and_wit_bindgen() {
        let cargo = cargo_toml("acme-bean");

        assert!(cargo.contains("name = \"acme-bean\""));
        assert!(cargo.contains("crate-type = [\"cdylib\"]"));
        assert!(cargo.contains("[workspace]"));
        assert!(cargo.contains("wit-bindgen = \"0.57\""));
        assert!(!cargo.contains("camel-wasm-sdk"));
    }

    #[test]
    fn lib_template_uses_wit_bindgen_and_bean_world() {
        let lib = lib_rs("acme-bean");

        assert!(lib.contains("wit_bindgen::generate!"));
        assert!(lib.contains("world: \"bean\""));
        assert!(lib.contains("path: \"../wit\""));
        assert!(lib.contains("impl Guest for AcmeBean"));
        assert!(lib.contains("fn methods() -> Vec<String>"));
        assert!(lib.contains("fn invoke("));
        assert!(lib.contains("Hello from acme-bean: {text}"));
        assert!(lib.contains("bindings::export!(AcmeBean with_types_in bindings);"));
        assert!(!lib.contains("camel_wasm_sdk"));
    }

    #[test]
    fn plugin_toml_contains_expected_keys() {
        let plugin = plugin_toml("acme-bean", "bean");

        assert!(plugin.contains("type = \"bean\""));
        assert!(plugin.contains("entry = \"acme-bean.wasm\""));
    }

    #[test]
    fn wit_templates_include_expected_worlds() {
        let bean_wit = camel_bean_wit();
        let plugin_wit = camel_plugin_wit();

        assert!(bean_wit.contains("world bean"));
        assert!(bean_wit.contains("import host;"));
        assert!(plugin_wit.contains("interface types"));
        assert!(plugin_wit.contains("interface host"));
    }

    #[test]
    fn gitignore_contains_target_and_plugins() {
        let files = bean_files("acme-bean");
        let gitignore = files
            .iter()
            .find(|f| f.path == ".gitignore")
            .expect("gitignore");
        assert!(gitignore.content.contains("target"));
        assert!(gitignore.content.contains("plugins/"));
    }
}
