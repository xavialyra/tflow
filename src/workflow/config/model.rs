use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn default_engine_type() -> String {
    super::ENGINE_PICKER.to_string()
}

fn default_workflow_api() -> u32 {
    1
}

#[derive(Debug, Clone, Default)]
pub struct WorkflowMetadata {
    pub name: String,
    pub entrypoint: Option<String>,
    pub styles: BTreeMap<String, crate::ui::theme::RawStyleBinding>,
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

/// What Backspace does on the empty input line of a non-root Picker that
/// renders a left prefix.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LeftPrefixBackspace {
    /// Return to the parent View, like Escape.
    Parent,
    /// Return to the root View in one step.
    Root,
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
    /// Left-side marker for non-root Picker input lines. Unset renders
    /// nothing, `"$route"` renders the target View's route label/alias, and
    /// any other value is rendered literally. The marker is presentational: it
    /// never changes key handling.
    #[serde(default)]
    pub(crate) left_prefix: Option<String>,
    /// Backspace behavior while the input line is empty. Unset leaves
    /// Backspace inert; it only applies while a left prefix is rendered.
    #[serde(default)]
    pub(crate) left_prefix_backspace: Option<LeftPrefixBackspace>,
    #[serde(default)]
    pub(crate) bindings: Option<toml::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureDefaults {
    #[serde(default)]
    pub(crate) bindings: Option<toml::Value>,
}

/// How a View's own `[views.<name>.keymap]` table is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeymapMode {
    /// The table (or, when absent or empty, the commands' own `key`) is the
    /// whole View scope. The default.
    #[default]
    View,
    /// The table is a base layer; the focused item's `bindings` override it
    /// per physical key, and the command-level `key` fallback does not apply.
    ItemMerge,
}

