use std::collections::HashMap;
use std::sync::Mutex;

use camel_api::{CamelError, RouteTemplateSpec, TemplateError, TemplateInstanceRecord};

/// Thread-safe registry for route templates and their instantiated records.
///
/// The registry stores template specifications (`RouteTemplateSpec`) keyed by
/// their unique ID, and tracks `TemplateInstanceRecord` entries for each
/// template that has been instantiated at runtime.
pub struct TemplateRegistry {
    templates: Mutex<HashMap<String, RouteTemplateSpec>>,
    instances: Mutex<HashMap<String, Vec<TemplateInstanceRecord>>>,
}

impl TemplateRegistry {
    /// Create a new empty `TemplateRegistry`.
    pub fn new() -> Self {
        Self {
            templates: Mutex::new(HashMap::new()),
            instances: Mutex::new(HashMap::new()),
        }
    }

    /// Register a route template specification.
    ///
    /// Returns `Err(CamelError)` if a template with the same ID is already registered.
    pub fn register(&self, spec: RouteTemplateSpec) -> Result<(), CamelError> {
        let id = spec.id.clone();
        let mut templates = self
            .templates
            .lock()
            .expect("template registry mutex poisoned"); // allow-unwrap
        if templates.contains_key(&id) {
            return Err(TemplateError::AlreadyRegistered(id).into());
        }
        templates.insert(id, spec);
        Ok(())
    }

    /// Retrieve a template specification by its ID.
    pub fn get(&self, id: &str) -> Option<RouteTemplateSpec> {
        let templates = self
            .templates
            .lock()
            .expect("template registry mutex poisoned"); // allow-unwrap
        templates.get(id).cloned()
    }

    /// Return all registered template IDs.
    pub fn template_ids(&self) -> Vec<String> {
        let templates = self
            .templates
            .lock()
            .expect("template registry mutex poisoned"); // allow-unwrap
        templates.keys().cloned().collect()
    }

    /// Record a newly instantiated template instance.
    pub fn record_instance(&self, record: TemplateInstanceRecord) {
        let template_id = record.template_id.clone();
        let mut instances = self
            .instances
            .lock()
            .expect("template instances mutex poisoned"); // allow-unwrap
        instances.entry(template_id).or_default().push(record);
    }

    /// Return all instance records for a given template ID.
    pub fn instances(&self, template_id: &str) -> Vec<TemplateInstanceRecord> {
        let instances = self
            .instances
            .lock()
            .expect("template instances mutex poisoned"); // allow-unwrap
        instances.get(template_id).cloned().unwrap_or_default()
    }
}

impl Default for TemplateRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn make_template(id: &str) -> RouteTemplateSpec {
        RouteTemplateSpec {
            id: id.to_string(),
            parameters: vec![],
            routes: vec![serde_json::json!({"from": {"uri": "timer:tick"}})],
        }
    }

    fn make_instance(template_id: &str, route_id: &str) -> TemplateInstanceRecord {
        TemplateInstanceRecord {
            template_id: template_id.to_string(),
            instance_id: Uuid::nil(),
            route_id: route_id.to_string(),
            parameters: BTreeMap::new(),
        }
    }

    #[test]
    fn register_and_get_template() {
        let registry = TemplateRegistry::new();
        let spec = make_template("test-tpl");
        registry.register(spec.clone()).unwrap();

        let retrieved = registry.get("test-tpl").expect("template should exist");
        assert_eq!(retrieved.id, "test-tpl");
    }

    #[test]
    fn get_returns_none_for_unknown_template() {
        let registry = TemplateRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn duplicate_registration_returns_error() {
        let registry = TemplateRegistry::new();
        registry.register(make_template("dup")).unwrap();
        let result = registry.register(make_template("dup"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("already registered"));
    }

    #[test]
    fn template_ids_returns_all_registered_ids() {
        let registry = TemplateRegistry::new();
        registry.register(make_template("a")).unwrap();
        registry.register(make_template("b")).unwrap();
        registry.register(make_template("c")).unwrap();

        let mut ids = registry.template_ids();
        ids.sort();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn record_and_retrieve_instances() {
        let registry = TemplateRegistry::new();
        let inst1 = make_instance("tpl-1", "route-1");
        let inst2 = make_instance("tpl-1", "route-2");
        let inst3 = make_instance("tpl-2", "route-3");

        registry.record_instance(inst1);
        registry.record_instance(inst2);
        registry.record_instance(inst3);

        let tpl1_instances = registry.instances("tpl-1");
        assert_eq!(tpl1_instances.len(), 2);

        let tpl2_instances = registry.instances("tpl-2");
        assert_eq!(tpl2_instances.len(), 1);
        assert_eq!(tpl2_instances[0].route_id, "route-3");
    }

    #[test]
    fn instances_returns_empty_for_unknown_template() {
        let registry = TemplateRegistry::new();
        let instances = registry.instances("nonexistent");
        assert!(instances.is_empty());
    }

    #[test]
    fn template_ids_empty_when_no_templates() {
        let registry = TemplateRegistry::new();
        assert!(registry.template_ids().is_empty());
    }
}
