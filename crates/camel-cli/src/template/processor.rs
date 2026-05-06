use super::TemplateFile;

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
            content: plugin_toml(plugin_name),
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
        "[package]\nname = \"{plugin_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\ncamel-wasm-sdk = {{ path = \"../../crates/camel-wasm-sdk\" }}\n"
    )
}

fn lib_rs() -> &'static str {
    "use camel_wasm_sdk::{export, Guest, WasmBody, WasmError, WasmExchange};\n\nstruct Processor;\n\nimpl Guest for Processor {\n    fn init() -> Result<(), String> {\n        Ok(())\n    }\n\n    fn process(mut exchange: WasmExchange) -> Result<WasmExchange, WasmError> {\n        exchange.input.body = match &exchange.input.body {\n            WasmBody::Text(s) => WasmBody::Text(format!(\"processed: {s}\")),\n            other => other.clone(),\n        };\n        Ok(exchange)\n    }\n}\n\nexport!(Processor);\n"
}

fn plugin_toml(plugin_name: &str) -> String {
    format!(
        "name = \"{plugin_name}\"\nversion = \"0.1.0\"\ntype = \"processor\"\nentry = \"{plugin_name}.wasm\"\n"
    )
}

fn readme_md(plugin_name: &str) -> String {
    format!(
        "# {plugin_name}\n\nWASM processor plugin for Camel.\n\n## Build\n\n```bash\ncamel plugin build\n```\n\n## Files\n\n- `src/lib.rs`: plugin entrypoint implementing Guest\n- `Camel.plugin.toml`: plugin metadata for Camel\n"
    )
}

fn gitignore() -> &'static str {
    "target\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processor_files_contains_expected_paths() {
        let files = processor_files("acme-processor");
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();

        assert_eq!(files.len(), 5);
        assert!(paths.contains(&"Cargo.toml"));
        assert!(paths.contains(&"src/lib.rs"));
        assert!(paths.contains(&"Camel.plugin.toml"));
        assert!(paths.contains(&"README.md"));
        assert!(paths.contains(&".gitignore"));
    }

    #[test]
    fn cargo_template_contains_cdylib() {
        let cargo = cargo_toml("acme-processor");

        assert!(cargo.contains("name = \"acme-processor\""));
        assert!(cargo.contains("crate-type = [\"cdylib\"]"));
        assert!(cargo.contains("camel-wasm-sdk"));
        assert!(cargo.contains("edition = \"2021\""));
        assert!(!cargo.contains("wit-bindgen"));
    }

    #[test]
    fn lib_template_uses_sdk_directly() {
        let lib = lib_rs();

        assert!(lib.contains("use camel_wasm_sdk::{"));
        assert!(lib.contains("export!(Processor)"));
        assert!(lib.contains("impl Guest for Processor"));
        assert!(lib.contains("fn process("));
        assert!(lib.contains("fn init()"));
        assert!(!lib.contains("wit_bindgen::generate!"));
        assert!(!lib.contains("mod bindings"));
    }

    #[test]
    fn plugin_toml_contains_expected_keys() {
        let plugin = plugin_toml("acme-processor");

        assert!(plugin.contains("type = \"processor\""));
        assert!(plugin.contains("entry = \"acme-processor.wasm\""));
    }
}