/// The View's own `[views.<name>.keymap]` bindings, keyed by physical key.
pub type ViewKeymap = BTreeMap<String, toml::Value>;

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct EngineSpec {
    #[serde(rename = "type", default = "default_engine_type")]
    pub engine_type: String,
    #[serde(default)]
    pub config: EngineOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedScriptTarget {
    File(String),
    Inline(String),
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedScriptSource {
    pub(crate) target: ResolvedScriptTarget,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProducerScriptHandler {
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    script: Option<String>,
}

pub(crate) fn parse_producer_script_handler(
    value: &toml::Value,
    script_root: Option<&Path>,
) -> Result<ResolvedScriptSource> {
    let source = parse_producer_script_handler_shape(value)?;
    source.validate_target(script_root)?;
    Ok(source)
}

/// Parse inert handler data when a projection does not carry its workflow root.
/// Configuration validation and mount preparation still validate the target.
pub(crate) fn parse_producer_script_handler_shape(
    value: &toml::Value,
) -> Result<ResolvedScriptSource> {
    let handler: ProducerScriptHandler = value
        .clone()
        .try_into()
        .context("script producer handler must be a table with file or script")?;
    let target = match (handler.file, handler.script) {
        (Some(file), None) if !file.trim().is_empty() => ResolvedScriptTarget::File(file),
        (None, Some(script)) if !script.trim().is_empty() => ResolvedScriptTarget::Inline(script),
        (Some(_), Some(_)) => {
            bail!("script producer handler must define exactly one of file or script")
        }
        (Some(_), None) => bail!("script producer handler file must be non-empty"),
        (None, Some(_)) => bail!("script producer handler script must be non-empty"),
        (None, None) => bail!("script producer handler must define exactly one of file or script"),
    };
    Ok(ResolvedScriptSource { target })
}

impl ResolvedScriptSource {
    pub(crate) fn validate_target(&self, root: Option<&Path>) -> Result<()> {
        let ResolvedScriptTarget::File(file) = &self.target else {
            return Ok(());
        };
        if let Some(root) = root {
            crate::execution::validate_script_target(root, file)
        } else if Path::new(file).is_relative() {
            bail!(
                "single-file workflow cannot reference relative script file {:?}; workflows must be self-contained using inline scripts or system binaries",
                file
            )
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct EngineOptions {
    #[serde(default)]
    pub items: Option<toml::Value>,
    #[serde(flatten)]
    pub fields: toml::Table,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ViewPresentationMode {
    #[default]
    Inline,
    Popup,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ViewPresentation {
    #[serde(default)]
    pub mode: ViewPresentationMode,
    #[serde(default)]
    pub width: Option<u16>,
    #[serde(default)]
    pub height: Option<u16>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub engine: EngineSpec,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub cancel_exit_code: Option<u8>,
    #[serde(default, rename = "query")]
    pub(crate) query: Option<toml::Table>,
    #[serde(default)]
    pub keymap_mode: KeymapMode,
    #[serde(default)]
    pub keymap: Option<ViewKeymap>,
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

    pub(crate) fn engine_field(&self, field: &str) -> Option<&toml::Value> {
        self.selected_engine_config().get(field)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ProducerKind {
    Declared,
    Script,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReturnProcessor {
    #[serde(rename = "type")]
    pub(crate) operation: String,
    pub(crate) producer: ProducerKind,
    pub(crate) handler: toml::Value,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CommandAction {
    #[serde(skip)]
    OpenCommands,
    #[serde(skip)]
    OpenParameters,
    Run {
        producer: ProducerKind,
        handler: toml::Value,
    },
    Navigate {
        producer: ProducerKind,
        handler: toml::Value,
    },
    Call {
        producer: ProducerKind,
        handler: toml::Value,
        #[serde(default)]
        return_processor: Option<ReturnProcessor>,
    },
    Return {
        producer: ProducerKind,
        handler: toml::Value,
    },
}

impl CommandAction {
    pub(crate) fn producer(&self) -> Option<ProducerKind> {
        match self {
            CommandAction::OpenCommands | CommandAction::OpenParameters => None,
            CommandAction::Run { producer, .. }
            | CommandAction::Navigate { producer, .. }
            | CommandAction::Call { producer, .. }
            | CommandAction::Return { producer, .. } => Some(*producer),
        }
    }

    pub(crate) fn operation_type(&self) -> &'static str {
        match self {
            CommandAction::OpenCommands => "open-commands",
            CommandAction::OpenParameters => "open-parameters",
            CommandAction::Run { .. } => "run",
            CommandAction::Navigate { .. } => "navigate",
            CommandAction::Call { .. } => "call",
            CommandAction::Return { .. } => "return",
        }
    }
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

#[derive(Debug, Clone)]
pub(crate) struct CommandBinding {
    pub(crate) key: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) visibility: Option<CommandBindingVisibility>,
    pub(crate) action: Option<CommandAction>,
}

impl<'de> Deserialize<'de> for CommandBinding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = toml::Value::deserialize(deserializer)?;
        let mut table = value
            .as_table()
            .cloned()
            .ok_or_else(|| serde::de::Error::custom("command binding must be a table"))?;
        let key = table
            .remove("key")
            .map(|value| value.try_into::<String>().map_err(serde::de::Error::custom))
            .transpose()?;
        let label = table
            .remove("label")
            .map(|value| value.try_into::<String>().map_err(serde::de::Error::custom))
            .transpose()?;
        let visibility = table
            .remove("visibility")
            .map(|value| {
                value
                    .try_into::<CommandBindingVisibility>()
                    .map_err(serde::de::Error::custom)
            })
            .transpose()?;
        let action = if table.is_empty() {
            None
        } else {
            Some(
                toml::Value::Table(table)
                    .try_into::<CommandAction>()
                    .map_err(serde::de::Error::custom)?,
            )
        };
        Ok(Self {
            key,
            label,
            visibility,
            action,
        })
    }
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

    pub(crate) fn builtin_parameters() -> Self {
        Self {
            key: Some("ctrl+g".to_string()),
            label: Some("Parameters".to_string()),
            visibility: Some(CommandBindingVisibility::Hidden),
            action: Some(CommandAction::OpenParameters),
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
        self.action
            .clone()
            .or_else(|| (id == "commands").then_some(CommandAction::OpenCommands))
    }

    pub(crate) fn as_command(&self, id: &str) -> Option<Command> {
        Some(Command {
            key: self.key(id).map(str::to_string),
            label: self.label(id)?.to_string(),
            action: self.command_action(id)?,
        })
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct Command {
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub label: String,
    #[serde(flatten)]
    pub action: CommandAction,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawConfig {
    #[serde(default)]
    pub(super) default_view: Option<String>,
    #[serde(default)]
    pub(super) entrypoint: Option<String>,
    #[serde(default)]
    pub(super) image_protocol: ImageProtocol,
    #[serde(default)]
    pub(super) log_file: Option<PathBuf>,
    #[serde(default)]
    pub(super) commands: CommandConfig,
    #[serde(default)]
    pub(super) aliases: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) view_aliases: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) workflows: BTreeMap<String, Workflow>,
    #[serde(default)]
    pub(super) defaults: Defaults,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSettings {
    #[serde(default)]
    pub(crate) image_protocol: ImageProtocol,
    #[serde(default)]
    pub(crate) theme: Option<String>,
    #[serde(default)]
    pub(crate) log_file: Option<PathBuf>,
    #[serde(default)]
    pub(crate) defaults: Defaults,
    #[serde(default)]
    pub(crate) styles: BTreeMap<String, BTreeMap<String, crate::ui::theme::RawStyleBinding>>,
}

pub(crate) fn validate_settings_purity(table: &toml::Table, path: &Path) -> Result<()> {
    for forbidden in [
        "default_view",
        "disabled_workflows",
        "workflows",
        "views",
        "suite",
        "workflow",
    ] {
        if table.contains_key(forbidden) {
            bail!(
                "settings file {} violates purity invariant: contains forbidden field {:?}; settings.toml is strictly reserved for passive host environment configuration (ADR 0005)",
                path.display(),
                forbidden
            );
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(super) enum RawSuiteEntrypoint {
    Target(String),
    Detailed {
        target: String,
        #[serde(default)]
        query: Option<toml::Table>,
    },
}

impl RawSuiteEntrypoint {
    pub(super) fn target(&self) -> &str {
        match self {
            Self::Target(t) => t,
            Self::Detailed { target, .. } => target,
        }
    }

    pub(super) fn query(&self) -> Option<&toml::Table> {
        match self {
            Self::Target(_) => None,
            Self::Detailed { query, .. } => query.as_ref(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SuiteHeader {
    #[serde(default = "default_workflow_api")]
    pub(super) api: u32,
    pub(super) name: String,
    pub(super) entrypoint: Option<RawSuiteEntrypoint>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkflowMountSpec {
    #[serde(default)]
    pub(super) file: Option<PathBuf>,
    #[serde(default)]
    pub(super) dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(super) enum WorkflowMount {
    Table(WorkflowMountSpec),
    String(PathBuf),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawSuiteManifest {
    pub(super) suite: SuiteHeader,
    #[serde(default)]
    pub(super) workflows: BTreeMap<String, WorkflowMount>,
    #[serde(default)]
    pub(super) aliases: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) styles: BTreeMap<String, BTreeMap<String, crate::ui::theme::RawStyleBinding>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Workflow {
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) entrypoint: Option<String>,
    #[serde(default)]
    pub(super) views: BTreeMap<String, View>,
    #[serde(default)]
    pub(super) commands: BTreeMap<String, Command>,
    #[serde(default)]
    pub(super) styles: BTreeMap<String, crate::ui::theme::RawStyleBinding>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkflowHeader {
    #[serde(default = "default_workflow_api")]
    pub(super) api: u32,
    pub(super) name: String,
    pub(super) entrypoint: String,
}

#[cfg(test)]
mod tests {
    use super::{
        CommandAction, CommandBinding, CommandBindingVisibility, CommandConfig, ProducerKind,
        ResolvedScriptSource, ResolvedScriptTarget, View, Workflow,
    };

    #[test]
    fn producer_script_handler_requires_one_literal_target() {
        for (source, expected) in [
            (
                r#"file = "/tmp/run.sh""#,
                ResolvedScriptTarget::File("/tmp/run.sh".to_string()),
            ),
            (
                r#"script = "printf '%s\\n' ok""#,
                ResolvedScriptTarget::Inline("printf '%s\\n' ok".to_string()),
            ),
        ] {
            let value: toml::Value = toml::from_str(source).unwrap();
            let resolved = super::parse_producer_script_handler(&value, None).unwrap();
            assert_eq!(resolved.target, expected);
        }

        let ambiguous: toml::Value = toml::from_str(
            r#"file = "scripts/run.sh"
script = "printf ok"
"#,
        )
        .unwrap();
        assert!(super::parse_producer_script_handler(&ambiguous, None).is_err());

        let invalid_source: toml::Value = toml::from_str(
            r#"source = "script"
file = "scripts/run.sh"
"#,
        )
        .unwrap();
        assert!(super::parse_producer_script_handler(&invalid_source, None).is_err());
    }

    #[test]
    fn producer_handler_accepts_inline_scripts_and_rejects_unknown_fields() {
        let value: toml::Value = toml::from_str(
            r#"script = "printf 'ok'"
"#,
        )
        .unwrap();
        let resolved = super::parse_producer_script_handler(&value, None).unwrap();
        assert_eq!(
            resolved.target,
            ResolvedScriptTarget::Inline("printf 'ok'".to_string())
        );

        let unknown: toml::Value = toml::from_str(
            r#"file = "scripts/run.sh"
args = []
"#,
        )
        .unwrap();
        assert!(super::parse_producer_script_handler(&unknown, None).is_err());
    }

    #[test]
    fn view_query_uses_the_canonical_key() {
        let view: View = toml::from_str(
            r#"
            [engine]
            type = "picker"
            [query]
            type = "string"
            "#,
        )
        .unwrap();
        let serialized = serde_json::to_value(view).unwrap();
        assert!(serialized.get("query").is_some());
        assert!(serialized.get("parameters").is_none());
    }

    #[test]
    fn command_handler_owns_no_operation_type() {
        let wf: Workflow = toml::from_str(
            r#"
            [commands.open]
            type = "call"
            producer = "declared"
            [commands.open.handler]
            target = "other:view"
            "#,
        )
        .unwrap();
        let CommandAction::Call {
            producer, handler, ..
        } = &wf.commands["open"].action
        else {
            panic!("expected call action");
        };
        assert_eq!(*producer, ProducerKind::Declared);
        assert_eq!(handler["target"].as_str(), Some("other:view"));
    }

    #[test]
    fn global_command_bindings_preserve_metadata_and_action_fields() {
        let config: CommandConfig = toml::from_str(
            r#"
            [bindings.help]
            key = "ctrl+h"
            label = "Help"
            visibility = "always"
            type = "return"
            producer = "declared"
            [bindings.help.handler]
            value = "help"
            "#,
        )
        .unwrap();
        let binding = &config.bindings["help"];
        assert_eq!(binding.key.as_deref(), Some("ctrl+h"));
        assert_eq!(binding.label.as_deref(), Some("Help"));
        assert_eq!(binding.visibility, Some(CommandBindingVisibility::Always));
        assert!(matches!(
            binding.action,
            Some(CommandAction::Return {
                producer: ProducerKind::Declared,
                ..
            })
        ));
    }

    #[test]
    fn global_command_bindings_reject_unknown_fields() {
        let unknown_with_type: std::result::Result<CommandConfig, _> = toml::from_str(
            r#"
            [bindings.help]
            key = "ctrl+h"
            label = "Help"
            type = "return"
            producer = "declared"
            extra = true
            [bindings.help.handler]
            value = "help"
            "#,
        );
        assert!(unknown_with_type.is_err());

        let unknown_without_type: std::result::Result<CommandConfig, _> = toml::from_str(
            r#"
            [bindings.help]
            key = "ctrl+h"
            label = "Help"
            extra = true
            "#,
        );
        assert!(unknown_without_type.is_err());
    }

    #[test]
    fn ordinary_global_bindings_without_actions_are_deferred_to_validation() {
        let config: CommandConfig = toml::from_str(
            r#"
            [bindings.help]
            key = "ctrl+h"
            label = "Help"
            "#,
        )
        .unwrap();
        assert!(config.bindings["help"].action.is_none());
    }

    #[test]
    fn built_in_commands_are_an_internal_action() {
        let action = CommandBinding::builtin_commands()
            .command_action("commands")
            .unwrap();
        assert!(matches!(action, CommandAction::OpenCommands));
        assert_eq!(action.operation_type(), "open-commands");
        let _ = ResolvedScriptSource {
            target: ResolvedScriptTarget::Inline(String::new()),
        };
    }
}
