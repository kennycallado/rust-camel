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
        "lifecycle/application/ports",
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
    let runtime_ports = root.join("lifecycle/application/ports/runtime_ports.rs");

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
    assert_file_contains(&reload, &["pub async fn execute_reload_actions"]);
    assert_file_prefix_not_contains(&reload, "#[cfg(test)]", &[".route_status("]);
}

#[test]
fn supervision_loop_decisions_are_runtime_query_driven() {
    let supervising = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("lifecycle")
        .join("adapters")
        .join("controller_actor.rs");

    assert_file_section_contains(
        &supervising,
        "pub fn spawn_supervision_task(",
        &[
            "controller: RouteControllerHandle",
            "controller.restart_route(route_id.clone()).await",
        ],
    );
    assert_file_section_not_contains(
        &supervising,
        "pub fn spawn_supervision_task(",
        &[
            "RuntimeQuery::GetRouteStatus",
            "RuntimeCommand::FailRoute",
            "RuntimeCommand::ReloadRoute",
            ".route_status(",
        ],
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
    let context_builder = root.join("context_builder.rs");

    assert_file_contains(&adapters_mod, &["RuntimeExecutionAdapter"]);
    assert_file_contains(&context_builder, &["RuntimeExecutionAdapter::new("]);
}

#[test]
fn domain_has_no_tower_or_framework_types() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lifecycle/domain");
    let mut files = Vec::new();
    collect_rust_files(&root, &mut files);

    // `BoxProcessor` is the contract-level type alias for a Tower
    // `BoxCloneService<Exchange, Exchange, CamelError>` re-exported by
    // `camel_api`. Domain value objects (`CompiledPipeline`) are allowed to
    // hold it as a field because it is the canonical exchange-processor
    // contract, not a framework-integration point. The direct tower usage
    // (`tower::`), composition helpers, and component framework imports are
    // still forbidden — those are the framework-coupling vectors.
    let forbidden = &[
        "tower::",
        "compose_pipeline",
        "TracingProcessor",
        "use camel_component_api::",
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
    let ports_file = root.join("lifecycle/application/ports/runtime_ports.rs");

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
    // The port (`hot_reload/ports/`) is the LEGITIMATE crossing point: it
    // names domain contract types (`CompiledPipeline`, etc.) as the language
    // of its trait surface, because the dependency rule is
    // `domain ← application ← adapters` and the port is the application-ring
    // façade for the hot-reload bounded context. Application code must go
    // THROUGH the port — it must not import domain types directly.
    //
    // `assert_no_forbidden_imports` is directory-recursive, so we cannot use
    // it here: the ports subdir must be excluded. Walk the subdirs manually.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/hot_reload");
    for ring in ["application", "adapters", "domain"] {
        let ring_dir = root.join(ring);
        if !ring_dir.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        collect_rust_files(&ring_dir, &mut files);
        for file in &files {
            let content = fs::read_to_string(file).expect("failed to read source file");
            assert!(
                !content.contains("crate::lifecycle::domain"),
                "hot_reload/{ring} must not import lifecycle::domain directly \
                 (use the ReloadExecutorPort surface, not raw domain types); \
                 found import in {}",
                file.display()
            );
        }
    }
}

#[test]
fn lifecycle_ports_registration_port_is_pub_crate_only() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/lifecycle/application/ports/registration_port.rs");
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

// ---- Stage 3 (Tier C, rc-d0pu.3, task C1): hot_reload/application internals depend on port not concrete handle ----

/// `reload.rs` is deliberately exempt: it owns the PUBLIC
/// `execute_reload_actions(&RuntimeExecutionHandle)` wrapper which coerces
/// to `&dyn ReloadExecutorPort` at internal call sites. Only the internal
/// handlers (`reload_actions.rs`, `drain.rs`) must depend on the port.
#[test]
fn hot_reload_application_internals_depend_on_port_not_concrete_handle() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for file in [
        "hot_reload/application/reload_actions.rs",
        "hot_reload/application/drain.rs",
    ] {
        let path = root.join(file);
        let content = fs::read_to_string(&path).expect("file must exist");
        assert!(
            !content.contains("RuntimeExecutionHandle"),
            "{file} must not reference concrete RuntimeExecutionHandle (use &dyn ReloadExecutorPort)"
        );
    }
}

// ---- Stage 1 (Tier A, rc-d0pu.1): extend coverage to shared/ + confine CQRS bypass ----

