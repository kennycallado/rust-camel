use std::fs;
use std::path::{Path, PathBuf};

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).expect("failed to read directory");
    for entry in entries {
        let entry = entry.expect("failed to read directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn assert_no_forbidden_imports(layer_dir: &str, forbidden: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(layer_dir);
    let mut files = Vec::new();
    collect_rust_files(&root, &mut files);
    for file in files {
        let content = fs::read_to_string(&file).expect("failed to read source file");
        for pattern in forbidden {
            assert!(
                !content.contains(pattern),
                "hexagonal boundary violation in {}: found forbidden import pattern `{}`",
                file.display(),
                pattern
            );
        }
    }
}

fn assert_file_contains(path: &Path, required: &[&str]) {
    let content = fs::read_to_string(path).expect("failed to read source file");
    for pattern in required {
        assert!(
            content.contains(pattern),
            "expected pattern `{}` in {}",
            pattern,
            path.display()
        );
    }
}

fn assert_file_not_contains(path: &Path, forbidden: &[&str]) {
    let content = fs::read_to_string(path).expect("failed to read source file");
    for pattern in forbidden {
        assert!(
            !content.contains(pattern),
            "found forbidden pattern `{}` in {}",
            pattern,
            path.display()
        );
    }
}

fn assert_file_prefix_not_contains(path: &Path, split_at: &str, forbidden: &[&str]) {
    let content = fs::read_to_string(path).expect("failed to read source file");
    let prefix = match content.split_once(split_at) {
        Some((before, _)) => before,
        None => content.as_str(),
    };

    for pattern in forbidden {
        assert!(
            !prefix.contains(pattern),
            "found forbidden pattern `{}` in {} before `{}`",
            pattern,
            path.display(),
            split_at
        );
    }
}

fn assert_file_section_contains(path: &Path, section_start: &str, required: &[&str]) {
    let content = fs::read_to_string(path).expect("failed to read source file");
    let section = content
        .split(section_start)
        .nth(1)
        .unwrap_or_else(|| panic!("missing section `{section_start}` in {}", path.display()));
    let section = section.split("#[cfg(test)]").next().unwrap_or(section);
    for pattern in required {
        assert!(
            section.contains(pattern),
            "expected pattern `{}` in section `{}` of {}",
            pattern,
            section_start,
            path.display()
        );
    }
}

fn assert_file_section_not_contains(path: &Path, section_start: &str, forbidden: &[&str]) {
    let content = fs::read_to_string(path).expect("failed to read source file");
    let section = content
        .split(section_start)
        .nth(1)
        .unwrap_or_else(|| panic!("missing section `{section_start}` in {}", path.display()));
    let section = section.split("#[cfg(test)]").next().unwrap_or(section);
    for pattern in forbidden {
        assert!(
            !section.contains(pattern),
            "found forbidden pattern `{}` in section `{}` of {}",
            pattern,
            section_start,
            path.display()
        );
    }
}

#[test]
fn domain_layer_has_no_infrastructure_dependencies() {
    assert_no_forbidden_imports(
        "lifecycle/domain",
        &[
            "crate::lifecycle::adapters",
            "crate::lifecycle::application",
            "crate::context",
            "crate::reload",
            "crate::route_controller",
            "crate::supervising_route_controller",
        ],
    );
}

#[test]
fn application_layer_depends_on_ports_and_domain_only() {
    assert_no_forbidden_imports(
        "lifecycle/application",
        &[
            "crate::context",
            "crate::reload",
            "crate::route_controller",
            "crate::supervising_route_controller",
        ],
    );
}

#[test]
fn ports_layer_only_uses_approved_application_imports() {
    assert_no_forbidden_imports(
        "lifecycle/ports",
        &[
            "crate::lifecycle::adapters",
            "crate::context",
            "crate::reload",
            "crate::route_controller",
            "crate::supervising_route_controller",
            "crate::lifecycle::application::commands",
            "crate::lifecycle::application::runtime_bus",
            "crate::lifecycle::application::queries",
            // `internal_commands` module was removed; keep guarding concrete app internals that still exist.
            "crate::lifecycle::application::supervision_service",
        ],
    );
}

#[test]
fn domain_and_ports_do_not_import_runtime_contract_types_from_camel_api_directly() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let route_runtime = root.join("lifecycle/domain/route_runtime.rs");
    let runtime_ports = root.join("lifecycle/ports/runtime_ports.rs");

    assert_file_not_contains(
        &route_runtime,
        &["use camel_api::{CamelError, RuntimeEvent};"],
    );
    assert_file_not_contains(
        &runtime_ports,
        &[
            "use camel_api::{CamelError, RuntimeEvent};",
            "use crate::route::RouteDefinition;",
        ],
    );
    assert_file_contains(
        &runtime_ports,
        &[
            "use crate::lifecycle::application::route_definition::RouteDefinition",
            "use crate::lifecycle::domain::{RouteRuntimeAggregate, RuntimeEvent}",
        ],
    );
    assert_file_not_contains(
        &runtime_ports,
        &["use crate::lifecycle::domain::{RouteDefinition"],
    );
}

