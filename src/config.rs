use crate::cancellation::CancellationToken;
use crate::expression::{
    ContextRequirements, EvalContext, EvaluationStage, Namespace, Template, TemplateRegistry,
    clone_json_value_bounded, evaluate_json_value, is_dynamic_string,
};
use crate::input::Key;
use crate::state::{StateInstance, StateRegistry};
use crate::theme::{ResolvedTheme, ThemeLoadOptions};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
#[cfg(test)]
use std::ops::DerefMut;
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs,
    ops::Deref,
    path::{Path, PathBuf},
};

pub type ViewRef = String;

pub const ENGINE_PICKER: &str = "picker";
pub const ENGINE_CAPTURE: &str = "capture";
pub const ENGINE_EMBEDDED: &str = "embedded";

#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub default_view: Option<ViewRef>,
    pub(crate) image_protocol: ImageProtocol,
    pub(crate) log_file: Option<PathBuf>,
    pub(crate) chrome: ChromeConfig,
    pub(crate) input_value: Value,
    pub(crate) invocation_state: StateInstance,
    compiled: CompiledConfig,
}

#[derive(Debug, Clone)]
pub(crate) struct CompiledConfig {
    pub(crate) views: BTreeMap<ViewRef, View>,
    pub(crate) plugins: BTreeMap<String, PluginMetadata>,
    pub(crate) defaults: Defaults,
    pub(crate) plugin_roots: BTreeMap<String, PathBuf>,
    pub(crate) config_value: Value,
    pub(crate) template_registry: TemplateRegistry,
    pub(crate) state_registry: StateRegistry,
}

impl Deref for Config {
    type Target = CompiledConfig;

    fn deref(&self) -> &Self::Target {
        &self.compiled
    }
}

#[cfg(test)]
impl DerefMut for Config {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.compiled
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ConfigSource<'a> {
    Root,
    View(&'a str),
}

/// Immutable invocation data captured before the session starts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InvocationScope<'a> {
    input: &'a Value,
}

impl<'a> InvocationScope<'a> {
    pub(crate) fn new(input: &'a Value) -> Self {
        Self { input }
    }

    fn populate(
        self,
        requirements: &ContextRequirements,
        cancellation: Option<&CancellationToken>,
        context: &mut serde_json::Map<String, Value>,
    ) -> Result<()> {
        if requirements.requires(Namespace::Input) {
            context.insert(
                Namespace::Input.name().to_string(),
                clone_json_value_bounded(self.input, cancellation)?,
            );
        }
        Ok(())
    }
}

/// Immutable active-session data captured for one evaluation operation.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SessionScope<'a> {
    runtime: &'a Value,
}

impl<'a> SessionScope<'a> {
    pub(crate) fn new(runtime: &'a Value) -> Self {
        Self { runtime }
    }

    fn populate(
        self,
        requirements: &ContextRequirements,
        cancellation: Option<&CancellationToken>,
        context: &mut serde_json::Map<String, Value>,
    ) -> Result<()> {
        if requirements.requires(Namespace::Page) {
            context.insert(
                Namespace::Page.name().to_string(),
                public_page_context(self.runtime, requirements, cancellation)?,
            );
        }
        if requirements.requires(Namespace::Selection) {
            let default = Value::Null;
            let selection = self
                .runtime
                .pointer("/view/current/selected_item")
                .unwrap_or(&default);
            context.insert(
                Namespace::Selection.name().to_string(),
                clone_json_value_bounded(selection, cancellation)?,
            );
        }
        if requirements.requires(Namespace::Session) {
            context.insert(
                Namespace::Session.name().to_string(),
                public_session_context(self.runtime, requirements, cancellation)?,
            );
        }
        Ok(())
    }
}

/// The View that owns the configuration value being evaluated.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OwnerViewScope<'a> {
    state: &'a StateInstance,
    /// Feed defaults retain the coordinating page's committed raw binding.
    binding_raw: Option<&'a str>,
}

impl<'a> OwnerViewScope<'a> {
    pub(crate) fn new(state: &'a StateInstance) -> Self {
        Self {
            state,
            binding_raw: None,
        }
    }

    pub(crate) fn with_binding_raw(mut self, binding_raw: Option<&'a str>) -> Self {
        self.binding_raw = binding_raw;
        self
    }

    fn populate(
        self,
        config: &Config,
        requirements: &ContextRequirements,
        cancellation: Option<&CancellationToken>,
        context: &mut serde_json::Map<String, Value>,
    ) -> Result<()> {
        if requirements.requires(Namespace::View) {
            let view = config.view_value(self.state, self.binding_raw)?;
            context.insert(
                Namespace::View.name().to_string(),
                clone_json_value_bounded(&view, cancellation)?,
            );
        }
        Ok(())
    }
}

/// A returned value exposed only while preparing a continuation or result handler.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReturnScope<'a> {
    value: &'a Value,
}

impl<'a> ReturnScope<'a> {
    pub(crate) fn new(value: &'a Value) -> Self {
        Self { value }
    }

    fn populate(
        self,
        requirements: &ContextRequirements,
        cancellation: Option<&CancellationToken>,
        context: &mut serde_json::Map<String, Value>,
    ) -> Result<()> {
        if requirements.requires(Namespace::Result) {
            context.insert(
                Namespace::Result.name().to_string(),
                clone_json_value_bounded(self.value, cancellation)?,
            );
        }
        Ok(())
    }
}

/// Immutable scope composition captured for one configuration operation.
///
/// ConfigSource selects raw configuration independently. The owner scope can
/// therefore differ from the active session page while a feed is evaluated.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EvaluationSnapshot<'a> {
    invocation: InvocationScope<'a>,
    session: SessionScope<'a>,
    owner: Option<OwnerViewScope<'a>>,
    returned: Option<ReturnScope<'a>>,
    cancellation: Option<&'a CancellationToken>,
}

impl<'a> EvaluationSnapshot<'a> {
    pub(crate) fn new(
        invocation: InvocationScope<'a>,
        session: SessionScope<'a>,
        owner: Option<OwnerViewScope<'a>>,
        cancellation: Option<&'a CancellationToken>,
    ) -> Self {
        Self {
            invocation,
            session,
            owner,
            returned: None,
            cancellation,
        }
    }

    pub(crate) fn with_return_scope(mut self, returned: Option<ReturnScope<'a>>) -> Self {
        self.returned = returned;
        self
    }

    fn owner_scope(&self) -> Result<OwnerViewScope<'a>> {
        self.owner
            .context("dynamic namespace \"view\" is unavailable without an owning View")
    }

    fn return_scope(&self) -> Result<ReturnScope<'a>> {
        self.returned
            .context("dynamic namespace \"result\" is unavailable in this operation")
    }

    fn expression_context(
        &self,
        config: &Config,
        requirements: &ContextRequirements,
    ) -> Result<Value> {
        let mut context = serde_json::Map::new();
        self.invocation
            .populate(requirements, self.cancellation, &mut context)?;
        self.session
            .populate(requirements, self.cancellation, &mut context)?;
        if requirements.requires(Namespace::View) {
            self.owner_scope()?
                .populate(config, requirements, self.cancellation, &mut context)?;
        }
        if requirements.requires(Namespace::Result) {
            self.return_scope()?
                .populate(requirements, self.cancellation, &mut context)?;
        }
        Ok(Value::Object(context))
    }
}