#[test]
fn shared_domain_has_no_infrastructure_or_cross_slice_imports() {
    // Mirror the existing `domain_has_no_tower_or_framework_types` rule, plus cross-slice guards.
    // Deliberately NOT forbidding serde / camel_component_api — those are accepted DDD value-object
    // patterns in this codebase (registry.rs holds Arc<dyn Component>; config.rs derives Deserialize).
    let forbidden = &[
        "tower::",
        "BoxProcessor",
        "compose_pipeline",
        "TracingProcessor",
        "crate::lifecycle",
        "crate::context",
        "crate::hot_reload",
    ];
    // shared/ is asymmetric: components/ has only domain/, observability/ has domain/ + adapters/.
    // Scope to the domain sub-layers only — adapters/tracer.rs legitimately imports tower::Service.
    assert_no_forbidden_imports("shared/components/domain", forbidden);
    assert_no_forbidden_imports("shared/observability/domain", forbidden);
}

#[test]
fn cqrs_inflight_bypass_is_confined_to_two_production_sites() {
    // The InFlightCount read bypass (ADR-0045 §4 exception) is a deliberate two-site contract in
    // PRODUCTION query-routing code. Pin both halves AND confine them: the variant may appear only
    // in queries.rs and runtime_bus.rs within lifecycle/application/ — a third file introducing it
    // would be a new read site (silent drift the charter §4 exception forbids).
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let queries = root.join("lifecycle/application/queries.rs");
    let bus = root.join("lifecycle/application/runtime_bus.rs");

    // Site 1: queries.rs InFlightCount arm must return an error (exhaustiveness stub, never reads).
    assert_file_contains(
        &queries,
        &["must be handled by RuntimeBus, not execute_query"],
    );
    // Site 2: runtime_bus.rs must intercept InFlightCount.
    assert_file_contains(&bus, &["RuntimeQuery::InFlightCount", ".in_flight_count("]);

    // Confinement: across lifecycle/application/, RuntimeQuery::InFlightCount may appear ONLY in
    // queries.rs and runtime_bus.rs. (Test-code mentions inside those two files are fine; the
    // #[cfg(test)] stub in lifecycle/adapters/controller_actor.rs is outside application/ entirely.)
    let app_dir = root.join("lifecycle/application");
    let mut files = Vec::new();
    collect_rust_files(&app_dir, &mut files);
    let mut offenders = Vec::new();
    for file in &files {
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "queries.rs" || name == "runtime_bus.rs" {
            continue;
        }
        let content = fs::read_to_string(file).expect("read application src file");
        if content.contains("RuntimeQuery::InFlightCount") {
            offenders.push(file.display().to_string());
        }
    }
    assert!(
        offenders.is_empty(),
        "RuntimeQuery::InFlightCount must appear only in queries.rs and runtime_bus.rs within \
         lifecycle/application/; also found in: {}",
        offenders.join(", ")
    );
}

#[test]
fn flat_root_modules_are_classified_as_slices() {
    // These flat-root modules are classified as single-ring slices or composition
    // roots in ADR-0045 §5. They stay flat by the right-sizing decision (a 3-ring
    // split is only warranted for modules with genuine entity/use-case/adapter
    // separation — see health_registry + datasource). This test asserts the flat
    // files still exist; ring discipline is enforced elsewhere where applicable.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let classified = [
        "context.rs",
        "context_builder.rs",
        "template.rs",
        "registry.rs",
        "language_registry.rs",
        "component_metadata_catalog.rs",
        "startup_validation.rs",
    ];
    for name in classified {
        let p = root.join(name);
        assert!(
            p.exists(),
            "expected classified flat-root slice {} to still exist at {}",
            name,
            p.display()
        );
    }
}

// ---- Stage 2 (Tier B, rc-d0pu.2): entity purity ----

#[test]
fn lifecycle_domain_error_has_no_thiserror() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lifecycle/domain/error.rs");
    let content = std::fs::read_to_string(&path).expect("failed to read domain/error.rs");
    assert!(
        !content.contains("thiserror"),
        "domain/error.rs must not use thiserror (ADR-0045 §4); DomainError uses a manual impl"
    );
}

