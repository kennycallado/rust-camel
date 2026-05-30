use super::{TemplateFile, cargo_toml, gitignore, plugin_toml, readme_md};

pub fn processor_files(plugin_name: &str) -> Vec<TemplateFile> {
    vec![
        TemplateFile {
            path: "Cargo.toml".to_string(),
            content: cargo_toml(plugin_name),
        },
        TemplateFile {
            path: "src/lib.rs".to_string(),
            content: lib_rs().to_string(),
        },
        TemplateFile {
            path: "Camel.plugin.toml".to_string(),
            content: plugin_toml(plugin_name, "processor"),
        },
        TemplateFile {
            path: "README.md".to_string(),
            content: readme_md(plugin_name, "processor"),
        },
        TemplateFile {
            path: ".gitignore".to_string(),
            content: gitignore().to_string(),
        },
        TemplateFile {
            path: "wit/camel-plugin.wit".to_string(),
            content: camel_wit::PLUGIN_WIT.to_string(),
        },
    ]
}

fn lib_rs() -> &'static str {
    "use bindings::camel::plugin::types::{WasmBody, WasmError, WasmExchange};\nuse bindings::Guest;\n\n#[allow(clippy::too_many_arguments)]\nmod bindings {\n    wit_bindgen::generate!({\n        world: \"plugin\",\n        path: \"../wit\",\n    });\n}\n\nstruct Processor;\n\nimpl Guest for Processor {\n    fn init() -> Result<(), String> {\n        Ok(())\n    }\n\n    fn process(mut exchange: WasmExchange) -> Result<WasmExchange, WasmError> {\n        exchange.input.body = match &exchange.input.body {\n            WasmBody::Text(s) => WasmBody::Text(format!(\"processed: {s}\")),\n            other => other.clone(),\n        };\n        Ok(exchange)\n    }\n}\n\nbindings::export!(Processor with_types_in bindings);\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processor_files_contains_expected_paths() {
        let files = processor_files("acme-processor");
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();

        assert_eq!(files.len(), 6);
        assert!(paths.contains(&"Cargo.toml"));
        assert!(paths.contains(&"src/lib.rs"));
        assert!(paths.contains(&"Camel.plugin.toml"));
        assert!(paths.contains(&"README.md"));
        assert!(paths.contains(&".gitignore"));
        assert!(paths.contains(&"wit/camel-plugin.wit"));
    }

    #[test]
    fn cargo_template_contains_cdylib() {
        let cargo = cargo_toml("acme-processor");

        assert!(cargo.contains("name = \"acme-processor\""));
        assert!(cargo.contains("crate-type = [\"cdylib\"]"));
        assert!(cargo.contains("wit-bindgen = \"0.57\""));
        assert!(cargo.contains("[workspace]"));
        assert!(!cargo.contains("camel-wasm-sdk"));
    }

    #[test]
    fn lib_template_generates_bindings() {
        let lib = lib_rs();

        assert!(lib.contains("use bindings::Guest;"));
        assert!(lib.contains("bindings::export!(Processor with_types_in bindings);"));
        assert!(lib.contains("impl Guest for Processor"));
        assert!(lib.contains("fn process("));
        assert!(lib.contains("fn init()"));
        assert!(lib.contains("wit_bindgen::generate!"));
        assert!(lib.contains("mod bindings"));
    }

    #[test]
    fn plugin_toml_contains_expected_keys() {
        let plugin = plugin_toml("acme-processor", "processor");

        assert!(plugin.contains("type = \"processor\""));
        assert!(plugin.contains("entry = \"acme-processor.wasm\""));
    }

    #[test]
    fn gitignore_contains_target_and_plugins() {
        let files = processor_files("acme-processor");
        let gitignore = files
            .iter()
            .find(|f| f.path == ".gitignore")
            .expect("gitignore");
        assert!(gitignore.content.contains("target"));
        assert!(gitignore.content.contains("plugins/"));
    }
}
