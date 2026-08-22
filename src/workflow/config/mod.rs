use crate::expression::TemplateRegistry;
#[cfg(test)]
use crate::expression::{Budget, EvalContext, EvaluationStage, Namespace, evaluate_json_value};
use crate::input::Key;
use crate::state::{StateInstance, StateRegistry};
#[cfg(test)]
use crate::theme::ThemeLoadOptions;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

mod compile;
mod evaluation;
mod loader;
mod model;
mod normalize;
mod validation;

#[cfg(test)]
use evaluation::public_page_context;
pub(crate) use evaluation::{
    ConfigSource, EvaluationSnapshot, InvocationScope, OwnerViewScope, ReturnScope, SessionScope,
};

#[cfg(test)]
use loader::{merge_values, resolve_config_path, validate_plugin_id};
pub(crate) use model::validate_script_source_args;
pub(crate) use model::*;
#[cfg(test)]
use normalize::normalize_keymap_tables;
pub(crate) use validation::{EngineConfigValidator, validate_templates};

pub type ViewRef = String;

pub const ENGINE_PICKER: &str = "picker";
pub const ENGINE_CAPTURE: &str = "capture";
pub const ENGINE_EMBEDDED: &str = "embedded";

#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub default_view: Option<ViewRef>,
    pub(crate) image_protocol: ImageProtocol,
    pub(crate) log_file: Option<PathBuf>,
    pub(crate) commands: CommandConfig,
    pub(crate) input_value: Value,
    pub(crate) invocation_state: StateInstance,
    compiled: CompiledConfig,
}

#[derive(Debug, Clone)]
struct CompiledConfig {
    views: BTreeMap<ViewRef, View>,
    plugins: BTreeMap<String, PluginMetadata>,
    defaults: Defaults,
    plugin_roots: BTreeMap<String, PathBuf>,
    config_value: Value,
    template_registry: TemplateRegistry,
    state_registry: StateRegistry,
}

impl Config {
    pub(crate) fn bind_invocation_state(
        &self,
        view_ref: &str,
        arguments: &[String],
    ) -> Result<StateInstance> {
        self.compiled.state_registry.bind_cli(view_ref, arguments)
    }

    pub(crate) fn set_invocation(&mut self, input: Value, state: StateInstance) {
        self.input_value = input;
        self.invocation_state = state;
    }

    pub(crate) fn instantiate_state(&self, view_ref: &str) -> Result<StateInstance> {
        self.compiled.state_registry.instantiate(view_ref)
    }

    pub(crate) fn query_value(&self, state: &StateInstance) -> Result<Value> {
        self.compiled.state_registry.query_value(state)
    }

    pub(crate) fn update_query_value(
        &self,
        state: &mut StateInstance,
        value: &Value,
    ) -> Result<bool> {
        self.compiled.state_registry.update_value(state, value)
    }

    pub(crate) fn render_query_input(&self, state: &StateInstance) -> Result<String> {
        self.compiled.state_registry.render_input(state)
    }

    pub(crate) fn validate_query_state(&self, state: &StateInstance) -> Result<()> {
        self.compiled.state_registry.validate_instance(state)
    }

    pub(crate) fn update_query_input(
        &self,
        state: &mut StateInstance,
        source: &str,
    ) -> Result<bool> {
        self.compiled.state_registry.update_input(state, source)
    }

    #[cfg(test)]
    pub(crate) fn test_views_mut(&mut self) -> &mut BTreeMap<ViewRef, View> {
        &mut self.compiled.views
    }

    #[cfg(test)]
    pub(crate) fn test_config_value_mut(&mut self) -> &mut Value {
        &mut self.compiled.config_value
    }