#[test]
fn runtime_event_entity_has_no_serde_derives() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lifecycle/domain/runtime_event.rs");
    let content = std::fs::read_to_string(&path).expect("failed to read runtime_event.rs");
    // Check no derive attribute names Serialize (robust to doc-comment mentions).
    let derives_serialize = content
        .lines()
        .filter(|l| l.trim_start().starts_with("#[derive("))
        .any(|l| l.contains("Serialize"));
    assert!(
        !derives_serialize,
        "RuntimeEvent entity must not derive Serialize (ADR-0045 §4); persistence uses RuntimeEventRecord"
    );
}

// ---- Stage 6 (Tier A, rc-d0pu.1): honest export feature (A-prime) ----

#[test]
fn export_feature_gates_reexports_not_compilation() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    // (a) Sanity anchor: the adapters module compiles unconditionally (no feature gate).
    let lifecycle_mod = root.join("lifecycle/mod.rs");
    let lm = fs::read_to_string(&lifecycle_mod).expect("read lifecycle/mod.rs");
    assert!(
        lm.contains("pub(crate) mod adapters;"),
        "adapters module must compile unconditionally"
    );

    // (b) The feature name must not appear in any src file other than lib.rs.
    let mut files = Vec::new();
    collect_rust_files(&root, &mut files);
    for file in &files {
        let rel = file.strip_prefix(&root).unwrap_or(file);
        if rel == Path::new("lib.rs") {
            continue;
        }
        let content = fs::read_to_string(file).expect("read src file");
        assert!(
            !content.contains("export-internal-adapters"),
            "export-internal-adapters must not appear outside lib.rs (found in {})",
            file.display()
        );
    }

    // (c) In lib.rs, EVERY line mentioning `export-internal-adapters` must govern an export — check
    //     by the bare feature NAME (not the canonical `cfg(feature = "...")` spelling) so noncanonical
    //     forms like cfg(all(feature = "...", ...)) or cfg_attr are also caught. The next non-blank,
    //     non-comment, non-attribute line must be `pub use` or `pub mod`.
    let lib = root.join("lib.rs");
    let content = fs::read_to_string(&lib).expect("read lib.rs");
    // Presence check: genuinely RED before the rename (0 mentions) and GREEN after.
    let gate_count = content.matches("export-internal-adapters").count();
    assert!(
        gate_count >= 1,
        "expected export-internal-adapters gates in lib.rs, found {gate_count}"
    );
    let lines: Vec<&str> = content.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("export-internal-adapters") {
            continue;
        }
        let mut j = i + 1;
        while j < lines.len() {
            let t = lines[j].trim();
            if t.is_empty() || t.starts_with("//") || t.starts_with("#[") {
                j += 1;
                continue;
            }
            break;
        }
        assert!(
            j < lines.len(),
            "export-internal-adapters reference at lib.rs:{} governs no item",
            i + 1
        );
        let item = lines[j].trim();
        assert!(
            item.starts_with("pub use") || item.starts_with("pub mod"),
            "export-internal-adapters at lib.rs:{} must govern `pub use`/`pub mod`, found `{}`",
            i + 1,
            item
        );
    }

    // (d) The compatibility alias must NOT appear as a source cfg gate. The literal "internal-adapters"
    //     may exist only in Cargo.toml (the alias declaration); no src/**/*.rs file may contain it —
    //     otherwise the old name could silently become a compile gate, re-introducing the N5 confusion.
    //     (Pre-rename lib.rs has 7 such literals, making the RED state concrete; post-rename → 0.)
    let mut alias_offenders = Vec::new();
    for file in &files {
        let content = fs::read_to_string(file).expect("read src file");
        if content.contains("\"internal-adapters\"") {
            alias_offenders.push(file.display().to_string());
        }
    }
    assert!(
        alias_offenders.is_empty(),
        "the \"internal-adapters\" literal must not appear in any src file (Cargo.toml-only alias); \
         found in: {}",
        alias_offenders.join(", ")
    );
}

// ---- Stage 7 (Tier C, rc-d0pu.3, fixes F2 + F3): port → adapter import guard ----