#[test]
fn runtime_bus_and_command_handlers_are_controller_agnostic() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let runtime_bus = root.join("lifecycle/application/runtime_bus.rs");
    let commands = root.join("lifecycle/application/commands.rs");

    assert_file_not_contains(
        &runtime_bus,
        &[
            "RouteControllerInternal",
            "with_controller(",
            "route_status(",
        ],
    );
    assert_file_not_contains(
        &commands,
        &[
            "RouteControllerInternal",
            "with_controller(",
            "route_status(",
        ],
    );
}

#[test]
fn runtime_side_effects_flow_through_execution_port() {
    let commands = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("lifecycle/application/commands.rs");

    assert_file_contains(
        &commands,
        &[
            "RuntimeExecutionPort",
            "pub execution: Option<Arc<dyn RuntimeExecutionPort>>",
            "if let Some(execution) = &deps.execution",
        ],
    );
}

#[test]
fn reload_runtime_path_does_not_use_controller_local_status_heuristics() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let reload_watcher = root.join("hot_reload/adapters/reload_watcher.rs");
    let reload = root.join("hot_reload/application/reload.rs");

    assert_file_contains(&reload_watcher, &["runtime_route_ids()"]);
    assert_file_contains(&reload, &["runtime_route_status("]);
    assert_file_contains(
        &reload,
        &["pub(crate) fn compute_reload_actions_from_runtime_snapshot"],
    );
    assert_file_contains(&reload, &["pub(crate) async fn execute_reload_actions"]);
    assert_file_prefix_not_contains(&reload, "#[cfg(test)]", &[".route_status("]);
}

#[test]
fn supervision_loop_decisions_are_runtime_query_driven() {
    let supervising = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("lifecycle")
        .join("application")
        .join("supervision_service.rs");

    assert_file_section_contains(
        &supervising,
        "async fn supervision_loop(",
        &[
            "RuntimeQuery::GetRouteStatus",
            "RuntimeCommand::FailRoute",
            "RuntimeCommand::ReloadRoute",
        ],
    );
    assert_file_section_not_contains(
        &supervising,
        "async fn supervision_loop(",
        &[".route_status("],
    );
}

#[test]
fn runtime_execution_handle_has_no_raw_controller_escape_hatch() {
    let context = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("context.rs");
    assert_file_not_contains(&context, &["fn raw(&self)"]);
    assert_file_contains(
        &context,
        &["ProducerContext::new().with_runtime(self.runtime())"],
    );
    assert_file_not_contains(&context, &["ProducerContext::new(route_controller)"]);
}

