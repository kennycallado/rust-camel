//! `camel openapi generate <file>` — generate an OpenAPI 3.0.3 document
//! from `rest:` blocks in a YAML or JSON route file. `${env:}` placeholders in
//! `rest:` blocks resolve default-only at generate time: string positions
//! take the concrete default, int/bool positions fail per the tree-walk
//! canon, and ambient env is never read.

use std::path::Path;

use clap::{Args, Subcommand};
use serde_json::Value;

use camel_dsl::openapi::generate_openapi;

#[derive(Subcommand, Debug)]
pub enum OpenapiAction {
    /// Generate an OpenAPI 3.0 document from a route file
    Generate(OpenapiGenerateArgs),
}

#[derive(Args, Debug)]
pub struct OpenapiGenerateArgs {
    /// Path to YAML or JSON route file containing `rest:` blocks.
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
/// Reads the file and extracts `rest:` blocks in one sibling call, which
/// also resolves `${env:}` placeholders default-only (string positions take
/// the concrete default; int/bool positions fail per the tree-walk canon;
/// ambient env is never read). Then validates via lowering, generates the
/// OpenAPI document, and returns it. Warnings are printed
/// to stderr by the caller.
pub fn run_generate(args: &OpenapiGenerateArgs) -> Result<Value, String> {
    let rest_blocks =
        camel_dsl::extract_rest_blocks_from_file_with_env(Path::new(&args.file), &|_| None)
            .map_err(|e| format!("route file error: {e}"))?;

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
    use crate::commands::run::tests::EnvVarGuard;
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

    #[test]
    fn generate_resolves_string_default_in_server_url() {
        let yaml = r#"
rest:
  - host: ${env:HOST:-0.0.0.0}
    port: 9090
    path: /api/users
    operations:
      - method: GET
        operation_id: listUsers
        to: direct:listUsers
"#;
        let mut tmp = tempfile::NamedTempFile::with_suffix(".yaml").unwrap(); // allow-unwrap
        write!(tmp, "{yaml}").unwrap(); // allow-unwrap

        let args = OpenapiGenerateArgs {
            file: tmp.path().to_string_lossy().to_string(),
            title: "Test API".to_string(),
            version: "0.1.0".to_string(),
        };

        let doc = run_generate(&args).expect("default-only env resolution should succeed");
        assert_eq!(doc["servers"][0]["url"], "http://0.0.0.0:9090");
        let serialized = serde_json::to_string(&doc).unwrap(); // allow-unwrap
        assert!(
            !serialized.contains("${env"),
            "document must not carry unresolved placeholders: {serialized}"
        );
    }

    #[test]
    fn generate_ambient_env_ignored() {
        let yaml = r#"
rest:
  - host: ${env:RC_GYKDS_CLI_VAR:-0.0.0.0}
    port: 9090
    path: /api/users
    operations:
      - method: GET
        operation_id: listUsers
        to: direct:listUsers
"#;
        let mut tmp = tempfile::NamedTempFile::with_suffix(".yaml").unwrap(); // allow-unwrap
        write!(tmp, "{yaml}").unwrap(); // allow-unwrap

        let args = OpenapiGenerateArgs {
            file: tmp.path().to_string_lossy().to_string(),
            title: "Test API".to_string(),
            version: "0.1.0".to_string(),
        };

        // The guard restores the prior value on drop, so a panicking
        // assertion cannot leak the ambient value into other tests.
        let _env = EnvVarGuard::set("RC_GYKDS_CLI_VAR", "evil");

        let doc = run_generate(&args).expect("ambient env must be ignored, default wins");
        assert_eq!(doc["servers"][0]["url"], "http://0.0.0.0:9090");
        let serialized = serde_json::to_string(&doc).unwrap(); // allow-unwrap
        assert!(
            !serialized.contains("${env"),
            "document must not carry unresolved placeholders: {serialized}"
        );
        assert!(
            !serialized.contains("evil"),
            "document must not surface the ambient value: {serialized}"
        );
    }

    #[test]
    fn generate_int_port_placeholder_errors() {
        let yaml = r#"
rest:
  - host: 127.0.0.1
    port: ${env:PORT:-8080}
    path: /api/users
    operations:
      - method: GET
        operation_id: listUsers
        to: direct:listUsers
"#;
        let mut tmp = tempfile::NamedTempFile::with_suffix(".yaml").unwrap(); // allow-unwrap
        write!(tmp, "{yaml}").unwrap(); // allow-unwrap

        let args = OpenapiGenerateArgs {
            file: tmp.path().to_string_lossy().to_string(),
            title: "API".to_string(),
            version: "1.0.0".to_string(),
        };

        let result = run_generate(&args);
        assert!(result.is_err());
        let err = result.unwrap_err(); // allow-unwrap
        assert!(
            err.contains("type mismatch")
                && err.contains("expected unsigned integer")
                && err.contains("found string"),
            "expected the type-mismatch wording, got: {err}"
        );
        assert!(
            !err.contains("not set"),
            "must not surface env wording: {err}"
        );
        assert!(!err.contains("PORT"), "must not name the variable: {err}");
    }

    #[test]
    fn generate_no_default_names_variable() {
        let yaml = r#"
rest:
  - host: ${env:REST_HOST}
    port: 9090
    path: /api/users
    operations:
      - method: GET
        operation_id: listUsers
        to: direct:listUsers
"#;
        let mut tmp = tempfile::NamedTempFile::with_suffix(".yaml").unwrap(); // allow-unwrap
        write!(tmp, "{yaml}").unwrap(); // allow-unwrap

        let args = OpenapiGenerateArgs {
            file: tmp.path().to_string_lossy().to_string(),
            title: "API".to_string(),
            version: "1.0.0".to_string(),
        };

        let result = run_generate(&args);
        assert!(result.is_err());
        let err = result.unwrap_err(); // allow-unwrap
        assert!(
            err.contains("REST_HOST") && err.contains("not set"),
            "expected the error to name REST_HOST as not set, got: {err}"
        );
    }

    #[test]
    fn generate_success_status_placeholder_errors() {
        let yaml = r#"
rest:
  - host: 127.0.0.1
    port: 9090
    path: /api/users
    operations:
      - method: POST
        operation_id: createUser
        consumes: application/json
        produces: application/json
        success_status: ${env:CODE:-201}
        to: direct:createUser
"#;
        let mut tmp = tempfile::NamedTempFile::with_suffix(".yaml").unwrap(); // allow-unwrap
        write!(tmp, "{yaml}").unwrap(); // allow-unwrap

        let args = OpenapiGenerateArgs {
            file: tmp.path().to_string_lossy().to_string(),
            title: "API".to_string(),
            version: "1.0.0".to_string(),
        };

        let result = run_generate(&args);
        assert!(result.is_err());
        let err = result.unwrap_err(); // allow-unwrap
        assert!(
            err.contains("type mismatch")
                && err.contains("expected unsigned integer")
                && err.contains("found string"),
            "expected the type-mismatch wording, got: {err}"
        );
        assert!(
            !err.contains("not set"),
            "must not surface env wording: {err}"
        );
        assert!(!err.contains("CODE"), "must not name the variable: {err}");
    }

    #[test]
    fn generate_free_value_schema_position_resolves() {
        let yaml = r#"
rest:
  - host: 127.0.0.1
    port: 9090
    path: /api/users
    operations:
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
              type: ${env:T:-string}
"#;
        let mut tmp = tempfile::NamedTempFile::with_suffix(".yaml").unwrap(); // allow-unwrap
        write!(tmp, "{yaml}").unwrap(); // allow-unwrap

        let args = OpenapiGenerateArgs {
            file: tmp.path().to_string_lossy().to_string(),
            title: "Test API".to_string(),
            version: "0.1.0".to_string(),
        };

        let doc = run_generate(&args).expect("free-value schema position should resolve");
        let schema = &doc["paths"]["/api/users"]["post"]["requestBody"]["content"]["application/json"]
            ["schema"];
        assert_eq!(schema["properties"]["name"]["type"], "string");
        let serialized = serde_json::to_string(&doc).unwrap(); // allow-unwrap
        assert!(
            !serialized.contains("${env"),
            "document must not carry unresolved placeholders: {serialized}"
        );
    }

    #[test]
    fn generate_json_arm_parity() {
        // String-default host resolves to a concrete server URL.
        let resolves = r#"{"rest":[{"host":"${env:HOST:-0.0.0.0}","port":9090,"path":"/api/users","operations":[{"method":"GET","operation_id":"listUsers","to":"direct:listUsers"}]}]}"#;
        let mut tmp = tempfile::NamedTempFile::with_suffix(".json").unwrap(); // allow-unwrap
        write!(tmp, "{resolves}").unwrap(); // allow-unwrap

        let args = OpenapiGenerateArgs {
            file: tmp.path().to_string_lossy().to_string(),
            title: "API".to_string(),
            version: "1.0.0".to_string(),
        };
        let doc = run_generate(&args).expect("json string default should resolve");
        assert_eq!(doc["servers"][0]["url"], "http://0.0.0.0:9090");
        let serialized = serde_json::to_string(&doc).unwrap(); // allow-unwrap
        assert!(
            !serialized.contains("${env"),
            "document must not carry unresolved placeholders: {serialized}"
        );

        // Port placeholder fails deserialization: the spliced leaf is a JSON
        // string where u16 is expected (verbatim serde_json wording).
        let int_port = r#"{"rest":[{"host":"127.0.0.1","port":"${env:PORT:-8080}","path":"/api/users","operations":[{"method":"GET","operation_id":"listUsers","to":"direct:listUsers"}]}]}"#;
        let mut tmp = tempfile::NamedTempFile::with_suffix(".json").unwrap(); // allow-unwrap
        write!(tmp, "{int_port}").unwrap(); // allow-unwrap

        let args = OpenapiGenerateArgs {
            file: tmp.path().to_string_lossy().to_string(),
            title: "API".to_string(),
            version: "1.0.0".to_string(),
        };
        let result = run_generate(&args);
        assert!(result.is_err());
        let err = result.unwrap_err(); // allow-unwrap
        assert!(
            err.contains("invalid type"),
            "expected serde_json invalid-type wording, got: {err}"
        );
        assert!(
            !err.contains("not set"),
            "must not surface env wording: {err}"
        );
        assert!(!err.contains("PORT"), "must not name the variable: {err}");

        // No-default token fails naming the variable.
        let no_default = r#"{"rest":[{"host":"${env:REST_HOST}","port":9090,"path":"/api/users","operations":[{"method":"GET","operation_id":"listUsers","to":"direct:listUsers"}]}]}"#;
        let mut tmp = tempfile::NamedTempFile::with_suffix(".json").unwrap(); // allow-unwrap
        write!(tmp, "{no_default}").unwrap(); // allow-unwrap

        let args = OpenapiGenerateArgs {
            file: tmp.path().to_string_lossy().to_string(),
            title: "API".to_string(),
            version: "1.0.0".to_string(),
        };
        let result = run_generate(&args);
        assert!(result.is_err());
        let err = result.unwrap_err(); // allow-unwrap
        assert!(
            err.contains("REST_HOST") && err.contains("not set"),
            "expected the error to name REST_HOST as not set, got: {err}"
        );
    }
}
