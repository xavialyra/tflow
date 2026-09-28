use crate::input::Key;
#[cfg(test)]
use crate::workflow::parameter::ParameterSnapshot;
use crate::workflow::parameter::{ParameterBinding, ParameterRegistry, ParameterState};
use anyhow::{Context, Result, bail};
pub(crate) use loader::parse_atomic_workflow_package;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

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

/// The target of one binding address, after [`CompiledConfig::resolve_address`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedAddress {
    /// A declared workflow command, identified by its fully-qualified id.
    Command { fqid: String, label: String },
    /// A built-in engine action, identified as `<engine>.<action>`.
    Engine { fqid: String, label: &'static str },
}

impl ResolvedAddress {
    /// The `<workflow>.<command>` or `<engine>.<action>` spelling of the target.
    pub(crate) fn fqid(&self) -> &str {
        match self {
            Self::Command { fqid, .. } | Self::Engine { fqid, .. } => fqid,
        }
    }

    /// The address spelling valid in configuration and item bindings:
    /// `<workflow>.<command>` for workflow commands, `@engine:<engine>.<action>`
    /// for built-in engine actions.
    pub(crate) fn binding_address(&self) -> String {
        match self {
            Self::Command { fqid, .. } => fqid.clone(),
            Self::Engine { fqid, .. } => format!("@engine:{fqid}"),
        }
    }
}

/// Immutable workflow configuration compiled once during startup.
///
/// Launch-specific data deliberately lives in `workflow::InvocationContext`.
#[derive(Debug, Clone)]
pub(crate) struct CompiledConfig {
    pub entrypoint: ViewRef,
    pub(crate) entrypoint_query: Option<Value>,
    pub(crate) suite_file: Option<PathBuf>,
    pub(crate) image_protocol: ImageProtocol,
    pub(crate) log_file: Option<PathBuf>,
    pub(crate) chrome_commands_show: Vec<String>,
    pub(crate) host_bindings: BTreeMap<String, String>,
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