#[test]
fn public_route_controller_trait_exposes_no_lifecycle_read_model() {
    let route_controller_api = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("camel-api")
        .join("src")
        .join("route_controller.rs");
    assert_file_not_contains(&route_controller_api, &["fn route_status("]);

    let core_lib = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("lib.rs");
    assert_file_not_contains(
        &core_lib,
        &["pub use route_controller::{DefaultRouteController, RouteControllerInternal};"],
    );
}

#[test]
fn runtime_execution_adapter_uses_semantic_executor_naming() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let adapters_mod = root.join("lifecycle/adapters/mod.rs");
    let context = root.join("context.rs");

    assert_file_contains(&adapters_mod, &["RuntimeExecutionAdapter"]);
    assert_file_contains(&context, &["RuntimeExecutionAdapter::new("]);
    assert_file_not_contains(&context, &["RouteControllerExecutionAdapter::new("]);
}

#[test]
fn domain_has_no_tower_or_framework_types() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lifecycle/domain");
    let mut files = Vec::new();
    collect_rust_files(&root, &mut files);

    let forbidden = &[
        "tower::",
        "BoxProcessor",
        "compose_pipeline",
        "TracingProcessor",
        "use camel_component::",
    ];

    for file in &files {
        let content = std::fs::read_to_string(file).expect("failed to read source file");
        for pattern in forbidden {
            assert!(
                !content.contains(pattern),
                "domain boundary violation in {}: found forbidden pattern `{}`",
                file.display(),
                pattern
            );
        }
    }
}

#[test]
fn ports_imports_route_definition_from_application_not_adapters() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let ports_file = root.join("lifecycle/ports/runtime_ports.rs");

    assert_file_contains(
        &ports_file,
        &["use crate::lifecycle::application::route_definition::RouteDefinition"],
    );

    assert_file_not_contains(&ports_file, &["crate::lifecycle::adapters::"]);
}

#[test]
fn domain_does_not_import_application_or_adapters() {
    assert_no_forbidden_imports(
        "lifecycle/domain",
        &[
            "crate::lifecycle::application::",
            "crate::lifecycle::adapters::",
        ],
    );
}

#[test]
fn lifecycle_domain_does_not_import_hot_reload() {
    assert_no_forbidden_imports("lifecycle/domain", &["crate::hot_reload", "hot_reload::"]);
}

#[test]
fn lifecycle_domain_does_not_import_lifecycle_application_or_adapters() {
    assert_no_forbidden_imports(
        "lifecycle/domain",
        &[
            "crate::lifecycle::application",
            "crate::lifecycle::adapters",
        ],
    );
}

#[test]
fn lifecycle_application_does_not_import_hot_reload() {
    assert_no_forbidden_imports(
        "lifecycle/application",
        &["crate::hot_reload", "hot_reload::"],
    );
}

#[test]
fn hot_reload_does_not_import_lifecycle_domain_directly() {
    // hot_reload may use lifecycle application/adapters types directly (by design),
    // but must never bypass the layering by importing raw domain types.
    assert_no_forbidden_imports("hot_reload", &["crate::lifecycle::domain"]);
}

#[test]
fn lifecycle_ports_registration_port_is_pub_crate_only() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/lifecycle/ports/registration_port.rs");
    let content = std::fs::read_to_string(&path).expect("failed to read registration_port.rs");
    assert!(
        content.contains("pub(crate) trait RouteRegistrationPort"),
        "RouteRegistrationPort must be pub(crate), not pub"
    );
    assert!(
        !content.contains("pub trait RouteRegistrationPort"),
        "RouteRegistrationPort must NOT be pub (only pub(crate))"
    );
}

#[test]
fn lifecycle_does_not_use_old_flat_config_path() {
    assert_no_forbidden_imports(
        "lifecycle",
        &["crate::config::", "crate::tracer::", "crate::registry::"],
    );
}

#[test]
fn hot_reload_does_not_use_old_flat_config_path() {
    assert_no_forbidden_imports(
        "hot_reload",
        &["crate::config::", "crate::tracer::", "crate::registry::"],
    );
}