    #[cfg(test)]
    pub(crate) fn test_rebuild_state_registry(&mut self) -> Result<()> {
        self.compiled.state_registry = StateRegistry::compile(&self.compiled.config_value)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn test_plugin_roots_mut(&mut self) -> &mut BTreeMap<String, PathBuf> {
        &mut self.compiled.plugin_roots
    }

    pub fn view(&self, view_ref: &str) -> Option<&View> {
        self.compiled.views.get(view_ref)
    }

    pub(crate) fn iter_views(&self) -> impl Iterator<Item = (&ViewRef, &View)> {
        self.compiled.views.iter()
    }

    pub(crate) fn view_count(&self) -> usize {
        self.compiled.views.len()
    }

    pub(crate) fn plugin_display_name(&self, package_id: &str) -> Option<&str> {
        self.compiled
            .plugins
            .get(package_id)
            .map(|plugin| plugin.name.as_str())
    }

    pub(crate) fn resolve_view(&self, selector: &str) -> Result<ViewRef> {
        if self.compiled.views.contains_key(selector) {
            return Ok(selector.to_string());
        }
        let matches = self
            .compiled
            .views
            .iter()
            .filter_map(|(view_ref, view)| {
                (view.alias.as_deref() == Some(selector)).then_some(view_ref.clone())
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [view_ref] => Ok(view_ref.clone()),
            [] => bail!("view {:?} is not configured", selector),
            _ => bail!(
                "view alias {:?} is ambiguous: {}",
                selector,
                matches.join(", ")
            ),
        }
    }

    pub fn engine(&self, view_ref: &str) -> Result<&str> {
        let view = self
            .compiled
            .views
            .get(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?;
        Ok(view.selected_engine_type())
    }

    pub fn plugin_root(&self, view_ref: &str) -> Option<&Path> {
        let plugin = package_id(view_ref);
        self.compiled.plugin_roots.get(plugin).map(PathBuf::as_path)
    }

    pub fn feed_views<'a>(&'a self, view_ref: &str) -> Result<Vec<(String, &'a View)>> {
        let view = self
            .compiled
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
                self.compiled
                    .views
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

    pub(crate) fn view_value(
        &self,
        state: &StateInstance,
        binding_raw: Option<&str>,
    ) -> Result<Value> {
        // Feed default binding must expose the coordinating page's committed
        // raw string (including ""). Rendering the owner state would replace
        // empty input with schema defaults and break view.raw_input == R.
        let input = match binding_raw {
            Some(raw) => raw.to_string(),
            None => self.render_query_input(state)?,
        };
        Ok(serde_json::json!({
            "ref": state.view_ref(),
            "query": self.query_value(state)?,
            "input": input,
            "raw_input": input,
            "state_revision": state.revision(),
        }))
    }

    pub(crate) fn ephemeral_feed_state(
        &self,
        feed_ref: &str,
        query_input: &str,
    ) -> Result<StateInstance> {
        let mut state = self.instantiate_state(feed_ref)?;
        self.compiled
            .state_registry
            .bind_feed_input(&mut state, query_input)?;
        self.validate_query_state(&state)?;
        Ok(state)
    }
}

#[cfg(test)]
pub(crate) fn load_test_fixture() -> Result<Config> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config/config.toml");
    Config::load(&path)
}

fn package_id(view_ref: &str) -> &str {
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
    use ratatui::style::Color;
    use std::{env, fs};

    fn config(source: &str) -> Config {
        let value: toml::Value = toml::from_str(source).unwrap();
        let raw: RawConfig = value.try_into().unwrap();
        Config::from_raw(raw, BTreeMap::new(), Value::Object(serde_json::Map::new())).unwrap()
    }

    #[test]
    fn image_protocol_is_loaded_from_the_root_config() {
        let configured = config(
            r#"
            image_protocol = "kitty"
            "#,
        );
        assert_eq!(configured.image_protocol, ImageProtocol::Kitty);

        let default_config = config("");
        assert_eq!(default_config.image_protocol, ImageProtocol::Halfblocks);
    }

    #[test]
    fn image_protocol_rejects_unknown_values() {
        let value: toml::Value = toml::from_str("image_protocol = \"auto\"").unwrap();
        assert!(value.try_into::<RawConfig>().is_err());
    }

    #[test]
    fn explicit_log_file_is_loaded() {
        let config = config("log_file = \"logs/runtime.jsonl\"");
        assert_eq!(config.log_file, Some(PathBuf::from("logs/runtime.jsonl")));
        assert_eq!(
            resolve_config_path(
                Path::new("/tmp/config/config.toml"),
                Path::new("logs/runtime.jsonl")
            ),
            PathBuf::from("/tmp/config/logs/runtime.jsonl")
        );
    }

    #[test]
    fn defaults_reject_unknown_fields() {
        for source in [
            r#"
            [defaults.capture]
            binding = { copy = ["enter"] }
            "#,
            r#"
            [defaults.captuer.bindings]
            copy = ["enter"]
            "#,
        ] {
            let value: toml::Value = toml::from_str(source).unwrap();
            assert!(
                value.try_into::<RawConfig>().is_err(),
                "unknown defaults fields must be rejected: {source}"
            );
        }
    }

    #[test]
    fn root_plugins_are_rejected_before_keymap_normalization() {
        let root =
            env::temp_dir().join(format!("tui-launcher-root-plugins-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        let config_path = root.join("config.toml");
        fs::write(
            &config_path,
            r#"
            disabled_plugins = ["ghost"]
            default_view = "core:default"

            [plugins.ghost.views.main.keymap]
            escape = "back"
            esc = false
            "#,
        )
        .unwrap();

        let error = Config::load(&config_path).expect_err("root plugins must be rejected");
        assert!(error.to_string().contains("cannot define plugins"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn disabled_directory_plugins_are_skipped_before_reading_keymaps() {
        let root = env::temp_dir().join(format!(
            "tui-launcher-disabled-directory-{}",
            std::process::id()
        ));
        fs::remove_dir_all(&root).ok();
        let plugin_root = root.join("plugins/ghost");
        fs::create_dir_all(&plugin_root).unwrap();
        fs::write(
            plugin_root.join("plugin.toml"),
            r#"
            [plugin]
            name = "Ghost"

            [views.main]
            [views.main.engine]
            type = "picker"
            [views.main.engine.config]
            [views.main.keymap]
            escape = "back"
            esc = false
            "#,
        )
        .unwrap();
        let core_root = root.join("plugins/core");
        fs::create_dir_all(&core_root).unwrap();
        fs::write(
            core_root.join("plugin.toml"),
            r#"
            [plugin]
            name = "core"

            [views.default.engine]
            type = "picker"
            [views.default.engine.config]
            "#,
        )
        .unwrap();
        let config_path = root.join("config.toml");
        fs::write(
            &config_path,
            r#"
            disabled_plugins = ["ghost"]
            default_view = "core:default"
            "#,
        )
        .unwrap();

        let config = Config::load(&config_path).unwrap();
        assert!(!config.compiled.views.contains_key("ghost:main"));
        assert!(config.compiled.views.contains_key("core:default"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plugin_views_are_namespaced() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [[plugins.core.views.default.engine.config.feeds]]
            view = "apps:main"
            [plugins.apps.views.main]
            [plugins.apps.views.main.engine]
            type = "picker"
            [plugins.apps.views.main.engine.config]
            items = []
"#,
        );
        assert_eq!(config.default_view.as_deref(), Some("core:default"));
        assert_eq!(
            config.compiled.views["apps:main"].engine.engine_type,
            ENGINE_PICKER.to_string()
        );
        assert_eq!(
            config.compiled.views["core:default"].selected_feeds()[0].view,
            "apps:main"
        );
    }

    #[test]
    fn view_type_selects_the_engine_directly() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
"#,
        );
        config.validate().unwrap();
        assert_eq!(config.engine("core:default").unwrap(), ENGINE_PICKER);
        assert_eq!(
            config.view("core:default").unwrap().engine.engine_type,
            ENGINE_PICKER
        );
    }

    #[test]
    fn engine_rejects_unknown_view_fields() {
        let config = config(
            r#"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "capture"
            [plugins.core.views.default.engine.config]
            output = "ok"
            titel = "typo"
"#,
        );
        let error = config
            .validate()
            .expect_err("unknown engine fields should be rejected");
        assert!(error.to_string().contains("unsupported field \"titel\""));
    }

    #[test]
    fn capture_keymap_patches_override_the_effective_physical_keys() {
        let valid = config(
            r#"
            [defaults.capture.bindings]
            copy = ["ctrl+y"]
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "capture"
            [plugins.core.views.default.engine.config]
            output = "ok"
            [plugins.core.views.default.keymap]
            "ctrl+y" = false
            "alt+c" = "copy"
"#,
        );
        valid.validate().unwrap();

        let invalid = config(
            r#"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "capture"
            [plugins.core.views.default.engine.config]
            output = "ok"
            [plugins.core.views.default.keymap]
            enter = false
            "ctrl+j" = "copy"
"#,
        );
        let error = invalid
            .validate()
            .expect_err("one key cannot be both disabled and rebound");
        assert!(format!("{error:#}").contains("both disabled and rebound"));
    }

    #[test]
    fn engine_config_does_not_accept_view_keymaps() {
        let config = config(
            r#"
            [plugins.core.views.default.engine]
            type = "capture"
            [plugins.core.views.default.engine.config]
            output = "ok"
            [plugins.core.views.default.engine.config.bindings]
            copy = ["ctrl+y"]
"#,
        );
        let error = config
            .validate()
            .expect_err("engine config bindings are not View keymap patches");
        assert!(error.to_string().contains("unsupported field \"bindings\""));
    }

    #[test]
    fn feeds_views_may_define_page_commands() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [[plugins.core.views.default.engine.config.feeds]]
            view = "apps:main"
            [plugins.core.views.default.commands.open]
            key = "enter"
            label = "Open"
            type = "run"

            [plugins.core.views.default.commands.open.payload]
            handler = { source = "script", file = "{{ view.query.script }}" }
            [plugins.apps.views.main]
            [plugins.apps.views.main.engine]
            type = "picker"
            [plugins.apps.views.main.engine.config]
            items = []
"#,
        );
        config
            .validate()
            .expect("feeds pages may define page-level commands");
    }

    #[test]
    fn nested_feeds_are_rejected() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [[plugins.core.views.default.engine.config.feeds]]
            view = "core:hub"
            [plugins.core.views.hub]
            [plugins.core.views.hub.engine]
            type = "picker"
            [plugins.core.views.hub.engine.config]
            [[plugins.core.views.hub.engine.config.feeds]]
            view = "apps:main"
            [plugins.apps.views.main]
            [plugins.apps.views.main.engine]
            type = "picker"
            [plugins.apps.views.main.engine.config]
            items = []
"#,
        );
        let error = config
            .validate()
            .expect_err("nested feeds should be rejected");
        assert!(error.to_string().contains("cannot use feeds view"));
    }

    #[test]
    fn feed_unknown_fields_are_rejected() {
        let value: Result<toml::Value, _> = toml::from_str(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [[plugins.core.views.default.engine.config.feeds]]
            view = "apps:main"
            raw = "{{ view.input }}"
            [plugins.apps.views.main]
            [plugins.apps.views.main.engine]
            type = "picker"
            [plugins.apps.views.main.engine.config]
            items = []
"#,
        );
        let value = value.expect("toml should parse");
        let error = value
            .try_into::<RawConfig>()
            .expect_err("unknown feed fields should be rejected");
        assert!(
            error.to_string().contains("unknown field") || error.to_string().contains("raw"),
            "error={error}"
        );
    }

    #[test]
    fn feeds_views_cannot_define_items() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            items = []
            [[plugins.core.views.default.engine.config.feeds]]
            view = "apps:main"
            [plugins.apps.views.main]
            [plugins.apps.views.main.engine]
            type = "picker"
            [plugins.apps.views.main.engine.config]
            items = []
"#,
        );
        let error = config
            .validate()
            .expect_err("feeds+items should be rejected");
        assert!(error.to_string().contains("cannot define items"));
    }