    pub(crate) fn bind_invocation_parameters_with_seed(
        &self,
        view_ref: &str,
        seed: Option<&Value>,
        arguments: &[String],
    ) -> Result<ParameterState> {
        let binding = self.parameter_binding(view_ref)?;
        let mut state = binding.instantiate()?;
        if let Some(seed_value) = seed {
            binding.update_sanitized_initial_value(&mut state, seed_value)?;
        }
        binding.apply_cli(&mut state, arguments)?;
        Ok(state)
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

    pub(crate) fn chrome_commands_show(&self, view_ref: &str) -> Result<Vec<String>> {
        let raw = self
            .view(view_ref)
            .and_then(|view| view.chrome_commands_show.as_ref());
        let bindings = raw
            .cloned()
            .unwrap_or_else(|| self.chrome_commands_show.clone());
        let mut seen = std::collections::HashSet::new();
        let mut normalized = Vec::with_capacity(bindings.len());
        for binding in bindings {
            let key = Key::canonical_binding_name(&binding)
                .with_context(|| format!("invalid chrome command binding {:?}", binding))?;
            anyhow::ensure!(
                seen.insert(key.clone()),
                "duplicate chrome command binding {:?}",
                key
            );
            normalized.push(key);
        }
        Ok(normalized)
    }

    /// Host-layer bindings as `canonical physical key -> command FQID`.
    pub(crate) fn host_bindings(&self) -> &BTreeMap<String, String> {
        &self.host_bindings
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
        if cmd_id.contains('.') {
            self.all_commands.get(cmd_id)
        } else {
            self.all_commands
                .get(&format!("{current_workflow}.{cmd_id}"))
        }
    }

    pub(crate) fn resolve_command_fqid(
        &self,
        current_workflow: &str,
        cmd_id: &str,
    ) -> Option<String> {
        if cmd_id.contains('.') {
            self.all_commands
                .contains_key(cmd_id)
                .then(|| cmd_id.to_string())
        } else {
            let fqid = format!("{current_workflow}.{cmd_id}");
            self.all_commands.contains_key(&fqid).then_some(fqid)
        }
    }

    /// Resolves one binding address to its target.
    ///
    /// Both a View's `[views.<name>.bindings]` values and its
    /// `[views.<name>.unbind] commands` use this one grammar: a bare name or an
    /// FQID (optionally prefixed with `@workflow:`) names a command of
    /// `current_workflow`, while only `@engine:<engine>.<action>` names a
    /// built-in engine action. There is no fallback between the two.
    pub(crate) fn resolve_address(
        &self,
        current_workflow: &str,
        address: &str,
    ) -> Option<ResolvedAddress> {
        if let Some((fqid, label)) = crate::engine::engine_action_from_address(address) {
            return Some(ResolvedAddress::Engine { fqid, label });
        }
        let address = address.strip_prefix("@workflow:").unwrap_or(address);
        let fqid = self.resolve_command_fqid(current_workflow, address)?;
        let label = self.all_commands.get(&fqid)?.label.clone();
        Some(ResolvedAddress::Command { fqid, label })
    }

    pub(crate) fn workflow_commands(&self, workflow_id: &str) -> BTreeMap<String, Command> {
        let prefix = format!("{workflow_id}.");
        let mut map = BTreeMap::new();
        for (k, v) in &self.all_commands {
            if k.starts_with(&prefix) {
                map.insert(k.clone(), v.clone());
            }
        }
        map
    }

    pub(crate) fn iter_public_views(&self) -> impl Iterator<Item = (&ViewRef, &View)> {
        self.views
            .iter()
            .filter(|(view_ref, _)| !view_ref.starts_with("__"))
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

    /// Resolve a view selector in the context of a caller view.
    ///
    /// When `selector` is an explicit local reference (`self:<view>` or `:<view>`)
    /// or a bare local view name (no colon), it prioritizes views declared within
    /// the caller's workflow package. If not found in the local workflow, bare
    /// names fall back to global resolution (aliases or unique view matches).
    pub fn resolve_view_scoped(&self, selector: &str, caller_view: &str) -> Result<String> {
        let caller_pkg = package_id(caller_view);

        let local_candidate = selector
            .strip_prefix("self:")
            .or_else(|| selector.strip_prefix(':'));
        if let Some(local_view) = local_candidate {
            if !caller_pkg.is_empty() {
                let candidate = format!("{caller_pkg}:{local_view}");
                if self.views.contains_key(&candidate) {
                    return Ok(candidate);
                }
            }
            if self.views.contains_key(local_view) {
                return Ok(local_view.to_string());
            }
            bail!(
                "workflow-local view {:?} not found in workflow {:?}",
                local_view,
                caller_pkg
            );
        }

        // Bare local view name without colon: prioritize caller's workflow
        if !selector.contains(':') && !caller_pkg.is_empty() {
            let candidate = format!("{caller_pkg}:{selector}");
            if self.views.contains_key(&candidate) {
                return Ok(candidate);
            }
        }

        self.resolve_view(selector)
    }

    pub(crate) fn alias_for_view(&self, view_ref: &str) -> Option<&str> {
        self.view_aliases.get(view_ref).map(String::as_str)
    }

    /// Suite alias for a user-facing View. Internal `__`-prefixed Views are not
    /// navigation targets and never advertise one.
    pub(crate) fn public_alias_for_view(&self, view_ref: &str) -> Option<&str> {
        (!view_ref.starts_with("__"))
            .then(|| self.alias_for_view(view_ref))
            .flatten()
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

    /// Left-side marker for non-root Picker input lines. `"$route"` is a
    /// sentinel resolved to the target View's route label by the View factory;
    /// any other value is a literal marker, and `None` disables it.
    pub(crate) fn picker_left_prefix(&self) -> Option<&str> {
        self.defaults.picker.left_prefix.as_deref()
    }

    /// Backspace behavior on an empty, prefixed Picker input line. `None`
    /// leaves Backspace inert.
    pub(crate) fn picker_left_prefix_backspace(&self) -> Option<LeftPrefixBackspace> {
        self.defaults.picker.left_prefix_backspace
    }

    pub(crate) fn capture_default_bindings(&self) -> Option<&toml::Value> {
        self.defaults.capture.bindings.as_ref()
    }

    pub(crate) fn embedded_default_bindings(&self) -> Option<&toml::Value> {
        self.defaults.embedded.bindings.as_ref()
    }

    pub(crate) fn form_default_bindings(&self) -> Option<&toml::Value> {
        self.defaults.form.bindings.as_ref()
    }
}

#[cfg(test)]
pub(crate) fn load_test_fixture() -> Result<CompiledConfig> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config/default.toml");
    let settings =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config/settings.toml");
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

pub(crate) fn toml_to_json(value: &toml::Value) -> Result<Value> {
    serde_json::to_value(value).context("could not convert TOML to JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    #[test]
    fn resolve_view_accepts_aliases_and_canonical_references() {
        let config = load_test_fixture().unwrap();
        assert_eq!(config.resolve_view("sys").unwrap(), "sys:main");
        assert_eq!(config.resolve_view("sys:main").unwrap(), "sys:main");
        assert!(config.resolve_view("unknown").is_err());
        assert!(config.resolve_view("sys:missing").is_err());
    }

    #[test]
    fn resolve_view_scoped_resolves_local_views_prioritizing_caller_workflow() {
        let config = load_test_fixture().unwrap();

        // 1. Bare local view name under sys and apps workflows
        assert_eq!(
            config.resolve_view_scoped("output", "sys:main").unwrap(),
            "sys:output"
        );
        assert_eq!(
            config.resolve_view_scoped("weight", "apps:main").unwrap(),
            "apps:weight"
        );

        // 2. Both sys and apps have a "main" view: resolving "main" under sys resolves to sys:main,
        // while resolving "main" under apps resolves to apps:main.
        assert_eq!(
            config.resolve_view_scoped("main", "sys:output").unwrap(),
            "sys:main"
        );
        assert_eq!(
            config.resolve_view_scoped("main", "apps:weight").unwrap(),
            "apps:main"
        );

        // 3. Explicit self: or : syntax
        assert_eq!(
            config
                .resolve_view_scoped("self:output", "sys:main")
                .unwrap(),
            "sys:output"
        );
        assert_eq!(
            config.resolve_view_scoped(":output", "sys:main").unwrap(),
            "sys:output"
        );
        assert_eq!(
            config
                .resolve_view_scoped("self:weight", "apps:main")
                .unwrap(),
            "apps:weight"
        );
        assert_eq!(
            config.resolve_view_scoped(":weight", "apps:main").unwrap(),
            "apps:weight"
        );

        // 4. Missing local view fails
        assert!(
            config
                .resolve_view_scoped("self:nonexistent", "sys:main")
                .is_err()
        );
        assert!(
            config
                .resolve_view_scoped(":nonexistent", "sys:main")
                .is_err()
        );

        // 5. Fallback to external alias or canonical reference when not found in caller workflow
        assert_eq!(
            config.resolve_view_scoped("app", "sys:main").unwrap(),
            "apps:main"
        );
        assert_eq!(
            config
                .resolve_view_scoped("apps:weight", "sys:main")
                .unwrap(),
            "apps:weight"
        );
    }

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
    fn view_bindings_accept_all_command_address_forms() {
        // `xxx.command`, `command`, and `@workflow:xxx.command` all resolve to
        // the same workflow command; anything else is rejected.
        for bind in [
            "\"escape\" = \"core.other\"",
            "\"escape\" = \"other\"",
            "\"escape\" = \"@workflow:core.other\"",
        ] {
            let compiled = config(&format!(
                r#"
                [workflows.core.views.default]
                [workflows.core.views.default.engine]
                type = "picker"
                [workflows.core.views.default.engine.config]
                items = []
                [workflows.core.views.default.bindings]
                {bind}
                [workflows.core.commands.other]
                label = "Other"
                type = "run"
                producer = "declared"
                [workflows.core.commands.other.handler]
                argv = ["true"]
            "#
            ));
            compiled
                .validate_with_engines(&EngineRegistry::new())
                .unwrap_or_else(|error| panic!("{bind} should resolve: {error}"));
        }

        let bad = config(
            r#"
            [workflows.core.views.default]
            [workflows.core.views.default.engine]
            type = "picker"
            [workflows.core.views.default.engine.config]
            items = []
            [workflows.core.views.default.bindings]
            "escape" = "@workflow:missing"
        "#,
        );
        let error = bad
            .validate_with_engines(&EngineRegistry::new())
            .expect_err("@workflow:missing is not a command");
        assert!(error.to_string().contains("unknown command"), "{error}");
    }

    #[test]
    fn view_unbind_table_validates_each_axis() {
        let ok = config(
            r#"
            [workflows.core.views.default]
            [workflows.core.views.default.engine]
            type = "picker"
            [workflows.core.views.default.engine.config]
            items = []
            [workflows.core.views.default.unbind]
            keys = ["ctrl+u"]
            commands = ["@engine:picker.clear_input", "core.other"]
            layers = ["host"]
            [workflows.core.commands.other]
            label = "Other"
            type = "run"
            producer = "declared"
            [workflows.core.commands.other.handler]
            argv = ["true"]
        "#,
        );
        ok.validate_with_engines(&EngineRegistry::new()).unwrap();

        for (label, unbind) in [
            ("unknown command", "commands = [\"core.missing\"]"),
            ("unknown layer", "layers = [\"nowhere\"]"),
            ("invalid key", "keys = [\"not a key\"]"),
            // Engine actions are engine-specific: this View runs the Picker, so
            // a Capture action is not addressable here.
            (
                "foreign engine action",
                "commands = [\"@engine:capture.copy\"]",
            ),
        ] {
            let bad = config(&format!(
                r#"
                [workflows.core.views.default]
                [workflows.core.views.default.engine]
                type = "picker"
                [workflows.core.views.default.engine.config]
                items = []
                [workflows.core.views.default.unbind]
                {unbind}
            "#
            ));
            assert!(
                bad.validate_with_engines(&EngineRegistry::new()).is_err(),
                "{label} must be rejected"
            );
        }
    }

    /// A View binding must name a command. Claiming a key with no command was a
    /// boolean tombstone; the surviving way to take a key away from a lower
    /// layer is `[views.<name>.unbind] keys`.
    #[test]
    fn a_view_binding_cannot_be_a_boolean() {
        for value in ["false", "true"] {
            let bad = config(&format!(
                r#"
                [workflows.core.views.default]
                [workflows.core.views.default.bindings]
                "escape" = {value}
                [workflows.core.views.default.engine]
                type = "picker"
                [workflows.core.views.default.engine.config]
                items = []
            "#
            ));
            let error = bad
                .validate_with_engines(&EngineRegistry::new())
                .expect_err("a boolean binding names no command");
            assert!(
                error.to_string().contains("unbind") && error.to_string().contains("\"escape\""),
                "the error should point at unbind.keys: {error}"
            );
        }
    }

    /// Every command FQID is `<owner>.<name>`: an engine action owns
    /// `<engine>.<action>`, a workflow command owns `<workflow>.<command>`. The
    /// index is unique exactly when no workflow command lands on an engine
    /// action's id, so that one FQID always names exactly one command.
    #[test]
    fn a_workflow_command_cannot_take_an_engine_actions_id() {
        for (engine, action) in [
            (ENGINE_PICKER, "exit"),
            (ENGINE_FORM, "exit"),
            (ENGINE_FORM, "focus_next"),
            (ENGINE_CAPTURE, "copy"),
            (ENGINE_EMBEDDED, "cancel"),
        ] {
            let bad = config(&format!(
                r#"
                [workflows.{engine}.views.main]
                [workflows.{engine}.views.main.engine]
                type = "picker"
                [workflows.{engine}.views.main.engine.config]
                items = []
                [workflows.{engine}.commands.{action}]
                label = "Collides"
                type = "run"
                producer = "declared"
                [workflows.{engine}.commands.{action}.handler]
                argv = ["true"]
            "#
            ));
            let error = bad
                .validate_with_engines(&EngineRegistry::new())
                .expect_err("an engine action already owns that id");
            assert!(
                error.to_string().contains("collides with the"),
                "{engine}.{action}: {error}"
            );
        }

        // Sharing the engine's *name* is fine: only the action ids are taken, so
        // a workflow called `form` may still declare `open` (the test fixture
        // does exactly this).
        let ok = config(
            r#"
            [workflows.form.views.main]
            [workflows.form.views.main.engine]
            type = "picker"
            [workflows.form.views.main.engine.config]
            items = []
            [workflows.form.commands.open]
            label = "Open"
            type = "run"
            producer = "declared"
            [workflows.form.commands.open.handler]
            argv = ["true"]
        "#,
        );
        ok.validate_with_engines(&EngineRegistry::new()).unwrap();
    }

    #[test]
    fn image_protocol_defaults_to_auto_and_accepts_explicit_fallback() {
        assert_eq!(config("").image_protocol, ImageProtocol::Auto);
        assert_eq!(
            config("image_protocol = 'auto'").image_protocol,
            ImageProtocol::Auto
        );
        assert_eq!(
            config("image_protocol = 'halfblocks'").image_protocol,
            ImageProtocol::Halfblocks
        );
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
    fn picker_left_prefix_defaults_off_and_is_configurable() {
        let defaults: PickerDefaults = toml::from_str("").unwrap();
        assert!(defaults.left_prefix.is_none());
        assert!(defaults.left_prefix_backspace.is_none());
        let defaults: PickerDefaults = toml::from_str("left_prefix = \"$route\"").unwrap();
        assert_eq!(defaults.left_prefix.as_deref(), Some("$route"));
        let defaults: PickerDefaults = toml::from_str("left_prefix = \"\u{3008}\"").unwrap();
        assert_eq!(defaults.left_prefix.as_deref(), Some("\u{3008}"));
    }

    #[test]
    fn picker_left_prefix_backspace_is_an_opt_in_enum() {
        let parent: PickerDefaults = toml::from_str("left_prefix_backspace = \"parent\"").unwrap();
        assert_eq!(
            parent.left_prefix_backspace,
            Some(LeftPrefixBackspace::Parent)
        );
        let root: PickerDefaults = toml::from_str("left_prefix_backspace = \"root\"").unwrap();
        assert_eq!(root.left_prefix_backspace, Some(LeftPrefixBackspace::Root));
        assert!(toml::from_str::<PickerDefaults>("left_prefix_backspace = \"none\"").is_err());
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
    fn item_merge_views_may_declare_a_base_bindings() {
        let compiled = config(
            r#"
            [workflows.core.commands.complete]
            label = "Complete"
            type = "navigate"
            producer = "declared"
            handler = { target = "core:default" }

            [workflows.core.views.default]
            binding_mode = "item_merge"

            [workflows.core.views.default.engine]
            type = "picker"

            [workflows.core.views.default.bindings]
            tab = "complete"
            "#,
        );
        compiled
            .validate_with_engines(&EngineRegistry::new())
            .unwrap();
        let view = compiled.view("core:default").expect("view");
        assert_eq!(view.binding_mode, BindingMode::ItemMerge);
        assert!(
            view.bindings
                .as_ref()
                .expect("bindings")
                .contains_key("tab")
        );
    }

    #[test]
    fn item_merge_base_bindings_still_validates_its_targets() {
        let compiled = config(
            r#"
            [workflows.core.views.default]
            binding_mode = "item_merge"

            [workflows.core.views.default.engine]
            type = "picker"

            [workflows.core.views.default.bindings]
            tab = "missing"
            "#,
        );
        let error = compiled
            .validate_with_engines(&EngineRegistry::new())
            .expect_err("an unknown command must be rejected in either mode");
        assert!(error.to_string().contains("unknown command"), "{error}");
    }

    #[test]
    fn fixture_validates_against_registered_engines() {
        let compiled = load_test_fixture().unwrap();
        compiled
            .validate_with_engines(&EngineRegistry::new())
            .unwrap();
    }

    #[test]
    fn explicit_host_bindings_are_references_into_the_definition_map() {
        let compiled = config(
            r#"
            [host_bindings]
            "ctrl+g" = "core.custom"

            [workflows.core.commands.custom]
            label = "Custom"
            type = "return"
            producer = "declared"
            handler = { value = "custom" }

            [workflows.core.views.main.engine]
            type = "picker"
            "#,
        );
        compiled
            .validate_with_engines(&EngineRegistry::new())
            .unwrap();
        assert_eq!(
            compiled.host_bindings(),
            &BTreeMap::from([("ctrl+g".to_string(), "core.custom".to_string())])
        );
        // The definition itself stays in the single command map.
        assert!(compiled.all_commands.contains_key("core.custom"));
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
        assert_eq!(Key::canonical_binding_name("ctrl+k").unwrap(), "ctrl+k");
        assert!(Key::canonical_binding_name("").is_err());
    }

    #[test]
    fn builtin_modal_views_show_only_enter_and_unbind_host_layer() {
        for (label, workflow) in [
            (
                "__commands",
                crate::workflow::builtin::builtin_commands_workflow()
                    .expect("__commands must parse"),
            ),
            (
                "__parameters",
                crate::workflow::builtin::builtin_parameters_workflow()
                    .expect("__parameters must parse"),
            ),
        ] {
            let view = workflow.views.get("main").expect("main view");
            assert_eq!(
                view.chrome_commands_show.as_deref(),
                Some(["enter".to_string()].as_slice()),
                "{label} must not advertise unbound host shortcuts in the footer"
            );
            assert_eq!(
                view.unbind.layers,
                vec!["host".to_string()],
                "{label} must isolate the host layer to prevent re-entrancy"
            );
            assert!(view.unbind.keys.is_empty());
            assert!(view.unbind.commands.is_empty());
        }
    }
}
