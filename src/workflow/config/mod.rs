use crate::input::Key;
#[cfg(test)]
use crate::workflow::parameter::ParameterSnapshot;
use crate::workflow::parameter::{ParameterBinding, ParameterRegistry, ParameterState};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
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
    pub default_view: Option<ViewRef>,
    pub(crate) image_protocol: ImageProtocol,
    pub(crate) log_file: Option<PathBuf>,
    pub(crate) commands: CommandConfig,
    views: BTreeMap<ViewRef, View>,
    workflows: BTreeMap<String, WorkflowMetadata>,
    defaults: Defaults,
    workflow_roots: BTreeMap<String, PathBuf>,
    parameter_registry: Arc<ParameterRegistry>,
}

/// The compiled, Picker-only configuration used to build feed definitions.
/// It contains no command, theme, or complete configuration owner and is safe to keep
/// in a mount-owned loader after preparation.
#[derive(Clone)]
pub(crate) struct PickerItemsView {
    pub(crate) alias: Option<String>,
    pub(crate) feeds: Vec<ViewRef>,
    pub(crate) items: Option<toml::Value>,
    pub(crate) binding: ParameterBinding,
    pub(crate) source_badge: bool,
}

#[derive(Clone)]
pub(crate) struct PickerItemsProjection {
    input: Value,
    views: BTreeMap<ViewRef, PickerItemsView>,
    workflow_roots: BTreeMap<String, PathBuf>,
}

impl PickerItemsProjection {
    pub(crate) fn from_config(
        config: &CompiledConfig,
        input: &Value,
        root_view_ref: &str,
    ) -> Result<Self> {
        let mut selected = BTreeSet::from([root_view_ref.to_string()]);
        let mut pending = vec![root_view_ref.to_string()];
        while let Some(view_ref) = pending.pop() {
            let view = config
                .views
                .get(&view_ref)
                .with_context(|| format!("view {:?} is not configured", view_ref))?;
            for feed in view.selected_feeds() {
                if selected.insert(feed.view.clone()) {
                    pending.push(feed.view.clone());
                }
            }
        }

        let mut views = BTreeMap::new();
        let mut workflow_roots = BTreeMap::new();
        for view_ref in selected {
            let view = config
                .views
                .get(&view_ref)
                .with_context(|| format!("view {:?} is not configured", view_ref))?;
            if let Some(root) = config.workflow_root(&view_ref) {
                workflow_roots.insert(package_id(&view_ref).to_string(), root.to_path_buf());
            }
            let source_badge = match view.engine_field("source_badge") {
                Some(toml::Value::Boolean(enabled)) => *enabled,
                _ => !view.selected_feeds().is_empty(),
            };
            views.insert(
                view_ref.clone(),
                PickerItemsView {
                    alias: view.alias.clone(),
                    feeds: view
                        .selected_feeds()
                        .iter()
                        .map(|feed| feed.view.clone())
                        .collect(),
                    items: view.selected_items().cloned(),
                    binding: config.parameter_registry.parameter_binding(&view_ref)?,
                    source_badge,
                },
            );
        }
        Ok(Self {
            input: input.clone(),
            views,
            workflow_roots,
        })
    }

    pub(crate) fn feed_views(&self, view_ref: &str) -> Result<Vec<(String, &PickerItemsView)>> {
        let view = self
            .views
            .get(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?;
        if view.feeds.is_empty() {
            return Ok(vec![(view_ref.to_string(), view)]);
        }
        view.feeds
            .iter()
            .map(|feed| {
                self.views
                    .get(feed)
                    .map(|source| (feed.clone(), source))
                    .with_context(|| {
                        format!("view {:?} references missing feed {:?}", view_ref, feed)
                    })
            })
            .collect()
    }

    pub(crate) fn input_value(&self) -> &Value {
        &self.input
    }

    pub(crate) fn source_badge(&self, view_ref: &str) -> bool {
        self.views
            .get(view_ref)
            .is_some_and(|view| view.source_badge)
    }

    pub(crate) fn workflow_root(&self, view_ref: &str) -> Option<&Path> {
        let package = package_id(view_ref);
        self.workflow_roots.get(package).map(PathBuf::as_path)
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

    pub(crate) fn session_command(&self, id: &str) -> Option<Command> {
        self.session_commands().get(id).cloned()
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
        if commands_binding_is_available && self.view("__selectors:commands").is_some() {
            globals.insert("commands".to_string(), CommandBinding::builtin_commands());
        }
        let parameters_binding_is_available = !globals.contains_key("parameters")
            && !globals
                .values()
                .any(|binding| binding_uses_key(binding, "ctrl+g"));
        if parameters_binding_is_available && self.view("__selectors:form").is_some() {
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
        for (view_ref, view) in &self.views {
            if view.alias.as_deref() == Some(selector) {
                return Ok(view_ref.clone());
            }
        }
        bail!("unknown view {:?}", selector);
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

    pub fn feed_views<'a>(&'a self, view_ref: &str) -> Result<Vec<(String, &'a View)>> {
        let view = self
            .views
            .get(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?;
        let feeds = view.selected_feeds();
        if feeds.is_empty() {
            return Ok(vec![(view_ref.to_string(), view)]);
        }
        feeds
            .iter()
            .map(|feed| {
                self.views
                    .get(&feed.view)
                    .map(|source| (feed.view.clone(), source))
                    .with_context(|| {
                        format!(
                            "view {:?} references missing feed {:?}",
                            view_ref, feed.view
                        )
                    })
            })
            .collect()
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
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config/config.toml");
    CompiledConfig::load(&path)
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
        assert!(error.to_string().contains("__selectors:commands"));
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

            [workflows.__selectors.views.form.engine]
            type = "form"
            [workflows.__selectors.views.form.engine.config.content]
            producer = "declared"
            [workflows.__selectors.views.form.engine.config.content.handler]
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
