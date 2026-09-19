use crate::input::Key;
#[cfg(test)]
use crate::workflow::parameter::ParameterSnapshot;
use crate::workflow::parameter::{ParameterBinding, ParameterRegistry, ParameterState};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

#[path = "../builtin/mod.rs"]
mod builtin;
mod compile;
mod loader;
mod model;
mod normalize;
mod validation;

pub(crate) use model::*;
pub(crate) use validation::EngineConfigValidator;

pub type ViewRef = String;

pub const ENGINE_PICKER: &str = "picker";
pub const ENGINE_FORM: &str = "form";
pub const ENGINE_CAPTURE: &str = "capture";
pub const ENGINE_EMBEDDED: &str = "embedded";

/// Immutable workflow configuration compiled once during startup.
///
/// Launch-specific data deliberately lives in `workflow::InvocationContext`.
#[derive(Debug, Clone)]
pub(crate) struct CompiledConfig {
    pub entrypoint: ViewRef,
    pub default_view: Option<ViewRef>,
    pub(crate) entrypoint_query: Option<Value>,
    pub(crate) suite_file: Option<PathBuf>,
    pub(crate) image_protocol: ImageProtocol,
    pub(crate) log_file: Option<PathBuf>,
    pub(crate) commands: CommandConfig,
    pub(crate) aliases: BTreeMap<String, ViewRef>,
    pub(crate) view_aliases: BTreeMap<ViewRef, String>,
    pub(crate) all_commands: BTreeMap<String, Command>,
    views: BTreeMap<ViewRef, View>,
    workflows: BTreeMap<String, WorkflowMetadata>,
    defaults: Defaults,
    workflow_roots: BTreeMap<String, PathBuf>,
    parameter_registry: Arc<ParameterRegistry>,
}

/// The compiled, Picker-only configuration used to build presentation definitions.
/// It contains no command, theme, or complete configuration owner and is safe to keep
/// in a mount-owned loader after preparation.
#[derive(Clone)]
pub(crate) struct PickerItemsView {
    pub(crate) alias: Option<String>,
    pub(crate) items: Option<toml::Value>,
}

#[derive(Clone)]
pub(crate) struct PickerItemsProjection {
    input: Value,
    view_ref: ViewRef,
    view: PickerItemsView,
    workflow_root: Option<PathBuf>,
}

impl PickerItemsProjection {
    pub(crate) fn from_config(
        config: &CompiledConfig,
        input: &Value,
        root_view_ref: &str,
    ) -> Result<Self> {
        let view = config
            .views
            .get(root_view_ref)
            .with_context(|| format!("view {:?} is not configured", root_view_ref))?;
        let workflow_root = config.workflow_root(root_view_ref).map(Path::to_path_buf);
        let items_view = PickerItemsView {
            alias: config.alias_for_view(root_view_ref).map(str::to_string),
            items: view.selected_items().cloned(),
        };
        Ok(Self {
            input: input.clone(),
            view_ref: root_view_ref.to_string(),
            view: items_view,
            workflow_root,
        })
    }

    pub(crate) fn target_view(&self) -> (&str, &PickerItemsView) {
        (&self.view_ref, &self.view)
    }

    pub(crate) fn input_value(&self) -> &Value {
        &self.input
    }

    pub(crate) fn workflow_root(&self) -> Option<&Path> {
        self.workflow_root.as_deref()
    }
}

impl CompiledConfig {
    pub(crate) fn workflows(&self) -> &BTreeMap<String, WorkflowMetadata> {
        &self.workflows
    }

    pub(crate) fn bind_invocation_parameters(
        &self,
        view_ref: &str,
        arguments: &[String],
    ) -> Result<ParameterState> {
        self.parameter_binding(view_ref)?.bind_cli(arguments)
    }

    pub(crate) fn instantiate_parameters(&self, view_ref: &str) -> Result<ParameterState> {
        self.parameter_binding(view_ref)?.instantiate()
    }

    pub(crate) fn parameter_values(&self, state: &ParameterState) -> Result<Value> {
        self.parameter_binding(state.view_ref())?
            .parameter_values(state)
    }

