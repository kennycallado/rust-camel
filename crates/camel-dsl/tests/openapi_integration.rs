//! Integration test: generate OpenAPI from the rest-crud example YAML
//! and validate the document structure end-to-end.

use camel_dsl::openapi::generate_openapi;
use camel_dsl::yaml::extract_rest_blocks;

/// The REST YAML from `examples/rest-crud/src/main.rs` (REST_YAML const).
/// Kept in sync manually — if the example YAML changes, update this fixture.
const REST_CRUD_YAML: &str = r#"
rest:
  - host: 0.0.0.0
    port: 9090
    path: /api/users
    operations:
      get:
        operation_id: listUsers
        to: direct:listUsers
        produces: application/json
      post:
        operation_id: createUser
        consumes: application/json
        produces: application/json
        success_status: 201
        to: direct:createUser
      put:
        path: /{id}
        operation_id: updateUser
        consumes: application/json
        produces: application/json
        to: direct:updateUser
      delete:
        path: /{id}
        operation_id: deleteUser
        to: direct:deleteUser
        success_status: 204
"#;

#[test]
fn generates_valid_openapi_from_rest_crud() {
    let rest_blocks = extract_rest_blocks(REST_CRUD_YAML).expect("YAML should parse");
    assert_eq!(rest_blocks.len(), 1);

    let result = generate_openapi(&rest_blocks, "Users API", "1.0.0");
    let doc = &result.document;

    // --- Top-level structure ---
    assert_eq!(doc["openapi"], "3.0.3");
    assert_eq!(doc["info"]["title"], "Users API");
    assert_eq!(doc["info"]["version"], "1.0.0");

    // --- Paths ---
    let paths = doc["paths"].as_object().expect("paths should be an object"); // allow-unwrap
    assert!(paths.contains_key("/api/users"));
    assert!(paths.contains_key("/api/users/{id}"));

    // --- GET /api/users (listUsers) ---
    let get_list = &paths["/api/users"]["get"];
    assert_eq!(get_list["operationId"], "listUsers");
    // No path params on /api/users
    assert!(
        get_list.get("parameters").is_none()
            || get_list["parameters"].as_array().unwrap().is_empty()
    ); // allow-unwrap
    // GET default 200
    assert!(get_list["responses"].get("200").is_some());

    // --- POST /api/users (createUser) ---
    let post_create = &paths["/api/users"]["post"];
    assert_eq!(post_create["operationId"], "createUser");
    // POST defaults to 201
    assert!(post_create["responses"].get("201").is_some());
    // POST has requestBody (weak stub since no request_schema in YAML)
    assert!(post_create.get("requestBody").is_some());

    // --- PUT /api/users/{id} (updateUser) ---
    let put_update = &paths["/api/users/{id}"]["put"];
    assert_eq!(put_update["operationId"], "updateUser");
    // Path param extracted
    let params = put_update["parameters"].as_array().unwrap(); // allow-unwrap
    assert_eq!(params.len(), 1);
    assert_eq!(params[0]["name"], "id");
    assert_eq!(params[0]["in"], "path");
    assert_eq!(params[0]["required"], true);

    // --- DELETE /api/users/{id} (deleteUser) ---
    let del = &paths["/api/users/{id}"]["delete"];
    assert_eq!(del["operationId"], "deleteUser");
    // DELETE success_status: 204 → No Content
    let resp204 = &del["responses"]["204"];
    assert!(resp204.get("content").is_none()); // 204 has no body
    assert_eq!(resp204["description"], "No Content");

    // --- Warnings ---
    // All 4 operations lack schemas → 4+ warnings (request for post/put, response for all non-204)
    assert!(
        !result.warnings.is_empty(),
        "Expected warnings for missing schemas, got: {:?}",
        result.warnings
    );
}

#[test]
fn openapi_document_is_serializable() {
    let rest_blocks = extract_rest_blocks(REST_CRUD_YAML).expect("YAML should parse");
    let result = generate_openapi(&rest_blocks, "Test", "0.1.0");

    // Must serialize to pretty JSON without errors
    let json = serde_json::to_string_pretty(&result.document).expect("should serialize"); // allow-unwrap
    assert!(json.contains("\"openapi\": \"3.0.3\""));
    assert!(json.contains("\"operationId\""));
}
