use super::{ViewRef, validate_templates};
use crate::expression::{Template, is_dynamic_string};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn default_engine_type() -> String {
    super::ENGINE_PICKER.to_string()
}

fn default_plugin_api() -> u32 {
    1
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

#[derive(Debug, Clone)]
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

    pub(crate) fn file_value(&self) -> Option<&str> {
        self.file.as_str()
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
        crate::execution::validate_script_target(root, file)
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
        crate::execution::validate_max_output_bytes(source.max_output_bytes)?;
        Ok(Self {
            file: source.file,
            args: source.args,
            max_output_bytes: source.max_output_bytes,
        })
    }

    pub(crate) fn script_args(&self, label: &str) -> Result<Vec<String>> {
        crate::execution::resolve_argv(self.args.as_ref(), label)
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

pub(crate) fn validate_script_source_args(value: Option<&toml::Value>) -> Result<()> {
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
            crate::execution::validate_max_output_bytes(Some(value))
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
    #[serde(default, rename = "query")]
    pub(crate) query: Option<toml::Table>,
    #[serde(default)]
    pub(crate) keymap: Option<toml::Value>,
    #[serde(default)]
    pub commands: BTreeMap<String, Command>,
}

/// Static composition metadata for one configured View.
pub(crate) type ViewDefinition = View;

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
    /// Replace the current View instead of pushing a new stack entry.
    #[serde(default)]
    pub replace: bool,
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
pub(crate) enum CommandBindingVisibility {
    #[default]
    Always,
    Overflow,
    Hidden,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandConfig {
    #[serde(default)]
    pub(crate) bindings: BTreeMap<String, CommandBinding>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CommandBinding {
    #[serde(default)]
    pub(crate) key: Option<String>,
    #[serde(default)]
    pub(crate) label: Option<String>,
    #[serde(default)]
    pub(crate) visibility: Option<CommandBindingVisibility>,
    #[serde(flatten)]
    pub(crate) action: Option<CommandAction>,
}

impl CommandBinding {
    pub(crate) fn builtin_commands() -> Self {
        Self {
            key: None,
            label: None,
            visibility: None,
            action: None,
        }
    }

    pub(crate) fn key(&self, id: &str) -> Option<&str> {
        self.key
            .as_deref()
            .or_else(|| (id == "commands").then_some("ctrl+k"))
    }

    pub(crate) fn label(&self, id: &str) -> Option<&str> {
        self.label
            .as_deref()
            .or_else(|| (id == "commands").then_some("Commands"))
    }

    pub(crate) fn visibility(&self, id: &str) -> Option<CommandBindingVisibility> {
        Some(self.visibility.unwrap_or(if id == "commands" {
            CommandBindingVisibility::Overflow
        } else {
            CommandBindingVisibility::Always
        }))
    }

    pub(crate) fn command_action(&self, id: &str) -> Option<CommandAction> {
        if let Some(action) = &self.action {
            return Some(action.clone());
        }
        if id != "commands" {
            return None;
        }
        let mut query = toml::map::Map::new();
        query.insert(
            "commands".to_string(),
            toml::Value::String("{{ page.commands }}".to_string()),
        );
        Some(CommandAction::Call {
            payload: CallPayload {
                target: toml::Value::String("selectors:commands".to_string()),
                query: Some(toml::Value::Table(query)),
                then: Some(Box::new(CommandAction::Invoke {
                    payload: InvokePayload {
                        command: toml::Value::String("{{ result.output.value }}".to_string()),
                    },
                })),
            },
        })
    }

    pub(crate) fn as_command(&self, id: &str) -> Option<Command> {
        Some(Command {
            key: self.key(id)?.to_string(),
            label: self.label(id)?.to_string(),
            scope: CommandScope::View,
            requires: CommandRequirement::Input,
            passthrough: false,
            action: self.command_action(id)?,
        })
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
    #[serde(default)]
    pub passthrough: bool,
    #[serde(flatten)]
    pub action: CommandAction,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct RawConfig {
    #[serde(default)]
    pub(super) default_view: Option<String>,
    #[serde(default)]
    pub(super) image_protocol: ImageProtocol,
    #[serde(default)]
    pub(super) log_file: Option<PathBuf>,
    #[serde(default)]
    pub(super) commands: CommandConfig,
    #[serde(default)]
    pub(super) theme: Option<String>,
    #[serde(default)]
    pub(super) plugins: BTreeMap<String, Plugin>,
    #[serde(default)]
    pub(super) defaults: Defaults,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Plugin {
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) views: BTreeMap<String, View>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct PluginHeader {
    #[serde(default = "default_plugin_api")]
    pub(super) api: u32,
    pub(super) name: String,
}

#[cfg(test)]
mod tests {
    use super::View;

    #[test]
    fn view_query_deserializes_and_serializes_with_the_compatibility_key() {
        let view: View = toml::from_str(
            r#"
            [engine]
            type = "picker"
            [query]
            type = "string"
            "#,
        )
        .expect("query must deserialize as the compatibility configuration key");

        let serialized = serde_json::to_value(view).expect("View must serialize");
        assert!(serialized.get("query").is_some());
        assert!(serialized.get("parameters").is_none());
    }

    #[test]
    fn view_parameters_key_is_rejected_as_an_unknown_field() {
        let error = toml::from_str::<View>(
            r#"
            [engine]
            type = "picker"
            [parameters]
            type = "string"
            "#,
        )
        .expect_err("parameters is an internal name, not a configuration key");
        assert!(error.to_string().contains("unknown field"));
    }
}