/// Ports live in the application ring; the dependency rule is
/// `domain ← application ← adapters`. A port trait must NOT import types from
/// any adapter ring — contract types the port needs must live in domain (or a
/// contracts module). This guard catches the class of violation F2 fixed
/// (`CompiledPipeline` was imported from `lifecycle::adapters`) and would catch
/// any future regression.
///
/// Ports live in the application ring; the dependency rule is
/// `domain ← application ← adapters`. A port trait must NOT import types from
/// any adapter ring — contract types the port needs must live in domain (or a
/// contracts module). This guard catches the class of violation F2 fixed
/// (`PreparedRoute` was imported from `lifecycle::adapters`; it is now a thin
/// `{ route_id }` token in `lifecycle::domain/route_compilation.rs`).
/// The allow-list is empty — the rule is absolute.
#[test]
fn port_traits_do_not_import_from_adapter_ring() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let port_dirs = ["hot_reload/ports", "lifecycle/application/ports"];
    let allowed_adapter_imports: [&str; 0] = [];
    let mut files = Vec::new();
    for dir in &port_dirs {
        let dir_path = root.join(dir);
        if dir_path.is_dir() {
            collect_rust_files(&dir_path, &mut files);
        }
    }
    for file in &files {
        let content = fs::read_to_string(file).expect("failed to read port file");
        // Normalize whitespace so multiline `use ...::{ A, B };` imports
        // (cargo fmt places `{` at line end, items on following lines) collapse
        // to a single parseable statement. Line-by-line parsing would miss them.
        let normalized: String = content
            .chars()
            .map(|c| if c.is_whitespace() { ' ' } else { c })
            .collect();
        for stmt in normalized.split(';') {
            let stmt = stmt.trim();
            if !stmt.starts_with("use ") || !stmt.contains("crate::") {
                continue;
            }
            // Does the statement reference the `adapters` module at all?
            // Token-level check avoids false positives on identifiers like
            // `not_adapters` (which would falsely match a substring scan).
            let has_adapters_token = stmt
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|tok| tok == "adapters");
            if !has_adapters_token {
                continue;
            }
            if stmt.contains("adapters::") {
                // Item import `use crate::<...>::adapters::<mod>::Item` or
                // `use crate::<...>::adapters::<mod>::{ A, B }`: extract the
                // imported items and check against the allow-list.
                let after_adapters = stmt.split("::adapters::").nth(1).unwrap_or("");
                let last_segment = after_adapters.rsplit("::").next().unwrap_or(after_adapters);
                let items: Vec<&str> = last_segment
                    .trim_matches(|c: char| c == '{' || c == '}' || c == ' ')
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();
                for item in items {
                    assert!(
                        allowed_adapter_imports.contains(&item),
                        "port file {} imports adapter type '{}' via `{stmt}`: ports must not \
                         import from any adapter ring (dependency rule). Only {:?} allowed \
                         (§4 exception).",
                        file.display(),
                        item,
                        allowed_adapter_imports
                    );
                }
            } else {
                // `adapters` appears as a LEAF — a whole-module import
                // (`::adapters;`, `::{adapters}`, `::adapters as x;`, etc.).
                // ALWAYS forbidden: cannot be granularly allow-listed.
                panic!(
                    "port file {} imports the whole adapter module (`{stmt}`): ports must not \
                     import from any adapter ring (dependency rule).",
                    file.display()
                );
            }
        }
    }
}

#[test]
fn context_lifecycle_use_cases_respect_dependency_rule() {
    // C2 + abort-port (rc-d0pu.3 + purge): start/stop/abort algorithms live
    // in lifecycle/application/context_lifecycle.rs as use-cases. All three
    // depend ONLY on port abstractions (RouteOrderingPort for start/stop/
    // shutdown_route_ids; RouteDestructiveTeardownPort for the destructive
    // shutdown()). The use-case ring must not import the concrete
    // RouteControllerHandle.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let context_lifecycle = root.join("lifecycle/application/context_lifecycle.rs");
    assert!(
        context_lifecycle.exists(),
        "context_lifecycle.rs must exist (C2)"
    );
    let content = fs::read_to_string(&context_lifecycle).expect("read context_lifecycle.rs");

    assert!(
        content.contains("RouteOrderingPort"),
        "context_lifecycle.rs must use RouteOrderingPort"
    );
    assert!(
        content.contains("RouteDestructiveTeardownPort"),
        "context_lifecycle.rs must use RouteDestructiveTeardownPort for abort"
    );
    assert!(
        !content.contains("RouteControllerHandle"),
        "context_lifecycle.rs must NOT import the concrete RouteControllerHandle \
         (dependency rule — purged in rc-d0pu.3-purge)"
    );
    assert!(
        !content.contains("InFlightCount"),
        "context_lifecycle.rs must not introduce the InFlightCount CQRS variant"
    );

    let ordering_impl = root.join("lifecycle/adapters/route_ordering_impl.rs");
    assert!(
        ordering_impl.exists(),
        "RouteOrderingPort impl must live in adapters"
    );
}
