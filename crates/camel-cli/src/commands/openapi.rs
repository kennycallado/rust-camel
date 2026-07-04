//! `camel openapi generate <file>` — generate an OpenAPI 3.0.3 document
//! from `rest:` blocks in a YAML route file.

use std::path::Path;

use clap::{Args, Subcommand};
use serde_json::Value;

use camel_dsl::openapi::generate_openapi;
use camel_dsl::yaml::extract_rest_blocks;

#[derive(Subcommand, Debug)]
pub enum OpenapiAction {
    /// Generate an OpenAPI 3.0 document from a route file
    Generate(OpenapiGenerateArgs),
}

#[derive(Args, Debug)]
pub struct OpenapiGenerateArgs {
    /// Path to YAML route file containing `rest:` blocks.
    pub file: String,

    /// API title for the OpenAPI `info` section.
    #[arg(long, default_value = "Generated API")]
    pub title: String,

    /// API version for the OpenAPI `info` section.
    #[arg(long, default_value = "1.0.0")]
    pub version: String,
}

/// Run the `openapi generate` subcommand logic.
///
/// Reads the file, extracts `rest:` blocks, validates them via lowering,
/// generates the OpenAPI document, and returns it. Warnings are printed
/// to stderr by the caller.
pub fn run_generate(args: &OpenapiGenerateArgs) -> Result<Value, String> {
    let content = std::fs::read_to_string(&args.file)
        .map_err(|e| format!("failed to read '{}': {e}", args.file))?;

    let extension = Path::new(&args.file)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");

    let rest_blocks = match extension {
        "yaml" | "yml" => {
            extract_rest_blocks(&content).map_err(|e| format!("YAML parse error: {e}"))?
        }
        "json" => {
            let dsl: camel_dsl::route_ast::RouteDslRoutes =
                serde_json::from_str(&content).map_err(|e| format!("JSON parse error: {e}"))?;
            dsl.rest
        }
        _ => extract_rest_blocks(&content).map_err(|e| format!("parse error (tried YAML): {e}"))?,
    };

    if rest_blocks.is_empty() {
        return Err("no 'rest:' blocks found in file".to_string());
    }

    // I3: validate via lowering + duplicate route ID check before generation
    let lowered = camel_dsl::rest::lower_all_rest_to_routes(&rest_blocks)
        .map_err(|e| format!("validation error: {e}"))?;
    camel_dsl::rest::check_duplicate_route_ids(&lowered)
        .map_err(|e| format!("validation error: {e}"))?;

    let result = generate_openapi(&rest_blocks, &args.title, &args.version);

    for warning in &result.warnings {
        eprintln!("warning: {warning}");
    }

    Ok(result.document)
}

/// M2: CLI entrypoint wrapper (consistent with other commands).
pub fn run(action: OpenapiAction) {
    match action {
        OpenapiAction::Generate(args) => match run_generate(&args) {
            Ok(doc) => {
                let pretty =
                    serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string()); // allow-unwrap
                println!("{pretty}");
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn generate_from_yaml_file() {
        let yaml = r#"
rest:
  - host: 0.0.0.0
    port: 9090
    path: /api/users
    operations:
      - method: GET
        operation_id: listUsers
        to: direct:listUsers
      - method: POST
        operation_id: createUser
        consumes: application/json
        produces: application/json
        success_status: 201
        to: direct:createUser
        request_schema:
          type: object
          properties:
            name:
              type: string
          required: [name]
"#;
        let mut tmp = tempfile::NamedTempFile::new().unwrap(); // allow-unwrap
        write!(tmp, "{yaml}").unwrap(); // allow-unwrap

        let args = OpenapiGenerateArgs {
            file: tmp.path().to_string_lossy().to_string(),
            title: "Test API".to_string(),
            version: "0.1.0".to_string(),
        };

        let doc = run_generate(&args).expect("generation should succeed");
        assert_eq!(doc["openapi"], "3.0.3");
        assert_eq!(doc["info"]["title"], "Test API");
        assert_eq!(doc["info"]["version"], "0.1.0");

        let paths = &doc["paths"];
        assert!(paths["/api/users"].get("get").is_some());
        assert!(paths["/api/users"].get("post").is_some());

        // Post has explicit request_schema
        let post_schema =
            &paths["/api/users"]["post"]["requestBody"]["content"]["application/json"]["schema"];
        assert_eq!(post_schema["properties"]["name"]["type"], "string");
    }

    #[test]
    fn generate_from_json_file() {
        let json = r#"{"rest":[{"host":"0.0.0.0","port":9090,"path":"/api/users","operations":[{"method":"GET","operation_id":"listUsers","to":"direct:listUsers"}]}]}"#;
        let mut tmp = tempfile::NamedTempFile::with_suffix(".json").unwrap(); // allow-unwrap
        write!(tmp, "{json}").unwrap(); // allow-unwrap

        let args = OpenapiGenerateArgs {
            file: tmp.path().to_string_lossy().to_string(),
            title: "Test API".to_string(),
            version: "1.0.0".to_string(),
        };

        let doc = run_generate(&args).expect("generation should succeed");
        assert_eq!(doc["openapi"], "3.0.3");
        assert_eq!(
            doc["paths"]["/api/users"]["get"]["operationId"],
            "listUsers"
        );
    }

    #[test]
    fn error_when_no_rest_blocks() {
        let yaml = r#"
routes:
  - id: myRoute
    from: timer:tick
    steps:
      - to: log:info
"#;
        let mut tmp = tempfile::NamedTempFile::new().unwrap(); // allow-unwrap
        write!(tmp, "{yaml}").unwrap(); // allow-unwrap

        let args = OpenapiGenerateArgs {
            file: tmp.path().to_string_lossy().to_string(),
            title: "API".to_string(),
            version: "1.0.0".to_string(),
        };

        let result = run_generate(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no 'rest:' blocks")); // allow-unwrap
    }

    #[test]
    fn validation_error_on_duplicate_path_verb() {
        let yaml = r#"
rest:
  - host: 0.0.0.0
    port: 9090
    path: /api/users
    operations:
      - method: GET
        operation_id: listUsers
        to: direct:listUsers
  - host: 0.0.0.0
    port: 9090
    path: /api/users
    operations:
      - method: GET
        operation_id: listUsers2
        to: direct:listUsers
"#;
        let mut tmp = tempfile::NamedTempFile::new().unwrap(); // allow-unwrap
        write!(tmp, "{yaml}").unwrap(); // allow-unwrap

        let args = OpenapiGenerateArgs {
            file: tmp.path().to_string_lossy().to_string(),
            title: "API".to_string(),
            version: "1.0.0".to_string(),
        };

        let result = run_generate(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("validation error"));
    }

    #[test]
    fn error_when_file_not_found() {
        let args = OpenapiGenerateArgs {
            file: "/nonexistent/file.yaml".to_string(),
            title: "API".to_string(),
            version: "1.0.0".to_string(),
        };
        let result = run_generate(&args);
        assert!(result.is_err());
    }
}