    #[test]
    fn navigation_target_must_reference_a_configured_view() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [plugins.core.views.default.commands.open]
            key = "enter"
            label = "Open"
            type = "navigate"
            [plugins.core.views.default.commands.open.payload]
            target = "missing:view"
            query = "item"
            "#,
        );
        let error = config
            .validate()
            .expect_err("navigation target must reference a configured view");
        assert!(error.to_string().contains("references missing view"));
    }

    #[test]
    fn static_command_targets_accept_view_aliases() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [plugins.core.views.default.commands.navigate]
            key = "enter"
            label = "Navigate"
            type = "navigate"
            [plugins.core.views.default.commands.navigate.payload]
            target = "app"
            [plugins.core.views.default.commands.call]
            key = "ctrl+a"
            label = "Call"
            type = "call"
            [plugins.core.views.default.commands.call.payload]
            target = "app"
            [plugins.apps.views.main]
            alias = "app"
            [plugins.apps.views.main.engine]
            type = "picker"
            [plugins.apps.views.main.engine.config]
            "#,
        );

        config.validate().unwrap();
    }

    #[test]
    fn continuation_return_rejects_root_only_adapter_fields() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [plugins.core.views.default.commands.open]
            key = "enter"
            label = "Open"
            type = "call"
            [plugins.core.views.default.commands.open.payload]
            target = "forms:main"
            [plugins.core.views.default.commands.open.payload.then]
            type = "return"
            [plugins.core.views.default.commands.open.payload.then.payload]
            value = "{{ result.output.value }}"
            handler = "scripts/result.sh"
            args = ["{{ result }}"]
            [plugins.forms.views.main]
            [plugins.forms.views.main.engine]
            type = "picker"
            [plugins.forms.views.main.engine.config]
            "#,
        );

        let error = config
            .validate()
            .expect_err("continuation return adapters should be rejected");
        assert!(
            error
                .to_string()
                .contains("continuation return cannot define handler or args")
        );
    }

    #[test]
    fn return_args_require_a_handler() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [plugins.core.views.default.commands.accept]
            key = "enter"
            label = "Accept"
            type = "return"
            [plugins.core.views.default.commands.accept.payload]
            value = "accepted"
            args = ["ignored"]
            "#,
        );

        let error = config
            .validate()
            .expect_err("return args without a handler should be rejected");
        assert!(error.to_string().contains("return args require a handler"));
    }

    #[test]
    fn duplicate_plugin_names_are_allowed() {
        let config = config(
            r#"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [plugins.package-a]
            name = "template"
            [plugins.package-a.views.default]
            alias = "temp-a"
            [plugins.package-a.views.default.engine]
            type = "picker"
            [plugins.package-a.views.default.engine.config]
            [plugins.package-b]
            name = "template"
            [plugins.package-b.views.default]
            alias = "temp-b"
            [plugins.package-b.views.default.engine]
            type = "picker"
            [plugins.package-b.views.default.engine.config]
"#,
        );

        config.validate().unwrap();
        assert_eq!(config.compiled.plugins["package-a"].name, "template");
        assert_eq!(config.compiled.plugins["package-b"].name, "template");
    }

    #[test]
    fn wildcard_feeds_expand_to_matching_picker_views() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [[plugins.core.views.default.engine.config.feeds]]
            view = "*:main"
            [[plugins.core.views.default.engine.config.feeds]]
            view = "*:default"
            [plugins.apps.views.main.engine]
            type = "picker"
            [plugins.apps.views.main.engine.config]
            items = []
            [plugins.apps.views.default.engine]
            type = "picker"
            [plugins.apps.views.default.engine.config]
            items = []
            [plugins.sys.views.main.engine]
            type = "picker"
            [plugins.sys.views.main.engine.config]
            items = []
            [plugins.shell.views.main.engine]
            type = "embedded"
            [plugins.shell.views.main.engine.config]
            command = ["sh"]
            title = "shell"
