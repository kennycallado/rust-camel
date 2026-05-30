use bindings::camel::plugin::types::{WasmError, WasmExchange};
use bindings::Guest;

mod bindings {
    wit_bindgen::generate!({
        world: "authorization-policy",
        path: "wit",
    });
}

struct RoleCheck;

static REQUIRED_ROLE: &str = "admin";

impl Guest for RoleCheck {
    fn init(config: Vec<(String, String)>) -> Result<(), String> {
        let _ = config;
        Ok(())
    }

    fn evaluate(exchange: WasmExchange) -> Result<Option<String>, WasmError> {
        let roles_prop = bindings::camel::plugin::host::get_property("camel.auth.roles");
        let roles_str = match roles_prop {
            Some(s) => s,
            None => return Ok(Some("no roles found".into())),
        };

        // roles_str is a JSON array: ["admin","user"]
        // Check if the required role appears as a JSON string value
        let has_required = roles_str.contains(&format!("\"{REQUIRED_ROLE}\""));

        if has_required {
            Ok(None)
        } else {
            Ok(Some(format!("role '{REQUIRED_ROLE}' required, got: {roles_str}")))
        }
    }
}

bindings::export!(RoleCheck with_types_in bindings);