    pub(crate) fn validate_parameter_values(&self, view_ref: &str, value: &Value) -> Result<()> {
        let binding = self.parameter_binding(view_ref)?;
        let mut state = binding.instantiate()?;
        binding.update_sanitized_initial_value(&mut state, value)?;
        binding.validate_instance(&state)
    }

    pub(crate) fn parameter_binding(&self, view_ref: &str) -> Result<ParameterBinding> {
        self.parameter_registry.parameter_binding(view_ref)
    }

    pub(crate) fn query_definition(&self, view_ref: &str) -> Result<Value> {
        let view = self
            .view(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?;
        match &view.query {
            Some(query) => toml_to_json(&toml::Value::Table(query.clone())),
            None => Ok(serde_json::json!({"type": "string"})),
        }
    }

    #[cfg(test)]
    pub(crate) fn parameter_snapshot(
        &self,
        state: &ParameterState,
        source: crate::input::InputSourceIdentity,
    ) -> Result<ParameterSnapshot> {
        Ok(ParameterSnapshot::from_parts(
            self.parameter_binding(state.view_ref())?
                .parameter_values(state)?,
            state.raw_input().to_string(),
            source,
            state.revision(),
        ))
    }

    pub(crate) fn session_commands(&self) -> BTreeMap<String, Command> {
        let mut globals = self.commands.bindings.clone();
        let binding_uses_key = |binding: &CommandBinding, key: &str| {
            binding
                .key
                .as_deref()
                .and_then(|value| normalize_key(value).ok())
                .is_some_and(|value| value == key)
        };
        let commands_binding_is_available = !globals.contains_key("commands")
            && !globals
                .values()
                .any(|binding| binding_uses_key(binding, "ctrl+k"));
        if commands_binding_is_available && self.view("__commands:main").is_some() {
            globals.insert("commands".to_string(), CommandBinding::builtin_commands());
        }
        let parameters_binding_is_available = !globals.contains_key("parameters")
            && !globals
                .values()
                .any(|binding| binding_uses_key(binding, "ctrl+g"));
        if parameters_binding_is_available && self.view("__form:main").is_some() {
            globals.insert(
                "parameters".to_string(),
                CommandBinding::builtin_parameters(),
            );
        }
        globals
            .into_iter()
            .filter_map(|(id, binding)| {
                let cmd = binding.as_command(&id)?;
                Some((id, cmd))
            })
            .collect()
    }

    pub(crate) fn update_sanitized_initial_parameter_values(
        &self,
        state: &mut ParameterState,
        value: &Value,
    ) -> Result<bool> {
        self.parameter_binding(state.view_ref())?
            .update_sanitized_initial_value(state, value)
    }

    pub(crate) fn sanitize_initial_parameter_values(
        &self,
        state: &mut ParameterState,
    ) -> Result<bool> {
        self.parameter_binding(state.view_ref())?
            .sanitize_typed_values(state)
    }

    pub(crate) fn render_parameter_input(&self, state: &ParameterState) -> Result<String> {
        self.parameter_binding(state.view_ref())?
            .render_input(state)
    }

    pub(crate) fn update_initial_parameter_input(
        &self,
        state: &mut ParameterState,
        source: &str,
    ) -> Result<bool> {
        self.parameter_binding(state.view_ref())?
            .update_initial_input(state, source)
    }

    pub fn view(&self, view_ref: &str) -> Option<&View> {
        self.views.get(view_ref)
    }

    pub(crate) fn find_command(&self, current_workflow: &str, cmd_id: &str) -> Option<&Command> {
        if cmd_id.contains(':') {
            self.all_commands.get(cmd_id)
        } else {
            let fqid = format!("{current_workflow}:{cmd_id}");
            self.all_commands
                .get(&fqid)
                .or_else(|| self.all_commands.get(cmd_id))
        }
    }

    pub(crate) fn resolve_command_fqid(
        &self,
        current_workflow: &str,
        cmd_id: &str,
    ) -> Option<String> {
        if cmd_id.contains(':') {
            self.all_commands.contains_key(cmd_id).then(|| cmd_id.to_string())
        } else {
            let fqid = format!("{current_workflow}:{cmd_id}");
            if self.all_commands.contains_key(&fqid) {
                Some(fqid)
            } else if self.all_commands.contains_key(cmd_id) {
                Some(cmd_id.to_string())
            } else {
                None
            }
        }
    }

    pub(crate) fn workflow_commands(&self, workflow_id: &str) -> BTreeMap<String, Command> {
        let prefix = format!("{workflow_id}:");
        let mut map = BTreeMap::new();
        for (k, v) in &self.all_commands {
            if k.starts_with(&prefix) {
                map.insert(k.clone(), v.clone());
            }
        }
        map
    }

    pub(crate) fn iter_views(&self) -> impl Iterator<Item = (&ViewRef, &View)> {
        self.views.iter()
    }

    pub(crate) fn iter_public_views(&self) -> impl Iterator<Item = (&ViewRef, &View)> {
        self.views
            .iter()
            .filter(|(view_ref, _)| !view_ref.starts_with("__"))
    }

    pub(crate) fn view_count(&self) -> usize {
        self.views.len()
    }

    pub(crate) fn workflow_display_name(&self, package_id: &str) -> Option<&str> {
        self.workflows
            .get(package_id)
            .map(|workflow| workflow.name.as_str())
    }

    pub fn resolve_view(&self, selector: &str) -> Result<String> {
        if self.views.contains_key(selector) {
            return Ok(selector.to_string());
        }
        if let Some(target) = self.aliases.get(selector) {
            return Ok(target.clone());
        }
        if !selector.contains(':') {
            let matches: Vec<_> = self
                .views
                .keys()
                .filter(|k| k.split_once(':').is_some_and(|(_, v)| v == selector))
                .cloned()
                .collect();
            if matches.len() == 1 {
                return Ok(matches[0].clone());
            }
        }
        let mut available: Vec<&str> = self.aliases.keys().map(String::as_str).collect();
        available.extend(self.iter_public_views().map(|(r, _)| r.as_str()));
        available.sort();
        available.dedup();
        bail!(
            "unknown view {:?}; available views: {}",
            selector,
            available.join(", ")
        );
    }

    pub(crate) fn alias_for_view(&self, view_ref: &str) -> Option<&str> {
        self.view_aliases.get(view_ref).map(String::as_str)
    }

    pub(crate) fn aliases(&self) -> &BTreeMap<String, ViewRef> {
        &self.aliases
    }

    pub fn view_engine_type(&self, view_ref: &str) -> Result<&str> {
        let view = self
            .views
            .get(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?;
        Ok(view.selected_engine_type())
    }

    pub fn engine(&self, view_ref: &str) -> Result<&str> {
        self.view_engine_type(view_ref)
    }

    pub fn workflow_root(&self, view_ref: &str) -> Option<&Path> {
        let workflow = package_id(view_ref);
        self.workflow_roots.get(workflow).map(PathBuf::as_path)
    }

    pub(crate) fn picker_default_bindings(&self) -> Option<&toml::Value> {
        self.defaults.picker.bindings.as_ref()
    }

    pub(crate) fn capture_default_bindings(&self) -> Option<&toml::Value> {
        self.defaults.capture.bindings.as_ref()
    }
}

#[cfg(test)]
pub(crate) fn load_test_fixture() -> Result<CompiledConfig> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config/default.toml");
    let settings = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config/settings.toml");
    let loaded = CompiledConfig::load_suite_unvalidated(&path, Some(&settings))?;
    let config = loaded.compile()?;
    let engines = crate::engine::EngineRegistry::new();
    config.validate_with_engines(&engines)?;
    Ok(config)
}

pub(crate) fn package_id(view_ref: &str) -> &str {
    view_ref
        .split_once(':')
        .map(|(package, _)| package)
        .unwrap_or(view_ref)
}

pub fn normalize_key(key: &str) -> Result<String> {
    Key::parse_binding(key)?
        .binding_name()
        .with_context(|| format!("unsupported command key {:?}", key))
}

pub(crate) fn toml_to_json(value: &toml::Value) -> Result<Value> {
    serde_json::to_value(value).context("could not convert TOML to JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    fn config(source: &str) -> CompiledConfig {
        let mut value: toml::Value = toml::from_str(source).unwrap();
        if let toml::Value::Table(fields) = &mut value {
            fields
                .entry("workflows".to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        }
        let raw: RawConfig = value.clone().try_into().unwrap();
        CompiledConfig::from_raw(raw, BTreeMap::new()).unwrap()
    }

    #[test]
    fn image_protocol_and_log_file_are_compiled() {
        let compiled = config(
            r#"
            image_protocol = "kitty"
            log_file = "logs/runtime.jsonl"
            "#,
        );
        assert_eq!(compiled.image_protocol, ImageProtocol::Kitty);
        assert_eq!(compiled.log_file, Some(PathBuf::from("logs/runtime.jsonl")));
    }

    #[test]
    fn unknown_root_fields_are_rejected() {
        let value: toml::Value = toml::from_str("unsupported = true").unwrap();
        assert!(value.try_into::<RawConfig>().is_err());
    }

    #[test]
    fn declared_item_handler_preserves_data() {
        let source: toml::Value = toml::from_str(
            r#"
            [workflows.demo]
            [workflows.demo.views.main.engine]
            type = "picker"
            [workflows.demo.views.main.engine.config]
            items = { producer = "declared", handler = { items = [{ display = "Example item" }] } }
            "#,
        )
        .unwrap();
        let raw: RawConfig = source.clone().try_into().unwrap();
        let compiled = CompiledConfig::from_raw(raw, BTreeMap::new()).unwrap();
        let items = compiled
            .view("demo:main")
            .unwrap()
            .selected_items()
            .unwrap();
        assert_eq!(
            items["handler"]["items"][0]["display"],
            toml::Value::String("Example item".to_string())
        );
    }

    #[test]
    fn fixture_validates_against_registered_engines() {
        let compiled = load_test_fixture().unwrap();
        compiled
            .validate_with_engines(&EngineRegistry::new())
            .unwrap();
    }

    #[test]
    fn built_in_commands_require_the_commands_selector_view() {
        let compiled = config(
            r#"
            [commands.bindings.commands]
            key = "ctrl+k"
            "#,
        );
        let error = compiled
            .validate_with_engines(&EngineRegistry::new())
            .expect_err("built-in commands need their selector view");
        assert!(error.to_string().contains("__commands:main"));
    }

    #[test]
    fn built_in_parameters_yield_to_a_user_ctrl_g_binding() {
        let compiled = config(
            r#"
            [commands.bindings.custom]
            key = "ctrl+g"
            label = "Custom"
            type = "return"
            producer = "declared"
            handler = { value = "custom" }

            [workflows.__form.views.main.engine]
            type = "form"
            [workflows.__form.views.main.engine.config.content]
            producer = "declared"
            [workflows.__form.views.main.engine.config.content.handler]
            fields = []
            "#,
        );
        let commands = compiled.session_commands();
        assert!(!commands.contains_key("parameters"));
        assert_eq!(commands["custom"].key.as_deref(), Some("ctrl+g"));
    }

    #[test]
    fn parameter_form_replace_values_use_strict_target_schema_validation() {
        let compiled = config(
            r#"
            [workflows.dynamic.views.main]
            [workflows.dynamic.views.main.query]
            type = "object"
            required_name = { type = "string" }
            count = { type = "integer", default = 1 }
            [workflows.dynamic.views.main.engine]
            type = "picker"
            [workflows.dynamic.views.main.engine.config]
            items = []
            "#,
        );
        assert!(
            compiled
                .validate_parameter_values(
                    "dynamic:main",
                    &serde_json::json!({"required_name":"ok","count":2}),
                )
                .is_ok()
        );
        assert!(
            compiled
                .validate_parameter_values("dynamic:main", &serde_json::json!({"count":2}))
                .is_err()
        );
        assert!(
            compiled
                .validate_parameter_values(
                    "dynamic:main",
                    &serde_json::json!({"required_name":"ok","count":"2"}),
                )
                .is_err()
        );
        assert!(
            compiled
                .validate_parameter_values(
                    "dynamic:main",
                    &serde_json::json!({"required_name":"ok","extra":true}),
                )
                .is_err()
        );
    }

    #[test]
    fn key_normalization_accepts_named_bindings_and_rejects_empty_values() {
        assert_eq!(normalize_key("ctrl+k").unwrap(), "ctrl+k");
        assert!(normalize_key("").is_err());
    }
}