"#,
        );
        let feeds = config
            .feed_views("core:default")
            .unwrap()
            .into_iter()
            .map(|(view_ref, _)| view_ref)
            .collect::<Vec<_>>();
        assert_eq!(feeds, ["apps:main", "sys:main", "apps:default"]);
        config.validate().unwrap();
    }

    #[test]
    fn duplicate_view_aliases_are_rejected() {
        let config = config(
            r#"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [plugins.package-a.views.default]
            alias = "temp"
            [plugins.package-a.views.default.engine]
            type = "picker"
            [plugins.package-a.views.default.engine.config]
            [plugins.package-b.views.default]
            alias = "temp"
            [plugins.package-b.views.default.engine]
            type = "picker"
            [plugins.package-b.views.default.engine.config]
"#,
        );

        let error = config
            .validate()
            .expect_err("duplicate aliases should be rejected");
        assert!(
            error
                .to_string()
                .contains("view alias \"temp\" is assigned to both")
        );
    }

    #[test]
    fn view_alias_cannot_contain_route_syntax_or_whitespace() {
        for alias in ["", "bad:alias", "bad alias"] {
            let config = config(&format!(
                r#"
                [plugins.core.views.default]
                alias = {alias:?}
                [plugins.core.views.default.engine]
                type = "picker"
                [plugins.core.views.default.engine.config]
"#
            ));
            assert!(
                config.validate().is_err(),
                "alias {alias:?} should be invalid"
            );
        }
    }

    #[test]
    fn command_keys_are_validated_and_normalized() {
        assert_eq!(normalize_key("Alt+C").unwrap(), "alt+c");
        assert_eq!(normalize_key("enter").unwrap(), "enter");
        assert_eq!(normalize_key("Ctrl+R").unwrap(), "ctrl+r");
        assert_eq!(normalize_key("Ctrl+J").unwrap(), "enter");
        assert_eq!(normalize_key("c").unwrap(), "c");
        assert_eq!(normalize_key("space").unwrap(), "space");
    }

    #[test]
    fn plugin_api_defaults_to_one() {
        let header: PluginHeader = toml::from_str(
            r#"
            name = "template"
            "#,
        )
        .unwrap();
        assert_eq!(header.api, 1);
    }

    #[test]
    fn plugin_directory_names_are_valid_view_namespace_components() {
        assert!(validate_plugin_id("apps").is_ok());
        assert!(validate_plugin_id("my-app").is_ok());
        assert!(validate_plugin_id("bad:name").is_err());
        assert!(validate_plugin_id("bad name").is_err());
    }

    #[test]
    fn file_backed_plugin_scripts_are_loaded_and_rooted() {
        let root = env::temp_dir().join(format!("tui-launcher-plugin-test-{}", std::process::id()));
        let plugin_root = root.join("plugins/filetest");
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(plugin_root.join("scripts")).unwrap();
        fs::write(
            plugin_root.join("plugin.toml"),
            r#"
            [plugin]
            name = "file test"

            [views.main]
            alias = "file"
            [views.main.engine]
            type = "picker"
            [views.main.engine.config.items]
            source = "script"
            file = "scripts/items.sh"
            [views.main.commands.run]
            key = "enter"
            label = "Run"
            type = "run"

            [views.main.commands.run.payload]
            handler = { source = "script", file = "scripts/run.sh" }
            "#,
        )
        .unwrap();
        fs::write(
            plugin_root.join("scripts/items.sh"),
            "printf '%s\\n' '[{\"label\":\"from file\"}]'\\n",
        )
        .unwrap();
        fs::write(plugin_root.join("scripts/run.sh"), "printf 'run\\n'\\n").unwrap();
        let core_root = root.join("plugins/core");
        fs::create_dir_all(&core_root).unwrap();
        fs::write(
            core_root.join("plugin.toml"),
            r#"
            [plugin]
            name = "core"

            [views.default.engine]
            type = "picker"
            [views.default.engine.config]
            [[views.default.engine.config.feeds]]
            view = "filetest:main"
            "#,
        )
        .unwrap();
        let config_path = root.join("config.toml");
        let default_source = r#"
            default_view = "core:default"
"#;
        fs::write(&config_path, default_source).unwrap();

        let config = Config::load(&config_path).unwrap();
        let items = config.compiled.views["filetest:main"]
            .engine
            .config
            .items
            .as_ref()
            .and_then(toml::Value::as_table)
            .expect("script items table");
        assert_eq!(
            items.get("source").and_then(toml::Value::as_str),
            Some("script")
        );
        assert_eq!(
            items.get("file").and_then(toml::Value::as_str),
            Some("scripts/items.sh")
        );
        let CommandAction::Run { payload } =
            &config.compiled.views["filetest:main"].commands["run"].action
        else {
            panic!("file command did not deserialize as a run action");
        };
        assert_eq!(
            payload.handler,
            toml::from_str::<toml::Value>("source = \"script\"\nfile = \"scripts/run.sh\"\n")
                .unwrap()
        );
        assert_eq!(
            config.plugin_root("filetest:main"),
            Some(plugin_root.as_path())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loaded_config_contains_the_merged_json_tree() {
        let root = env::temp_dir().join(format!("tui-launcher-config-json-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        let core_root = root.join("plugins/core");
        fs::create_dir_all(&core_root).unwrap();
        fs::write(
            core_root.join("plugin.toml"),
            r#"
            [plugin]
            name = "core"

            [views.default.engine]
            type = "picker"
            [views.default.engine.config]
            "#,
        )
        .unwrap();
        let config_path = root.join("config.toml");
        fs::write(
            &config_path,
            r#"
            [aa.a]
            bb = 1

            [aa.b]
            bb = 2
            "#,
        )
        .unwrap();

        let config = Config::load(&config_path).unwrap();
        assert_eq!(
            config
                .compiled
                .config_value
                .pointer("/aa/a/bb")
                .and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            config
                .compiled
                .config_value
                .pointer("/aa/b/bb")
                .and_then(Value::as_i64),
            Some(2)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dynamic_script_source_fields_resolve_with_one_evaluator() {
        let source: toml::Value = toml::from_str(
            r#"
            source = "{{ page.query.source }}"
            file = "{{ page.query.file }}"
            max_output_bytes = "{{ page.query.limit }}"
            args = ["{{ page.query }}"]
            "#,
        )
        .unwrap();
        ScriptSourceSpec::parse(&source).unwrap();
        let source_json = toml_to_json(&source).unwrap();
        let registry = TemplateRegistry::compile_json_tree(&source_json).unwrap();
        let root = serde_json::json!({
            "page": {
                "query": {
                    "source": "script",
                    "file": "scripts/items.sh",
                    "limit": 128,
                }
            }
        });
        let resolved_value = evaluate_json_value(
            &source_json,
            &EvalContext {
                root: &root,
                cancellation: None,
                templates: Some(&registry),
            },
        )
        .unwrap();
        let resolved = ResolvedScriptSource::parse(&resolved_value).unwrap();
        assert_eq!(resolved.file, "scripts/items.sh");
        assert_eq!(resolved.max_output_bytes, Some(128));
        assert_eq!(
            resolved.args,
            Some(serde_json::json!([
                {
                    "source": "script",
                    "file": "scripts/items.sh",
                    "limit": 128,
                }
            ]))
        );
    }

    #[test]
    fn root_source_resolves_against_the_consuming_view_owner() {
        let mut config = load_test_fixture().unwrap();
        config.compiled.config_value["defaults"]["picker"]["bindings"] = serde_json::json!({
            "exit": ["{{ view.ref }}"]
        });
        config.rebuild_template_registry().unwrap();
        let owner = config.instantiate_state("apps:main").unwrap();
        let runtime = serde_json::json!({
            "view": {"current": {"ref": "core:default"}}
        });
        let snapshot = EvaluationSnapshot::new(
            InvocationScope::new(&config.input_value),
            SessionScope::new(&runtime),
            Some(OwnerViewScope::new(&owner)),
            None,
        );
        let value = config
            .get(
                ConfigSource::Root,
                &snapshot,
                EvaluationStage::Operation,
                &["defaults", "picker", "bindings"],
            )
            .unwrap()
            .unwrap();
        assert_eq!(value["exit"][0], "apps:main");

        let missing_owner = EvaluationSnapshot::new(
            InvocationScope::new(&config.input_value),
            SessionScope::new(&runtime),
            None,
            None,
        );
        let error = config
            .get(
                ConfigSource::Root,
                &missing_owner,
                EvaluationStage::Operation,
                &["defaults", "picker", "bindings"],
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("namespace \"view\" is unavailable")
        );
    }

    #[test]
    fn scope_capabilities_distinguish_an_unavailable_result_from_null_selection() {
        let mut config = load_test_fixture().unwrap();
        config.compiled.config_value["defaults"]["picker"]["bindings"] = serde_json::json!({
            "exit": ["{{ result }}"],
            "back": ["{{ selection }}"],
        });
        config.rebuild_template_registry().unwrap();
        let owner = config.instantiate_state("apps:main").unwrap();
        let runtime = serde_json::json!({
            "view": {"current": {"ref": "core:default"}}
        });
        let missing_return_scope = EvaluationSnapshot::new(
            InvocationScope::new(&config.input_value),
            SessionScope::new(&runtime),
            Some(OwnerViewScope::new(&owner)),
            None,
        );
        let error = config
            .get(
                ConfigSource::Root,
                &missing_return_scope,
                EvaluationStage::Return,
                &["defaults", "picker", "bindings"],
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("namespace \"result\" is unavailable")
        );

        let returned = Value::Null;
        let snapshot = EvaluationSnapshot::new(
            InvocationScope::new(&config.input_value),
            SessionScope::new(&runtime),
            Some(OwnerViewScope::new(&owner)),
            None,
        )
        .with_return_scope(Some(ReturnScope::new(&returned)));
        let value = config
            .get(
                ConfigSource::Root,
                &snapshot,
                EvaluationStage::Return,
                &["defaults", "picker", "bindings"],
            )
            .unwrap()
            .unwrap();
        assert!(value["exit"][0].is_null());
        assert!(value["back"][0].is_null());
    }

    #[test]
    fn dynamic_context_projects_only_requested_page_fields() {
        let source = serde_json::json!("{{ page.input }}");
        let registry = TemplateRegistry::compile_json_tree(&source).unwrap();
        let requirements = registry.requirements_for_value(&source).unwrap();
        assert!(requirements.requires(Namespace::Page));
        assert!(requirements.requires_field(Namespace::Page, "input"));
        assert!(!requirements.requires_field(Namespace::Page, "items"));
        assert!(!requirements.requires(Namespace::Session));

        let runtime = serde_json::json!({
            "view": {"current": {
                "input": "query",
                "items": (0..100_001).collect::<Vec<_>>()
            }}
        });
        let mut budget = Budget::default();
        let page = public_page_context(&runtime, &requirements, None, &mut budget).unwrap();
        assert_eq!(page["input"], "query");
        assert!(page.get("items").is_none());
    }

    #[test]
    fn root_theme_is_not_exposed_in_the_workflow_config_tree() {
        let root =
            env::temp_dir().join(format!("tui-launcher-config-theme-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        let core_root = root.join("plugins/core");
        fs::create_dir_all(&core_root).unwrap();
        fs::write(
            core_root.join("plugin.toml"),
            r#"
            [plugin]
            name = "core"

            [views.default.engine]
            type = "picker"
            "#,
        )
        .unwrap();
        let config_path = root.join("config.toml");
        fs::write(
            &config_path,
            r#"
            theme = "work"
            "#,
        )
        .unwrap();
        fs::create_dir_all(root.join("themes")).unwrap();
        fs::write(
            root.join("themes/work.toml"),
            "[palette]\nbrand = \"green\"\n\n[scheme]\nprimary = \"palette:brand\"\n",
        )
        .unwrap();

        let loaded = Config::load_app(&config_path, &ThemeLoadOptions::default()).unwrap();
        assert!(loaded.config.compiled.config_value.get("theme").is_none());
        assert_eq!(loaded.theme.picker.marker.fg, Some(Color::Green));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn keymap_tombstones_override_recursive_plugin_values() {
        let mut base: toml::Value = toml::from_str(
            r#"
            [plugins.base.views.main]
            [plugins.base.views.main.engine]
            type = "picker"
            [plugins.base.views.main.engine.config]
            [plugins.base.views.main.keymap]
            escape = "back"
"#,
        )
        .unwrap();
        let mut overlay: toml::Value = toml::from_str(
            r#"
            [plugins.base.views.main.keymap]
            esc = false
"#,
        )
        .unwrap();
        normalize_keymap_tables(&mut base).unwrap();
        normalize_keymap_tables(&mut overlay).unwrap();
        merge_values(&mut base, overlay);
        let keymap = base
            .get("plugins")
            .and_then(|value| value.get("base"))
            .and_then(|value| value.get("views"))
            .and_then(|value| value.get("main"))
            .and_then(|value| value.get("keymap"))
            .and_then(toml::Value::as_table)
            .expect("merged view keymap");
        assert_eq!(keymap.len(), 1);
        assert_eq!(keymap.get("escape"), Some(&toml::Value::Boolean(false)));

        let raw: RawConfig = base.try_into().unwrap();
        let config =
            Config::from_raw(raw, BTreeMap::new(), Value::Object(serde_json::Map::new())).unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.compiled.views["base:main"]
                .keymap
                .as_ref()
                .and_then(toml::Value::as_table)
                .and_then(|value| value.get("escape")),
            Some(&toml::Value::Boolean(false))
        );
    }

    #[test]
    fn keymap_aliases_conflict_within_one_configuration_layer() {
        let mut value: toml::Value = toml::from_str(
            r#"
            [plugins.core.views.default.keymap]
            escape = "back"
            esc = false
"#,
        )
        .unwrap();
        let error = normalize_keymap_tables(&mut value)
            .expect_err("aliases in one keymap layer must conflict");
        assert!(error.to_string().contains("normalize to the same key"));
    }

    #[test]
    fn user_tables_extend_defaults() {
        let mut base: toml::Value = toml::from_str(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [plugins.base.views.main]
            [plugins.base.views.main.engine]
            type = "picker"
            [plugins.base.views.main.engine.config]
            items = "{{ page.items }}"
"#,
        )
        .unwrap();
        let overlay: toml::Value = toml::from_str(
            r#"
            [plugins.base.views.main.engine.config]
            items = "{{ view.query }}"
            "#,
        )
        .unwrap();
        merge_values(&mut base, overlay);
        let raw: RawConfig = base.try_into().unwrap();
        let config =
            Config::from_raw(raw, BTreeMap::new(), Value::Object(serde_json::Map::new())).unwrap();
        assert_eq!(
            config.compiled.views["base:main"]
                .engine
                .config
                .items
                .as_ref()
                .and_then(toml::Value::as_str),
            Some("{{ view.query }}")
        );
    }
}