#[derive(Debug, Clone)]
pub struct PluginMetadata {
    pub name: String,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ImageProtocol {
    #[default]
    Halfblocks,
    Kitty,
    Sixel,
    Iterm2,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Defaults {
    #[serde(default)]
    pub(crate) picker: PickerDefaults,
    #[serde(default)]
    pub(crate) capture: CaptureDefaults,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PickerDefaults {
    #[serde(default)]
    pub(crate) bindings: Option<toml::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureDefaults {
    #[serde(default)]
    pub(crate) bindings: Option<toml::Value>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct EngineSpec {
    #[serde(rename = "type", default = "default_engine_type")]
    pub engine_type: String,
    #[serde(default)]
    pub config: EngineOptions,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct FeedSpec {
    pub view: ViewRef,
}

/// A file-backed script source descriptor shared by data-source engines and
/// `run` commands. Script contents are never part of the configuration tree.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptSourceSpec {
    source: toml::Value,
    file: toml::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    args: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_output_bytes: Option<toml::Value>,
}

#[derive(Debug)]
pub(crate) struct ResolvedScriptSource {
    pub(crate) file: String,
    pub(crate) args: Option<Value>,
    pub(crate) max_output_bytes: Option<usize>,
}

impl ScriptSourceSpec {
    #[cfg(test)]
    pub(crate) fn script_file(file: impl Into<String>) -> Self {
        Self {
            source: toml::Value::String("script".to_string()),
            file: toml::Value::String(file.into()),
            args: None,
            max_output_bytes: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn as_toml_value(&self) -> toml::Value {
        let mut table = toml::map::Map::new();
        table.insert("source".to_string(), self.source.clone());
        table.insert("file".to_string(), self.file.clone());
        if let Some(args) = &self.args {
            table.insert("args".to_string(), args.clone());
        }
        if let Some(max_output_bytes) = &self.max_output_bytes {
            table.insert("max_output_bytes".to_string(), max_output_bytes.clone());
        }
        toml::Value::Table(table)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        validate_script_source_name(&self.source)?;
        validate_script_source_file(&self.file)?;
        validate_script_source_args(self.args.as_ref())?;
        validate_script_source_limit(self.max_output_bytes.as_ref())?;
        Ok(())
    }

    pub(crate) fn validate_picker_source(&self) -> Result<()> {
        self.validate()
    }

    pub(crate) fn validate_capture_source(&self) -> Result<()> {
        self.validate()
    }

    pub(crate) fn validate_command_handler(&self) -> Result<()> {
        self.validate()?;
        if self.args.is_some() {
            bail!("run command handler source cannot define args; configure payload args");
        }
        if self.max_output_bytes.is_some() {
            bail!("run command handler source cannot define max_output_bytes");
        }
        Ok(())
    }

    pub(crate) fn validate_target(&self, root: &Path) -> Result<()> {
        let Some(file) = self.file.as_str() else {
            return Ok(());
        };
        if is_dynamic_string(file) {
            return Ok(());
        }
        crate::script_runner::validate_script_target(root, file)
    }

    pub(crate) fn parse(value: &toml::Value) -> Result<Self> {
        let spec: Self = value
            .clone()
            .try_into()
            .context("script source must match the source schema")?;
        spec.validate()?;
        Ok(spec)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolvedScriptSourceConfig {
    source: String,
    file: String,
    #[serde(default)]
    args: Option<Value>,
    #[serde(default)]
    max_output_bytes: Option<usize>,
}

impl ResolvedScriptSource {
    pub(crate) fn is_candidate(value: &Value) -> bool {
        value
            .as_object()
            .is_some_and(|fields| fields.contains_key("source"))
    }

    pub(crate) fn parse(value: &Value) -> Result<Self> {
        let source: ResolvedScriptSourceConfig = serde_json::from_value(value.clone())
            .context("script source must resolve to an object with source and file fields")?;
        if source.source != "script" {
            bail!("unsupported script source {:?}", source.source);
        }
        if source.file.is_empty() {
            bail!("script source file must be non-empty");
        }
        crate::script_runner::validate_max_output_bytes(source.max_output_bytes)?;
        Ok(Self {
            file: source.file,
            args: source.args,
            max_output_bytes: source.max_output_bytes,
        })
    }

    pub(crate) fn script_args(&self, label: &str) -> Result<Vec<String>> {
        crate::script_runner::resolve_argv(self.args.as_ref(), label)
    }

    pub(crate) fn command_file(&self) -> Result<&str> {
        anyhow::ensure!(
            self.args.is_none(),
            "run command handler source cannot define args; configure payload args"
        );
        anyhow::ensure!(
            self.max_output_bytes.is_none(),
            "run command handler source cannot define max_output_bytes"
        );
        Ok(&self.file)
    }
}

fn validate_script_source_name(value: &toml::Value) -> Result<()> {
    let source = value
        .as_str()
        .context("script source must be a string or dynamic path")?;
    if is_dynamic_string(source) {
        Template::parse(source)?;
    } else if source != "script" {
        bail!("unsupported script source {:?}", source);
    }
    Ok(())
}

fn validate_script_source_file(value: &toml::Value) -> Result<()> {
    let file = value
        .as_str()
        .context("script source file must be a string or dynamic path")?;
    if file.is_empty() {
        bail!("script source file must be non-empty");
    }
    if is_dynamic_string(file) {
        Template::parse(file)?;
    }
    Ok(())
}

fn validate_script_source_args(value: Option<&toml::Value>) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    match value {
        toml::Value::String(source) => {
            anyhow::ensure!(
                Template::parse(source)?.is_complete_path(),
                "script args must be an array or complete dynamic path"
            );
        }
        toml::Value::Array(values) => {
            for value in values {
                validate_templates(value)?;
            }
        }
        _ => bail!("script args must be an array or complete dynamic path"),
    }
    Ok(())
}

fn validate_script_source_limit(value: Option<&toml::Value>) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    match value {
        toml::Value::Integer(value) => {
            let value = usize::try_from(*value)
                .context("script max_output_bytes must be a non-negative integer")?;
            crate::script_runner::validate_max_output_bytes(Some(value))
        }
        toml::Value::String(source) => {
            anyhow::ensure!(
                Template::parse(source)?.is_complete_path(),
                "script max_output_bytes must be an integer or complete dynamic path"
            );
            Ok(())
        }
        _ => bail!("script max_output_bytes must be an integer or complete dynamic path"),
    }
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct EngineOptions {
    #[serde(default)]
    pub feeds: Vec<FeedSpec>,
    #[serde(default)]
    pub items: Option<toml::Value>,
    #[serde(flatten)]
    pub fields: toml::Table,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub engine: EngineSpec,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub run_shell: Option<String>,
    #[serde(default)]
    pub cancel_exit_code: Option<u8>,
    #[serde(default)]
    #[allow(dead_code)]
    pub(crate) query: Option<toml::Table>,
    #[serde(default)]
    pub(crate) keymap: Option<toml::Value>,
    #[serde(default)]
    pub commands: BTreeMap<String, Command>,
}

impl View {
    pub(crate) fn selected_engine_type(&self) -> &str {
        &self.engine.engine_type
    }
    pub(crate) fn selected_engine_config(&self) -> &toml::Table {
        &self.engine.config.fields
    }
    pub(crate) fn selected_items(&self) -> Option<&toml::Value> {
        self.engine.config.items.as_ref()
    }
    pub(crate) fn selected_feeds(&self) -> &[FeedSpec] {
        &self.engine.config.feeds
    }
    pub(crate) fn is_feeds_page(&self) -> bool {
        !self.selected_feeds().is_empty()
    }
    pub(crate) fn engine_field(&self, field: &str) -> Option<&toml::Value> {
        self.selected_engine_config().get(field)
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CommandScope {
    View,
    #[default]
    Selection,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CommandRequirement {
    Input,
    #[default]
    Items,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CommandAction {
    Run {
        payload: RunPayload,
    },
    Navigate {
        payload: NavigatePayload,
    },
    Call {
        payload: CallPayload,
    },
    Return {
        #[serde(default)]
        payload: ReturnPayload,
    },
    EditInput {
        payload: EditInputPayload,
    },
    Invoke {
        payload: InvokePayload,
    },
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunPayload {
    pub handler: toml::Value,
    /// Positional arguments passed to the file-backed handler after dynamic
    /// evaluation. The handler retains terminal stdin/stdout rather than using
    /// the bounded non-interactive script runner.
    #[serde(default)]
    pub args: Option<toml::Value>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub exit: bool,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NavigatePayload {
    pub target: toml::Value,
    #[serde(default)]
    pub query: Option<toml::Value>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct CallPayload {
    pub target: toml::Value,
    #[serde(default)]
    pub query: Option<toml::Value>,
    #[serde(default)]
    pub then: Option<Box<CommandAction>>,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReturnPayload {
    #[serde(default)]
    pub value: Option<toml::Value>,
    #[serde(default)]
    pub handler: Option<toml::Value>,
    /// Positional arguments passed to the file-backed return handler after
    /// dynamic evaluation. Return handlers receive no JSON on stdin; their
    /// stdout, stderr, and exit status remain the invocation result.
    #[serde(default)]
    pub args: Option<toml::Value>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct EditInputPayload {
    pub value: toml::Value,
    #[serde(default)]
    pub cursor: Option<toml::Value>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvokePayload {
    pub command: toml::Value,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ChromeBindingVisibility {
    #[default]
    Always,
    Overflow,
    Hidden,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChromeConfig {
    #[serde(default)]
    pub(crate) footer: ChromeRegionConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChromeRegionConfig {
    #[serde(default)]
    pub(crate) bindings: BTreeMap<String, ChromeBinding>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ChromeBinding {
    pub(crate) key: String,
    pub(crate) label: String,
    #[serde(default)]
    pub(crate) visibility: ChromeBindingVisibility,
    #[serde(flatten)]
    pub(crate) action: CommandAction,
}

impl ChromeBinding {
    pub(crate) fn as_command(&self) -> Command {
        Command {
            key: self.key.clone(),
            label: self.label.clone(),
            scope: CommandScope::View,
            requires: CommandRequirement::Input,
            action: self.action.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct Command {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub scope: CommandScope,
    #[serde(default)]
    pub requires: CommandRequirement,
    #[serde(flatten)]
    pub action: CommandAction,
}

pub(crate) struct LoadedApp {
    pub(crate) config: Config,
    pub(crate) theme: ResolvedTheme,
}

#[derive(Debug, Clone, Deserialize)]
struct RawConfig {
    #[serde(default)]
    default_view: Option<String>,
    #[serde(default)]
    image_protocol: ImageProtocol,
    #[serde(default)]
    log_file: Option<PathBuf>,
    #[serde(default)]
    chrome: ChromeConfig,
    #[serde(default)]
    theme: Option<String>,
    #[serde(default)]
    plugins: BTreeMap<String, Plugin>,
    #[serde(default)]
    defaults: Defaults,
    #[serde(default)]
    viewtypes: Option<toml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct Plugin {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    views: BTreeMap<String, View>,
}

#[derive(Debug, Clone, Deserialize)]
struct PluginHeader {
    #[serde(default = "default_plugin_api")]
    api: u32,
    name: String,
}

impl CompiledConfig {
    fn build(
        views: BTreeMap<ViewRef, View>,
        plugins: BTreeMap<String, PluginMetadata>,
        defaults: Defaults,
        plugin_roots: BTreeMap<String, PathBuf>,
        config_value: Value,
    ) -> Result<Self> {
        let template_value = if config_value
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
        {
            views_config_value(&views, &plugins)?
        } else {
            config_value.clone()
        };
        let template_registry = TemplateRegistry::compile_json_tree(&template_value)?;
        validate_view_bootstrap_requirements(&views, &template_registry)?;
        let state_registry = if config_value.get("plugins").is_some() {
            StateRegistry::compile_with_templates(&config_value, &template_registry)?
        } else {
            StateRegistry::default()
        };
        Ok(Self {
            views,
            plugins,
            defaults,
            plugin_roots,
            config_value,
            template_registry,
            state_registry,
        })
    }
}

fn validate_json_requirements(
    templates: &TemplateRegistry,
    value: &Value,
    stage: EvaluationStage,
    consumer: &str,
) -> Result<()> {
    templates
        .requirements_for_value(value)?
        .validate_stage(stage, consumer)
}

fn validate_toml_requirements(
    templates: &TemplateRegistry,
    value: &toml::Value,
    stage: EvaluationStage,
    consumer: &str,
) -> Result<()> {
    let value = toml_to_json(value)?;
    validate_json_requirements(templates, &value, stage, consumer)
}

fn validate_string_requirements(
    templates: &TemplateRegistry,
    value: &str,
    stage: EvaluationStage,
    consumer: &str,
) -> Result<()> {
    validate_json_requirements(
        templates,
        &Value::String(value.to_string()),
        stage,
        consumer,
    )
}

fn validate_optional_string_requirements(
    templates: &TemplateRegistry,
    value: Option<&str>,
    stage: EvaluationStage,
    consumer: &str,
) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    validate_string_requirements(templates, value, stage, consumer)
}

fn validate_view_bootstrap_requirements(
    views: &BTreeMap<ViewRef, View>,
    templates: &TemplateRegistry,
) -> Result<()> {
    for (view_ref, view) in views {
        validate_string_requirements(
            templates,
            view.selected_engine_type(),
            EvaluationStage::Bootstrap,
            &format!("view {view_ref:?} engine type"),
        )?;
        validate_optional_string_requirements(
            templates,
            view.alias.as_deref(),
            EvaluationStage::Bootstrap,
            &format!("view {view_ref:?} alias"),
        )?;
        if let Some(query) = &view.query {
            validate_toml_requirements(
                templates,
                &toml::Value::Table(query.clone()),
                EvaluationStage::Bootstrap,
                &format!("view {view_ref:?} query schema"),
            )?;
        }
        for feed in view.selected_feeds() {
            validate_string_requirements(
                templates,
                &feed.view,
                EvaluationStage::Bootstrap,
                &format!("view {view_ref:?} feed target"),
            )?;
        }
        for (command_id, command) in &view.commands {
            validate_string_requirements(
                templates,
                &command.key,
                EvaluationStage::Bootstrap,
                &format!("view {view_ref:?} command {command_id:?} key"),
            )?;
            validate_string_requirements(
                templates,
                &command.label,
                EvaluationStage::Bootstrap,
                &format!("view {view_ref:?} command {command_id:?} label"),
            )?;
        }
    }
    Ok(())
}

impl Config {
    #[allow(dead_code)]
    pub(crate) fn load(user_path: &Path) -> Result<Self> {
        let engines = crate::engine::EngineRegistry::new();
        Self::load_with_engines(user_path, &engines)
    }

    pub(crate) fn load_app(user_path: &Path, options: &ThemeLoadOptions) -> Result<LoadedApp> {
        let engines = crate::engine::EngineRegistry::new();
        Self::load_with_engines_and_options(user_path, &engines, options)
    }

    #[allow(dead_code)]
    pub(crate) fn load_with_engines(
        user_path: &Path,
        engines: &crate::engine::EngineRegistry,
    ) -> Result<Self> {
        Ok(
            Self::load_with_engines_and_options(user_path, engines, &ThemeLoadOptions::default())?
                .config,
        )
    }

    fn load_with_engines_and_options(
        user_path: &Path,
        engines: &crate::engine::EngineRegistry,
        options: &ThemeLoadOptions,
    ) -> Result<LoadedApp> {
        let user_source = fs::read_to_string(user_path)
            .with_context(|| format!("could not read config {}", user_path.display()))?;
        let mut user_config: toml::Value = toml::from_str(&user_source)
            .with_context(|| format!("cannot parse config {}", user_path.display()))?;
        reject_root_plugins(&user_config)?;
        let disabled_plugins = disabled_plugins(Some(&user_config))?;
        remove_disabled_plugins(&mut user_config, &disabled_plugins);
        normalize_keymap_tables(&mut user_config)
            .with_context(|| format!("invalid keymap in {}", user_path.display()))?;
        let mut merged = toml::Value::Table(toml::map::Map::new());
        let plugin_directory = user_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("plugins");
        let plugin_roots = load_plugin_packages(&mut merged, &plugin_directory, &disabled_plugins)?;
        merge_values(&mut merged, user_config);
        remove_disabled_plugins(&mut merged, &disabled_plugins);

        let raw: RawConfig = merged.clone().try_into().with_context(|| {
            format!(
                "merged configuration from {} does not match the launcher schema",
                user_path.display()
            )
        })?;
        let log_file = raw
            .log_file
            .as_deref()
            .map(|path| resolve_config_path(user_path, path));
        let theme = crate::theme::load(user_path, raw.theme.as_deref(), options)?;
        if let Some(table) = merged.as_table_mut() {
            table.remove("theme");
            table.remove("log_file");
        }
        let mut config_value =
            toml_to_json(&merged).context("merged configuration cannot be represented as JSON")?;
        normalize_engine_configs(&mut config_value);
        let mut config = Self::from_raw(raw, plugin_roots, config_value)
            .context("could not compile dynamic configuration values")?;
        config.log_file = log_file;
        config.validate_with_engines(engines)?;
        Ok(LoadedApp { config, theme })
    }

    pub(crate) fn bind_invocation_state(
        &self,
        view_ref: &str,
        arguments: &[String],
    ) -> Result<StateInstance> {
        self.state_registry.bind_cli(view_ref, arguments)
    }

    pub(crate) fn set_invocation(&mut self, input: Value, state: StateInstance) {
        self.input_value = input;
        self.invocation_state = state;
    }

    pub(crate) fn items_value(
        &self,
        view_ref: &str,
        snapshot: &EvaluationSnapshot<'_>,
    ) -> Result<Option<Value>> {
        self.get(
            ConfigSource::View(view_ref),
            snapshot,
            EvaluationStage::Operation,
            &["items"],
        )
    }

    pub(crate) fn instantiate_state(&self, view_ref: &str) -> Result<StateInstance> {
        self.state_registry.instantiate(view_ref)
    }

    pub(crate) fn query_value(&self, state: &StateInstance) -> Result<Value> {
        self.state_registry.query_value(state)
    }

    pub(crate) fn update_query_value(
        &self,
        state: &mut StateInstance,
        value: &Value,
    ) -> Result<bool> {
        self.state_registry.update_value(state, value)
    }

    pub(crate) fn render_query_input(&self, state: &StateInstance) -> Result<String> {
        self.state_registry.render_input(state)
    }

    pub(crate) fn validate_query_state(&self, state: &StateInstance) -> Result<()> {
        self.state_registry.validate_instance(state)
    }

    pub(crate) fn update_query_input(
        &self,
        state: &mut StateInstance,
        source: &str,
    ) -> Result<bool> {
        self.state_registry.update_input(state, source)
    }

    fn view_config(&self, view_ref: &str) -> Result<&Value> {
        let (plugin, view) = view_ref
            .split_once(':')
            .with_context(|| format!("invalid view reference {:?}", view_ref))?;
        self.config_value
            .get("plugins")
            .and_then(|plugins| plugins.get(plugin))
            .and_then(|plugin| plugin.get("views"))
            .and_then(|views| views.get(view))
            .with_context(|| format!("view {:?} configuration disappeared", view_ref))
    }

    fn from_raw(
        raw: RawConfig,
        plugin_roots: BTreeMap<String, PathBuf>,
        config_value: Value,
    ) -> Result<Self> {
        if raw.viewtypes.is_some() {
            bail!("viewtypes are no longer supported; configure the engine directly on each view");
        }
        let default_view = raw.default_view;
        let mut views = BTreeMap::new();
        let mut plugins = BTreeMap::new();
        for (package_id, plugin) in raw.plugins {
            let metadata = PluginMetadata {
                name: plugin.name.unwrap_or_else(|| package_id.clone()),
            };
            for (view_name, view) in plugin.views {
                let view_ref = qualify_view_ref(&package_id, &view_name)?;
                if views.insert(view_ref.clone(), view).is_some() {
                    bail!("duplicate view {:?}", view_ref);
                }
            }
            plugins.insert(package_id, metadata);
        }
        expand_feed_patterns(&mut views)?;
        let compiled =
            CompiledConfig::build(views, plugins, raw.defaults, plugin_roots, config_value)?;
        validate_optional_string_requirements(
            &compiled.template_registry,
            default_view.as_deref(),
            EvaluationStage::Bootstrap,
            "default_view",
        )?;
        Ok(Self {
            default_view,
            image_protocol: raw.image_protocol,
            log_file: raw.log_file,
            chrome: raw.chrome,
            input_value: Value::Null,
            invocation_state: StateInstance::default(),
            compiled,
        })
    }

    #[cfg(test)]
    pub(crate) fn test_new(
        default_view: Option<ViewRef>,
        views: BTreeMap<ViewRef, View>,
        plugins: BTreeMap<String, PluginMetadata>,
        plugin_roots: BTreeMap<String, PathBuf>,
        config_value: Value,
    ) -> Result<Self> {
        let compiled = CompiledConfig::build(
            views,
            plugins,
            Defaults::default(),
            plugin_roots,
            config_value,
        )?;
        Ok(Self {
            default_view,
            image_protocol: ImageProtocol::default(),
            log_file: None,
            chrome: ChromeConfig::default(),
            input_value: Value::Null,
            invocation_state: StateInstance::default(),
            compiled,
        })
    }

    #[cfg(test)]
    pub(crate) fn rebuild_template_registry(&mut self) -> Result<()> {
        self.test_rebuild_compiled()
    }

    #[cfg(test)]
    pub(crate) fn test_rebuild_compiled(&mut self) -> Result<()> {
        self.compiled = CompiledConfig::build(
            self.compiled.views.clone(),
            self.compiled.plugins.clone(),
            self.compiled.defaults.clone(),
            self.compiled.plugin_roots.clone(),
            self.compiled.config_value.clone(),
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub fn validate(&self) -> Result<()> {
        let engines = crate::engine::EngineRegistry::new();
        self.validate_with_engines(&engines)
    }

    pub(crate) fn validate_with_engines(
        &self,
        engines: &crate::engine::EngineRegistry,
    ) -> Result<()> {
        if let Some(default_view) = &self.default_view {
            self.engine(default_view)?;
        }

        engines.validate_defaults(&self.defaults)?;
        if let Some(bindings) = &self.defaults.picker.bindings {
            validate_toml_requirements(
                &self.template_registry,
                bindings,
                EvaluationStage::Operation,
                "root picker defaults",
            )?;
        }
        if let Some(bindings) = &self.defaults.capture.bindings {
            validate_toml_requirements(
                &self.template_registry,
                bindings,
                EvaluationStage::Operation,
                "root capture defaults",
            )?;
        }
        for (package_id, plugin) in &self.plugins {
            if plugin.name.trim().is_empty() {
                bail!("plugin package {:?} has an empty name", package_id);
            }
        }

        let mut footer_keys = BTreeMap::new();
        let mut footer_overflow = 0;
        for (id, binding) in &self.chrome.footer.bindings {
            if !matches!(binding.action, CommandAction::Call { .. }) {
                bail!("chrome footer binding {id:?} must use a call action");
            }
            validate_optional_string_requirements(
                &self.template_registry,
                Some(&binding.key),
                EvaluationStage::Bootstrap,
                &format!("chrome footer binding {id:?} key"),
            )?;
            validate_optional_string_requirements(
                &self.template_registry,
                Some(&binding.label),
                EvaluationStage::Bootstrap,
                &format!("chrome footer binding {id:?} label"),
            )?;
            let key = normalize_key(&binding.key)?;
            if let Some(previous) = footer_keys.insert(key.clone(), id) {
                bail!(
                    "chrome footer bindings {:?} and {:?} both use key {:?}",
                    previous,
                    id,
                    key
                );
            }
            if binding.visibility == ChromeBindingVisibility::Overflow {
                footer_overflow += 1;
            }
            validate_command_action(
                self.default_view.as_deref().unwrap_or("<root>"),
                &format!("chrome:footer:{id}"),
                &binding.action,
                &self.views,
                None,
                0,
            )?;
            validate_command_action_requirements(
                &self.template_registry,
                &binding.action,
                EvaluationStage::Operation,
                &format!("chrome footer binding {id:?}"),
                0,
            )?;
        }
        if footer_overflow > 1 {
            bail!("chrome footer can define at most one overflow binding");
        }

        let mut aliases = BTreeMap::<&str, &str>::new();
        for (view_ref, view) in &self.views {
            validate_view_ref(view_ref)?;
            if let Some(alias) = &view.alias {
                if alias.trim().is_empty()
                    || alias.contains(':')
                    || alias.chars().any(char::is_whitespace)
                {
                    bail!("view {:?} has an invalid alias {:?}", view_ref, alias);
                }
                if let Some(previous) = aliases.insert(alias, view_ref) {
                    bail!(
                        "view alias {:?} is assigned to both {:?} and {:?}",
                        alias,
                        previous,
                        view_ref
                    );
                }
            }
            let feeds = view.selected_feeds();
            let items = view.selected_items();
            if !feeds.is_empty() && items.is_some() {
                bail!("feeds view {:?} cannot define items", view_ref);
            }
            let engine = self.engine(view_ref)?;
            engines.validate_config(view_ref, view)?;
            self.validate_view_operation_requirements(view_ref, view)?;
            if engine != ENGINE_PICKER && (!feeds.is_empty() || items.is_some()) {
                bail!(
                    "view {:?} using engine {:?} cannot provide picker items",
                    view_ref,
                    engine
                );
            }
            if let Some(items) = items {
                validate_toml_requirements(
                    &self.template_registry,
                    items,
                    EvaluationStage::Operation,
                    &format!("view {view_ref:?} items"),
                )?;
                validate_items_source_config(items, self.plugin_root(view_ref)).with_context(
                    || format!("view {:?} has invalid items source configuration", view_ref),
                )?;
            }
            if engine == ENGINE_CAPTURE
                && let Some(output) = view.engine_field("output")
                && output.is_table()
            {
                let spec = ScriptSourceSpec::parse(output)
                    .with_context(|| format!("view {:?} has invalid capture source", view_ref))?;
                spec.validate_capture_source()
                    .with_context(|| format!("view {:?} has invalid capture source", view_ref))?;
                let root = self
                    .plugin_root(view_ref)
                    .with_context(|| format!("view {:?} has no plugin root", view_ref))?;
                spec.validate_target(root).with_context(|| {
                    format!("view {:?} has invalid capture source target", view_ref)
                })?;
            }

            let mut seen_feeds = BTreeSet::new();
            for feed in feeds {
                let feed_ref = &feed.view;
                if !seen_feeds.insert(feed_ref.clone()) {
                    bail!(
                        "view {:?} lists feed {:?} more than once",
                        view_ref,
                        feed_ref
                    );
                }
                let feed_view = self.views.get(feed_ref).with_context(|| {
                    format!("view {:?} references missing feed {:?}", view_ref, feed_ref)
                })?;
                if self.engine(feed_ref)? != ENGINE_PICKER {
                    bail!(
                        "view {:?} feed {:?} does not use the picker engine",
                        view_ref,
                        feed_ref
                    );
                }
                if feed_view.is_feeds_page() {
                    bail!(
                        "view {:?} cannot use feeds view {:?} as a feed",
                        view_ref,
                        feed_ref
                    );
                }
                if feed_view.selected_items().is_none() {
                    bail!("view {:?} feed {:?} must define items", view_ref, feed_ref);
                }
            }

            let mut keys = BTreeMap::new();
            for (command_id, command) in &view.commands {
                if command.label.trim().is_empty() {
                    bail!(
                        "view {:?} command {:?} has an empty label",
                        view_ref,
                        command_id
                    );
                }
                let key = normalize_key(&command.key)
                    .with_context(|| format!("view {:?} command {:?}", view_ref, command_id))?;
                if keys.insert(key.clone(), command_id).is_some() {
                    bail!("view {:?} has duplicate command key {:?}", view_ref, key);
                }
                validate_command_action(
                    view_ref,
                    command_id,
                    &command.action,
                    &self.views,
                    self.plugin_root(view_ref),
                    0,
                )?;
            }
        }

        Ok(())
    }

    fn validate_view_operation_requirements(&self, view_ref: &str, view: &View) -> Result<()> {
        for (field, value) in view.selected_engine_config() {
            validate_toml_requirements(
                &self.template_registry,
                value,
                EvaluationStage::Operation,
                &format!("view {view_ref:?} engine field {field:?}"),
            )?;
        }
        if let Some(keymap) = &view.keymap {
            validate_toml_requirements(
                &self.template_registry,
                keymap,
                EvaluationStage::Operation,
                &format!("view {view_ref:?} keymap"),
            )?;
        }
        validate_optional_string_requirements(
            &self.template_registry,
            view.run_shell.as_deref(),
            EvaluationStage::Operation,
            &format!("view {view_ref:?} run_shell"),
        )?;
        for (command_id, command) in &view.commands {
            validate_command_action_requirements(
                &self.template_registry,
                &command.action,
                EvaluationStage::Operation,
                &format!("view {view_ref:?} command {command_id:?}"),
                0,
            )?;
        }
        Ok(())
    }

    pub fn view(&self, view_ref: &str) -> Option<&View> {
        self.views.get(view_ref)
    }

    pub(crate) fn resolve_view(&self, selector: &str) -> Result<ViewRef> {
        if self.views.contains_key(selector) {
            return Ok(selector.to_string());
        }
        let matches = self
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
            .views
            .get(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?;
        Ok(view.selected_engine_type())
    }

    pub(crate) fn get(
        &self,
        source: ConfigSource<'_>,
        snapshot: &EvaluationSnapshot<'_>,
        stage: EvaluationStage,
        path: &[&str],
    ) -> Result<Option<Value>> {
        let raw = self.source_config_value(source)?;
        let Some(value) = path
            .iter()
            .try_fold(raw.as_ref(), |value, segment| value.get(segment))
        else {
            return Ok(None);
        };
        self.resolve_json_value(snapshot, stage, value).map(Some)
    }

    pub(crate) fn evaluate_value(
        &self,
        snapshot: &EvaluationSnapshot<'_>,
        stage: EvaluationStage,
        value: &toml::Value,
    ) -> Result<Value> {
        let value = toml_to_json(value)?;
        self.resolve_json_value(snapshot, stage, &value)
    }

    pub(crate) fn evaluate_argv(
        &self,
        configured: Option<&toml::Value>,
        snapshot: &EvaluationSnapshot<'_>,
        stage: EvaluationStage,
        label: &str,
    ) -> Result<Vec<String>> {
        let Some(configured) = configured else {
            return Ok(Vec::new());
        };
        let value = self.evaluate_value(snapshot, stage, configured)?;
        crate::script_runner::resolve_argv(Some(&value), label)
    }

    fn source_config_value(&self, source: ConfigSource<'_>) -> Result<Cow<'_, Value>> {
        match source {
            ConfigSource::Root => Ok(Cow::Borrowed(&self.config_value)),
            ConfigSource::View(view_ref) => match self.view_config(view_ref) {
                Ok(value) => Ok(Cow::Borrowed(value)),
                Err(_)
                    if self
                        .config_value
                        .as_object()
                        .is_some_and(|value| value.is_empty()) =>
                {
                    Ok(Cow::Owned(self.fallback_view_config_value(view_ref)?))
                }
                Err(error) => Err(error),
            },
        }
    }

    fn fallback_view_config_value(&self, view_ref: &str) -> Result<Value> {
        let view = self
            .view(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?;
        let mut values = serde_json::Map::new();
        if let Some(keymap) = &view.keymap {
            values.insert("keymap".to_string(), toml_to_json(keymap)?);
        }
        if let Some(items) = view.selected_items() {
            values.insert("items".to_string(), toml_to_json(items)?);
        }
        for (name, value) in view.selected_engine_config() {
            values.insert(name.clone(), toml_to_json(value)?);
        }
        Ok(Value::Object(values))
    }

    fn resolve_json_value(
        &self,
        snapshot: &EvaluationSnapshot<'_>,
        stage: EvaluationStage,
        value: &Value,
    ) -> Result<Value> {
        let requirements = self.template_registry.requirements_for_value(value)?;
        requirements.validate_stage(stage, "runtime configuration value")?;
        let root = if requirements.is_empty() {
            Value::Null
        } else {
            snapshot.expression_context(self, &requirements)?
        };
        let evaluator = EvalContext {
            root: &root,
            cancellation: snapshot.cancellation,
            templates: Some(&self.template_registry),
        };
        evaluate_json_value(value, &evaluator)
    }

    pub fn plugin_root(&self, view_ref: &str) -> Option<&Path> {
        let plugin = package_id(view_ref);
        self.plugin_roots.get(plugin).map(PathBuf::as_path)
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
        self.state_registry
            .bind_feed_input(&mut state, query_input)?;
        self.validate_query_state(&state)?;
        Ok(state)
    }
}

fn views_config_value(
    views: &BTreeMap<ViewRef, View>,
    plugins: &BTreeMap<String, PluginMetadata>,
) -> Result<Value> {
    let mut plugin_values = serde_json::Map::new();
    for (package, metadata) in plugins {
        plugin_values.insert(
            package.clone(),
            serde_json::json!({"name": metadata.name, "views": {}}),
        );
    }
    for (view_ref, view) in views {
        let (package, name) = view_ref
            .split_once(':')
            .with_context(|| format!("test view {:?} is not namespaced", view_ref))?;
        let plugin = plugin_values
            .entry(package.to_string())
            .or_insert_with(|| serde_json::json!({"name": package, "views": {}}));
        let views = plugin
            .get_mut("views")
            .and_then(Value::as_object_mut)
            .context("test plugin views must be an object")?;
        views.insert(name.to_string(), serde_json::to_value(view)?);
    }
    let mut value = serde_json::json!({"plugins": plugin_values});
    remove_null_fields(&mut value);
    normalize_engine_configs(&mut value);
    Ok(value)
}

fn remove_null_fields(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                remove_null_fields(value);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                remove_null_fields(value);
            }
            values.retain(|_, value| !value.is_null());
        }
        _ => {}
    }
}

#[cfg(test)]
pub(crate) fn load_test_fixture() -> Result<Config> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config/config.toml");
    Config::load(&path)
}

fn public_page_context(
    runtime: &Value,
    requirements: &ContextRequirements,
    cancellation: Option<&CancellationToken>,
) -> Result<Value> {
    let current = runtime.pointer("/view/current").and_then(Value::as_object);
    let mut page = serde_json::Map::new();
    let fields = [
        ("ref", Value::Null),
        ("input", Value::String(String::new())),
        ("raw_input", Value::String(String::new())),
        ("query", Value::Null),
        ("state_revision", Value::Null),
        ("items", serde_json::json!([])),
        ("selected_item", Value::Null),
        ("command_owner", Value::Null),
    ];
    for (field, default) in fields {
        if requirements.requires_field(Namespace::Page, field) {
            let value = current
                .and_then(|values| values.get(field))
                .unwrap_or(&default);
            page.insert(
                field.to_string(),
                clone_json_value_bounded(value, cancellation)?,
            );
        }
    }
    if requirements.requires_field(Namespace::Page, "commands") {
        let default = Value::Array(Vec::new());
        let value = current
            .and_then(|values| values.get("command").or_else(|| values.get("commands")))
            .unwrap_or(&default);
        page.insert(
            "commands".to_string(),
            clone_json_value_bounded(value, cancellation)?,
        );
    }
    Ok(Value::Object(page))
}

fn public_session_context(
    runtime: &Value,
    requirements: &ContextRequirements,
    cancellation: Option<&CancellationToken>,
) -> Result<Value> {
    let session = runtime.get("session").and_then(Value::as_object);
    let mut public = serde_json::Map::new();
    if requirements.requires_field(Namespace::Session, "input") {
        let default = Value::Object(serde_json::Map::new());
        let value = session
            .and_then(|values| values.get("input"))
            .unwrap_or(&default);
        public.insert(
            "input".to_string(),
            clone_json_value_bounded(value, cancellation)?,
        );
    }
    if requirements.requires_field(Namespace::Session, "views") {
        let default = Value::Array(Vec::new());
        let value = session
            .and_then(|values| values.get("views"))
            .unwrap_or(&default);
        public.insert(
            "views".to_string(),
            clone_json_value_bounded(value, cancellation)?,
        );
    }
    Ok(Value::Object(public))
}

fn package_id(view_ref: &str) -> &str {
    view_ref
        .split_once(':')
        .map(|(package, _)| package)
        .unwrap_or(view_ref)
}

fn expand_feed_patterns(views: &mut BTreeMap<ViewRef, View>) -> Result<()> {
    let view_refs = views.keys().cloned().collect::<Vec<_>>();
    let picker_views = views
        .iter()
        .filter(|(_, view)| view.selected_engine_type() == ENGINE_PICKER)
        .map(|(view_ref, _)| view_ref.clone())
        .collect::<BTreeSet<_>>();
    for (owner_ref, owner) in views.iter_mut() {
        let mut expanded = Vec::new();
        for feed in &owner.engine.config.feeds {
            if is_dynamic_string(&feed.view) {
                let value = Value::String(feed.view.clone());
                let templates = TemplateRegistry::compile_json_tree(&value)?;
                validate_json_requirements(
                    &templates,
                    &value,
                    EvaluationStage::Bootstrap,
                    &format!("view {owner_ref:?} feed target"),
                )?;
            }
            if let Some(view_name) = feed.view.strip_prefix("*:") {
                if view_name.is_empty() || view_name.contains(':') {
                    bail!(
                        "view {:?} has invalid feed pattern {:?}; expected *:view",
                        owner_ref,
                        feed.view
                    );
                }
                for candidate in &view_refs {
                    if candidate != owner_ref
                        && candidate
                            .split_once(':')
                            .is_some_and(|(_, name)| name == view_name)
                        && picker_views.contains(candidate)
                    {
                        expanded.push(FeedSpec {
                            view: candidate.clone(),
                        });
                    }
                }
            } else if feed.view.contains('*') {
                bail!(
                    "view {:?} has invalid feed pattern {:?}; only *:view is supported",
                    owner_ref,
                    feed.view
                );
            } else {
                expanded.push(feed.clone());
            }
        }
        owner.engine.config.feeds = expanded;
    }
    Ok(())
}

fn qualify_view_ref(plugin: &str, view: &str) -> Result<ViewRef> {
    if plugin.trim().is_empty()
        || view.trim().is_empty()
        || plugin.contains(':')
        || view.contains(':')
    {
        bail!("invalid view reference components {:?}:{:?}", plugin, view);
    }
    Ok(format!("{}:{}", plugin, view))
}

fn validate_view_ref(view_ref: &str) -> Result<()> {
    let Some((plugin, view)) = view_ref.split_once(':') else {
        bail!("view reference {:?} must use plugin:view form", view_ref);
    };
    if plugin.is_empty() || view.is_empty() || view.contains(':') {
        bail!("invalid view reference {:?}", view_ref);
    }
    Ok(())
}

pub fn normalize_key(key: &str) -> Result<String> {
    Key::parse_binding(key)?
        .binding_name()
        .with_context(|| format!("unsupported command key {:?}", key))
}

fn reject_root_plugins(user_config: &toml::Value) -> Result<()> {
    if user_config.get("plugins").is_some() {
        bail!(
            "root configuration cannot define plugins; use the sibling plugins/<id>/plugin.toml files"
        );
    }
    Ok(())
}

fn disabled_plugins(user_config: Option<&toml::Value>) -> Result<BTreeSet<String>> {
    let mut disabled = BTreeSet::new();
    let Some(table) = user_config.and_then(toml::Value::as_table) else {
        return Ok(disabled);
    };

    if table.contains_key("plugin_dirs") {
        bail!("plugin_dirs is no longer supported; use the config directory plugins/ path");
    }
    if let Some(value) = table.get("disabled_plugins") {
        let entries = value
            .as_array()
            .with_context(|| "disabled_plugins must be an array of plugin IDs")?;
        for entry in entries {
            let entry = entry
                .as_str()
                .with_context(|| "disabled_plugins entries must be strings")?;
            disabled.insert(entry.to_string());
        }
    }

    Ok(disabled)
}

fn load_plugin_packages(
    merged: &mut toml::Value,
    directory: &Path,
    disabled: &BTreeSet<String>,
) -> Result<BTreeMap<String, PathBuf>> {
    if !directory.exists() {
        return Ok(BTreeMap::new());
    }
    if !directory.is_dir() {
        bail!("plugin path {:?} is not a directory", directory);
    }

    let mut manifests = Vec::new();
    for entry in fs::read_dir(directory)
        .with_context(|| format!("could not read plugin directory {}", directory.display()))?
    {
        let entry = entry
            .with_context(|| format!("could not read plugin directory {}", directory.display()))?;
        let path = entry.path();
        if path.is_dir() {
            let manifest = path.join("plugin.toml");
            if manifest.is_file() {
                manifests.push(manifest);
            }
        }
    }
    manifests.sort();

    let mut roots = BTreeMap::new();
    for manifest in manifests {
        let plugin_id = manifest
            .parent()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .with_context(|| {
                format!(
                    "plugin directory for {} has no valid name",
                    manifest.display()
                )
            })?;
        if disabled.contains(plugin_id) {
            continue;
        }
        let (plugin_id, package) = read_plugin_package(&manifest)?;
        let root = manifest
            .parent()
            .expect("plugin manifest has a parent directory")
            .to_path_buf();
        if roots.insert(plugin_id.clone(), root).is_some() {
            bail!(
                "duplicate plugin {:?} in the user plugin directory",
                plugin_id
            );
        }
        merge_values(merged, package);
    }
    Ok(roots)
}

fn read_plugin_package(manifest: &Path) -> Result<(String, toml::Value)> {
    let root = manifest
        .parent()
        .with_context(|| format!("plugin manifest {} has no parent", manifest.display()))?;
    let plugin_id = root
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| {
            format!(
                "plugin directory for {} has no valid name",
                manifest.display()
            )
        })?;
    validate_plugin_id(plugin_id)?;

    let source = fs::read_to_string(manifest)
        .with_context(|| format!("could not read plugin manifest {}", manifest.display()))?;
    let mut value: toml::Value = toml::from_str(&source)
        .with_context(|| format!("could not parse plugin manifest {}", manifest.display()))?;
    let header_value = value
        .get("plugin")
        .cloned()
        .with_context(|| format!("plugin manifest {} is missing [plugin]", manifest.display()))?;
    let header: PluginHeader = header_value
        .try_into()
        .with_context(|| format!("invalid [plugin] in {}", manifest.display()))?;
    if header.api != 1 {
        bail!(
            "plugin {:?} uses unsupported API version {}",
            plugin_id,
            header.api
        );
    }

    let table = value
        .as_table_mut()
        .context("plugin manifest root is not a table")?;
    table.remove("plugin");
    let mut views = table
        .remove("views")
        .with_context(|| format!("plugin {:?} is missing [views.*]", plugin_id))?;
    normalize_view_keymaps(&mut views, plugin_id)?;

    let mut plugin_table = toml::map::Map::new();
    plugin_table.insert("name".to_string(), toml::Value::String(header.name));
    plugin_table.insert("views".to_string(), views);
    let mut plugins_table = toml::map::Map::new();
    plugins_table.insert(plugin_id.to_string(), toml::Value::Table(plugin_table));
    let mut package_table = toml::map::Map::new();
    package_table.insert("plugins".to_string(), toml::Value::Table(plugins_table));
    Ok((plugin_id.to_string(), toml::Value::Table(package_table)))
}

fn remove_disabled_plugins(value: &mut toml::Value, disabled: &BTreeSet<String>) {
    if let Some(plugins) = value.get_mut("plugins").and_then(toml::Value::as_table_mut) {
        for plugin_id in disabled {
            plugins.remove(plugin_id);
        }
    }
}

fn normalize_keymap_tables(value: &mut toml::Value) -> Result<()> {
    let Some(plugins) = value.get_mut("plugins").and_then(toml::Value::as_table_mut) else {
        return Ok(());
    };
    for (plugin_id, plugin) in plugins {
        let Some(views) = plugin.get_mut("views").and_then(toml::Value::as_table_mut) else {
            continue;
        };
        for (view_name, view) in views {
            let Some(keymap) = view
                .as_table_mut()
                .and_then(|view| view.get_mut("keymap"))
                .and_then(toml::Value::as_table_mut)
            else {
                continue;
            };
            normalize_keymap_table(keymap, &format!("view {plugin_id}:{view_name}"))?;
        }
    }
    Ok(())
}

fn normalize_view_keymaps(value: &mut toml::Value, plugin_id: &str) -> Result<()> {
    let views = value
        .as_table_mut()
        .context("plugin views must be a table")?;
    for (view_name, view) in views {
        let Some(keymap) = view
            .as_table_mut()
            .and_then(|view| view.get_mut("keymap"))
            .and_then(toml::Value::as_table_mut)
        else {
            continue;
        };
        normalize_keymap_table(keymap, &format!("view {plugin_id}:{view_name}"))?;
    }
    Ok(())
}

fn normalize_keymap_table(
    table: &mut toml::map::Map<String, toml::Value>,
    label: &str,
) -> Result<()> {
    let entries = std::mem::replace(table, toml::map::Map::new());
    let mut normalized = toml::map::Map::new();
    let mut source_by_key = BTreeMap::new();
    for (source, value) in entries {
        let key = Key::parse_binding(&source)
            .with_context(|| format!("{label} keymap binding {:?}", source))?
            .binding_name()
            .with_context(|| {
                format!("{label} keymap binding {:?} has no canonical name", source)
            })?;
        if let Some(previous) = source_by_key.insert(key.clone(), source.clone()) {
            bail!(
                "{label} keymap bindings {:?} and {:?} normalize to the same key {:?}",
                previous,
                source,
                key
            );
        }
        normalized.insert(key, value);
    }
    *table = normalized;
    Ok(())
}

fn merge_values(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                if let Some(existing) = base.get_mut(&key) {
                    merge_values(existing, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

pub(crate) fn toml_to_json(value: &toml::Value) -> Result<Value> {
    serde_json::to_value(value).context("could not convert TOML to JSON")
}

fn normalize_engine_configs(config: &mut Value) {
    let Some(plugins) = config.get_mut("plugins").and_then(Value::as_object_mut) else {
        return;
    };
    for plugin in plugins.values_mut() {
        let Some(views) = plugin.get_mut("views").and_then(Value::as_object_mut) else {
            continue;
        };
        for view in views.values_mut() {
            let Some(view) = view.as_object_mut() else {
                continue;
            };
            let Some(engine) = view.get("engine").cloned() else {
                continue;
            };
            let Some(engine) = engine.as_object() else {
                continue;
            };
            let Some(engine_type) = engine.get("type").cloned() else {
                continue;
            };
            let Some(config) = engine.get("config").and_then(Value::as_object) else {
                continue;
            };
            view.insert("type".to_string(), engine_type);
            for (key, value) in config {
                view.insert(key.clone(), value.clone());
            }
        }
    }
}

fn validate_plugin_id(plugin_id: &str) -> Result<()> {
    if plugin_id.is_empty() || plugin_id.contains(':') || plugin_id.chars().any(char::is_whitespace)
    {
        bail!("plugin directory {:?} is not a valid package ID", plugin_id);
    }
    Ok(())
}

fn validate_run_handler(
    handler: &toml::Value,
    script_root: Option<&Path>,
    owner: &str,
) -> Result<()> {
    let spec = match handler {
        toml::Value::Table(_) => {
            let spec = ScriptSourceSpec::parse(handler)
                .with_context(|| format!("{} has an invalid command handler source", owner))?;
            spec.validate_command_handler()
                .with_context(|| format!("{} has an invalid command handler source", owner))?;
            spec
        }
        toml::Value::String(source) if is_dynamic_string(source) => {
            let template = Template::parse(source)
                .with_context(|| format!("{} has an invalid command handler source", owner))?;
            anyhow::ensure!(
                template.is_complete_path(),
                "{} command handler must be a script source table or complete dynamic path",
                owner
            );
            return Ok(());
        }
        _ => {
            bail!(
                "{} command handler must be a script source table or complete dynamic path",
                owner
            )
        }
    };
    let Some(file) = spec.file.as_str() else {
        bail!(
            "{} command handler file must be a string or dynamic path",
            owner
        );
    };
    if file.trim().is_empty() {
        bail!("{} has an empty command handler file", owner);
    }
    if is_dynamic_string(file) {
        return Ok(());
    }
    let root = script_root
        .with_context(|| format!("{} file-backed command handler has no plugin root", owner))?;
    spec.validate_target(root)
        .with_context(|| format!("{} has an invalid command handler file", owner))
}

fn validate_argv_arguments(arguments: &toml::Value, owner: &str) -> Result<()> {
    validate_script_source_args(Some(arguments))
        .with_context(|| format!("{} args must be an array or complete dynamic path", owner))
}

fn validate_run_arguments(arguments: &toml::Value, owner: &str) -> Result<()> {
    validate_argv_arguments(arguments, &format!("{} command", owner))
}

fn validate_command_action_requirements(
    templates: &TemplateRegistry,
    action: &CommandAction,
    stage: EvaluationStage,
    consumer: &str,
    depth: usize,
) -> Result<()> {
    if depth > 16 {
        bail!("{consumer} action nesting exceeds 16 levels");
    }
    let validate = |value: &toml::Value, label: &str, value_stage: EvaluationStage| {
        validate_toml_requirements(templates, value, value_stage, label)
    };
    match action {
        CommandAction::Run { payload } => {
            validate(&payload.handler, &format!("{consumer} handler"), stage)?;
            if let Some(args) = &payload.args {
                validate(args, &format!("{consumer} args"), stage)?;
            }
            if let Some(shell) = &payload.shell {
                validate(
                    &toml::Value::String(shell.clone()),
                    &format!("{consumer} shell"),
                    stage,
                )?;
            }
        }
        CommandAction::Navigate { payload } => {
            validate(&payload.target, &format!("{consumer} target"), stage)?;
            if let Some(query) = &payload.query {
                validate(query, &format!("{consumer} query"), stage)?;
            }
        }
        CommandAction::Call { payload } => {
            validate(&payload.target, &format!("{consumer} target"), stage)?;
            if let Some(query) = &payload.query {
                validate(query, &format!("{consumer} query"), stage)?;
            }
            if let Some(then) = &payload.then {
                validate_command_action_requirements(
                    templates,
                    then,
                    EvaluationStage::Return,
                    &format!("{consumer} continuation"),
                    depth + 1,
                )?;
            }
        }
        CommandAction::Return { payload } => {
            if let Some(value) = &payload.value {
                validate(value, &format!("{consumer} value"), stage)?;
            }
            if let Some(handler) = &payload.handler {
                validate(
                    handler,
                    &format!("{consumer} handler"),
                    EvaluationStage::Return,
                )?;
            }
            if let Some(args) = &payload.args {
                validate(args, &format!("{consumer} args"), EvaluationStage::Return)?;
            }
        }
        CommandAction::EditInput { payload } => {
            validate(&payload.value, &format!("{consumer} value"), stage)?;
            if let Some(cursor) = &payload.cursor {
                validate(cursor, &format!("{consumer} cursor"), stage)?;
            }
        }
        CommandAction::Invoke { payload } => {
            validate(&payload.command, &format!("{consumer} command"), stage)?;
        }
    }
    Ok(())
}

fn validate_command_action(
    view_ref: &str,
    command_id: &str,
    action: &CommandAction,
    views: &BTreeMap<ViewRef, View>,
    script_root: Option<&Path>,
    depth: usize,
) -> Result<()> {
    if depth > 16 {
        bail!(
            "view {:?} command {:?} action nesting exceeds 16 levels",
            view_ref,
            command_id
        );
    }
    let owner = format!("{}:{}", view_ref, command_id);
    match action {
        CommandAction::Run { payload } => {
            validate_run_handler(&payload.handler, script_root, &owner)?;
            if let Some(args) = &payload.args {
                validate_run_arguments(args, &owner)?;
            }
        }
        CommandAction::Navigate { payload } => {
            validate_target(view_ref, command_id, "navigation", &payload.target, views)?;
            if let Some(query) = &payload.query {
                validate_templates(query)?;
            }
        }
        CommandAction::Call { payload } => {
            validate_target(view_ref, command_id, "call", &payload.target, views)?;
            if let Some(value) = &payload.query {
                validate_templates(value)?;
            }
            if let Some(then) = &payload.then {
                validate_command_action(view_ref, command_id, then, views, script_root, depth + 1)?;
            }
        }
        CommandAction::Return { payload } => {
            if depth > 0 && (payload.handler.is_some() || payload.args.is_some()) {
                bail!(
                    "view {:?} command {:?} continuation return cannot define handler or args",
                    view_ref,
                    command_id
                );
            }
            if payload.handler.is_none() && payload.args.is_some() {
                bail!(
                    "view {:?} command {:?} return args require a handler",
                    view_ref,
                    command_id
                );
            }
            if let Some(value) = &payload.value {
                validate_templates(value)?;
            }
            if let Some(handler) = &payload.handler {
                validate_result_handler(handler, &owner, script_root)?;
            }
            if let Some(args) = &payload.args {
                validate_argv_arguments(
                    args,
                    &format!("view {:?} command {:?}", view_ref, command_id),
                )?;
            }
        }
        CommandAction::EditInput { payload } => {
            validate_templates(&payload.value)?;
            if let Some(cursor) = &payload.cursor {
                validate_templates(cursor)?;
            }
        }
        CommandAction::Invoke { payload } => validate_templates(&payload.command)?,
    }
    Ok(())
}

fn validate_target(
    view_ref: &str,
    command_id: &str,
    kind: &str,
    target: &toml::Value,
    views: &BTreeMap<ViewRef, View>,
) -> Result<()> {
    validate_templates(target).with_context(|| {
        format!(
            "view {:?} command {:?} has invalid {} payload",
            view_ref, command_id, kind
        )
    })?;
    let target = target
        .as_str()
        .with_context(|| format!("{} target must be a string or dynamic path", kind))?;
    let configured = views.contains_key(target)
        || views
            .values()
            .any(|view| view.alias.as_deref() == Some(target));
    if !is_dynamic_string(target) && !configured {
        bail!(
            "view {:?} command {:?} references missing view {:?}",
            view_ref,
            command_id,
            target
        );
    }
    Ok(())
}

fn validate_result_handler(
    handler: &toml::Value,
    owner: &str,
    script_root: Option<&Path>,
) -> Result<()> {
    let handler = handler
        .as_str()
        .with_context(|| format!("{} result handler must be a string or dynamic path", owner))?;
    if is_dynamic_string(handler) {
        Template::parse(handler)?;
        return Ok(());
    }
    let path = Path::new(handler);
    if handler.trim().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        bail!("{} has invalid result handler {:?}", owner, handler);
    }
    if let Some(root) = script_root {
        crate::script_runner::validate_script_target(root, handler)
            .with_context(|| format!("{} has an invalid result handler target", owner))?;
    }
    Ok(())
}

fn validate_templates(value: &toml::Value) -> Result<()> {
    match value {
        toml::Value::String(source) => {
            Template::parse(source)?;
        }
        toml::Value::Array(values) => {
            for value in values {
                validate_templates(value)?;
            }
        }
        toml::Value::Table(values) => {
            for value in values.values() {
                validate_templates(value)?;
            }
        }
        toml::Value::Boolean(_)
        | toml::Value::Datetime(_)
        | toml::Value::Float(_)
        | toml::Value::Integer(_) => {}
    }
    Ok(())
}

fn validate_items_source_config(value: &toml::Value, root: Option<&Path>) -> Result<()> {
    match value {
        toml::Value::Array(_) => Ok(()),
        toml::Value::String(source) => {
            let template = Template::parse(source)?;
            if template.is_complete_path() {
                Ok(())
            } else {
                bail!("items must be an array, complete dynamic path, or script source object")
            }
        }
        toml::Value::Table(_) => {
            let spec = ScriptSourceSpec::parse(value)
                .context("items must be an array or a script source object")?;
            spec.validate_picker_source()?;
            if let Some(root) = root {
                spec.validate_target(root)?;
            }
            Ok(())
        }
        _ => bail!("items must be an array, complete dynamic path, or script source object"),
    }
}

fn resolve_config_path(config_path: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    }
}

fn default_engine_type() -> String {
    ENGINE_PICKER.to_string()
}

fn default_plugin_api() -> u32 {
    1
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
        assert!(!config.views.contains_key("ghost:main"));
        assert!(config.views.contains_key("core:default"));
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
            config.views["apps:main"].engine.engine_type,
            ENGINE_PICKER.to_string()
        );
        assert_eq!(
            config.views["core:default"].selected_feeds()[0].view,
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
    fn legacy_viewtypes_are_rejected() {
        let value: toml::Value = toml::from_str(
            r#"
            [viewtypes.picker.engine]
            type = "picker"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
"#,
        )
        .unwrap();
        let raw: RawConfig = value.try_into().unwrap();
        let error = Config::from_raw(raw, BTreeMap::new(), Value::Object(serde_json::Map::new()))
            .expect_err("legacy viewtypes should be rejected");
        assert!(error.to_string().contains("no longer supported"));
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
    fn legacy_sources_field_is_rejected() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            sources = ["apps:main"]
            [plugins.apps.views.main]
            [plugins.apps.views.main.engine]
            type = "picker"
            [plugins.apps.views.main.engine.config]
            items = []
"#,
        );
        let error = config
            .validate()
            .expect_err("legacy sources should be rejected");
        assert!(
            error.to_string().contains("unsupported field \"sources\""),
            "error={error}"
        );
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
        assert_eq!(config.plugins["package-a"].name, "template");
        assert_eq!(config.plugins["package-b"].name, "template");
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
        let items = config.views["filetest:main"]
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
        let CommandAction::Run { payload } = &config.views["filetest:main"].commands["run"].action
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
                .config_value
                .pointer("/aa/a/bb")
                .and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            config
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
        config.config_value["defaults"]["picker"]["bindings"] = serde_json::json!({
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
        config.config_value["defaults"]["picker"]["bindings"] = serde_json::json!({
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
        let page = public_page_context(&runtime, &requirements, None).unwrap();
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
        assert!(loaded.config.config_value.get("theme").is_none());
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
            config.views["base:main"]
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
            config.views["base:main"]
                .engine
                .config
                .items
                .as_ref()
                .and_then(toml::Value::as_str),
            Some("{{ view.query }}")
        );
    }
}
