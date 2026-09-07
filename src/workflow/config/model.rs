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

fn default_workflow_api() -> u32 {
    1
}

#[derive(Debug, Clone, Default)]
pub struct WorkflowMetadata {
    pub name: String,
    pub styles: BTreeMap<String, crate::theme::RawStyleBinding>,
}

pub type PluginMetadata = WorkflowMetadata;

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

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptSourceSpec {
    #[serde(default = "default_script_source_val")]
    source: toml::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    script: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    args: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_output_bytes: Option<toml::Value>,
}

fn default_script_source_val() -> toml::Value {
    toml::Value::String("script".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedScriptTarget {
    File(String),
    Inline(String),
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedScriptSource {
    pub(crate) target: ResolvedScriptTarget,
    pub(crate) args: Option<Value>,
    pub(crate) max_output_bytes: Option<usize>,
}

impl ScriptSourceSpec {
    #[cfg(test)]
    pub(crate) fn script_file(file: impl Into<String>) -> Self {
        Self {
            source: toml::Value::String("script".to_string()),
            file: Some(toml::Value::String(file.into())),
            script: None,
            args: None,
            max_output_bytes: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn as_toml_value(&self) -> toml::Value {
        let mut table = toml::map::Map::new();
        table.insert("source".to_string(), self.source.clone());
        if let Some(file) = &self.file {
            table.insert("file".to_string(), file.clone());
        }
        if let Some(script) = &self.script {
            table.insert("script".to_string(), script.clone());
        }
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
        let has_file = self.file.is_some();
        let has_script = self.script.is_some();
        if has_file == has_script {
            bail!("script source must define exactly one of file or script");
        }
        if let Some(source) = self.source.as_str()
            && !is_dynamic_string(source)
        {
            match (source, has_file, has_script) {
                ("inline", false, true) => {}
                ("script", true, false) => {}
                ("inline", true, false) => {
                    bail!("inline script source requires script and forbids file")
                }
                ("script", false, true) => bail!("script source requires file and forbids script"),
                _ => unreachable!("script source name was validated"),
            }
        }
        if let Some(file) = &self.file {
            validate_script_source_file(file)?;
        }
        if let Some(script) = &self.script {
            validate_script_source_body(script)?;
        }
        validate_script_source_args(self.args.as_ref())?;
        validate_script_source_limit(self.max_output_bytes.as_ref())?;
        Ok(())
    }

    pub(crate) fn file_value(&self) -> Option<&str> {
        self.file.as_ref().and_then(toml::Value::as_str)
    }

    pub(crate) fn script_value(&self) -> Option<&str> {
        self.script.as_ref().and_then(toml::Value::as_str)
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

    pub(crate) fn validate_target(&self, root: Option<&Path>) -> Result<()> {
        if self.file.is_none() {
            return Ok(());
        }
        let Some(file_val) = &self.file else {
            return Ok(());
        };
        let Some(file) = file_val.as_str() else {
            return Ok(());
        };
        if is_dynamic_string(file) {
            return Ok(());
        }
        if let Some(root) = root {
            crate::execution::validate_script_target(root, file)
        } else {
            let path = Path::new(file);
            if path.is_relative() {
                bail!(
                    "single-file workflow cannot reference relative script file {:?}; workflows must be self-contained using inline scripts or system binaries",
                    file
                );
            }
            Ok(())
        }
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
    #[serde(default = "default_script_source_str")]
    source: String,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    script: Option<String>,
    #[serde(default)]
    args: Option<Value>,
    #[serde(default)]
    max_output_bytes: Option<usize>,
}

fn default_script_source_str() -> String {
    "script".to_string()
}

impl ResolvedScriptSource {
    pub(crate) fn is_candidate(value: &Value) -> bool {
        value.as_object().is_some_and(|fields| {
            fields.contains_key("source")
                || fields.contains_key("file")
                || fields.contains_key("script")
        })
    }

    pub(crate) fn parse(value: &Value) -> Result<Self> {
        let source: ResolvedScriptSourceConfig = serde_json::from_value(value.clone()).context(
            "script source must resolve to an object with source and file/script fields",
        )?;
        if source.source != "script" && source.source != "inline" {
            bail!("unsupported script source {:?}", source.source);
        }
        let target = match (source.source.as_str(), source.file, source.script) {
            ("inline", None, Some(script)) => {
                if script.is_empty() {
                    bail!("script source body must be non-empty");
                }
                ResolvedScriptTarget::Inline(script)
            }
            ("script", Some(file), None) => {
                if file.is_empty() {
                    bail!("script source file must be non-empty");
                }
                ResolvedScriptTarget::File(file)
            }
            ("inline", Some(_), _) => {
                bail!("inline script source requires script and forbids file")
            }
            ("script", _, Some(_)) => {
                bail!("script source requires file and forbids script")
            }
            (_, Some(_), Some(_)) | (_, None, None) => {
                bail!("script source must define exactly one of file or script")
            }
            (_, Some(file), None) => ResolvedScriptTarget::File(file),
            (_, None, Some(script)) => ResolvedScriptTarget::Inline(script),
        };
        crate::execution::validate_max_output_bytes(source.max_output_bytes)?;
        Ok(Self {
            target,
            args: source.args,
            max_output_bytes: source.max_output_bytes,
        })
    }

    pub(crate) fn script_args(&self, label: &str) -> Result<Vec<String>> {
        crate::execution::resolve_argv(self.args.as_ref(), label)
    }

    pub(crate) fn command_target(&self) -> Result<&ResolvedScriptTarget> {
        anyhow::ensure!(
            self.args.is_none(),
            "run command handler source cannot define args; configure payload args"
        );
        anyhow::ensure!(
            self.max_output_bytes.is_none(),
            "run command handler source cannot define max_output_bytes"
        );
        Ok(&self.target)
    }

    pub(crate) fn file(&self) -> Option<&str> {
        match &self.target {
            ResolvedScriptTarget::File(file) => Some(file.as_str()),
            ResolvedScriptTarget::Inline(_) => None,
        }
    }

    pub(crate) fn target_display(&self) -> &str {
        match &self.target {
            ResolvedScriptTarget::File(file) => file.as_str(),
            ResolvedScriptTarget::Inline(_) => "<inline script>",
        }
    }
}

fn validate_script_source_name(value: &toml::Value) -> Result<()> {
    let source = value
        .as_str()
        .context("script source must be a string or dynamic path")?;
    if is_dynamic_string(source) {
        Template::parse(source)?;
    } else if source != "script" && source != "inline" {
        bail!("unsupported script source {:?}", source);
    }
    Ok(())
}

fn validate_script_source_body(value: &toml::Value) -> Result<()> {
    let script = value
        .as_str()
        .context("script source body must be a string or dynamic path")?;
    if script.is_empty() {
        bail!("script source body must be non-empty");
    }
    if is_dynamic_string(script) {
        Template::parse(script)?;
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
        #[serde(default)]
        payload: Option<RunPayload>,
        #[serde(default)]
        script: Option<String>,
        #[serde(default)]
        handler: Option<toml::Value>,
        #[serde(default)]
        args: Option<toml::Value>,
        #[serde(default)]
        shell: Option<String>,
        #[serde(default)]
        exit: bool,
    },
    Navigate {
        payload: NavigatePayload,
    },
    Call {
        payload: CallPayload,
    },
    Return {
        #[serde(default)]
        payload: Option<ReturnPayload>,
        #[serde(default)]
        value: Option<toml::Value>,
        #[serde(default)]
        handler: Option<toml::Value>,
        #[serde(default)]
        args: Option<toml::Value>,
    },
    EditInput {
        payload: EditInputPayload,
    },
    Invoke {
        payload: InvokePayload,
    },
}

impl CommandAction {
    pub fn run_payload(&self) -> Option<RunPayload> {
        match self {
            CommandAction::Run {
                payload,
                script,
                handler,
                args,
                shell,
                exit,
            } => {
                let mut p = payload.clone().unwrap_or_default();
                if p.script.is_none() {
                    p.script = script.clone();
                }
                if p.handler.is_none() {
                    p.handler = handler.clone();
                }
                if p.args.is_none() {
                    p.args = args.clone();
                }
                if p.shell.is_none() {
                    p.shell = shell.clone();
                }
                if !p.exit {
                    p.exit = *exit;
                }
                Some(p)
            }
            _ => None,
        }
    }

    pub fn return_payload(&self) -> Option<ReturnPayload> {
        match self {
            CommandAction::Return {
                payload,
                value,
                handler,
                args,
            } => {
                let mut p = payload.clone().unwrap_or_default();
                if p.value.is_none() {
                    p.value = value.clone();
                }
                if p.handler.is_none() {
                    p.handler = handler.clone();
                }
                if p.args.is_none() {
                    p.args = args.clone();
                }
                Some(p)
            }
            _ => None,
        }
    }

    pub fn new_run(payload: RunPayload) -> Self {
        CommandAction::Run {
            payload: Some(payload),
            script: None,
            handler: None,
            args: None,
            shell: None,
            exit: false,
        }
    }

    pub fn new_return(payload: ReturnPayload) -> Self {
        CommandAction::Return {
            payload: Some(payload),
            value: None,
            handler: None,
            args: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunPayload {
    #[serde(default)]
    pub handler: Option<toml::Value>,
    #[serde(default)]
    pub script: Option<String>,
    /// Positional arguments passed to the handler after dynamic evaluation.
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
    #[serde(default)]
    pub presentation: ViewPresentation,
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
    pub presentation: ViewPresentation,
    #[serde(default)]
    pub engine: Option<toml::Value>,
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
        let mut engine = toml::map::Map::new();
        engine.insert("show_input".to_string(), toml::Value::Boolean(false));
        engine.insert("show_divider".to_string(), toml::Value::Boolean(false));
        Some(CommandAction::Call {
            payload: CallPayload {
                target: toml::Value::String("selectors:commands".to_string()),
                query: Some(toml::Value::Table(query)),
                presentation: ViewPresentation {
                    mode: ViewPresentationMode::Popup,
                    width: Some(72),
                    height: Some(16),
                },
                engine: Some(toml::Value::Table(engine)),
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
            key: self.key(id).map(str::to_string),
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
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
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

impl Command {
    #[allow(dead_code)]
    pub fn has_key(&self) -> bool {
        self.key.is_some()
    }
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
    #[serde(default, alias = "plugins")]
    pub(super) workflows: BTreeMap<String, Workflow>,
    #[serde(default)]
    pub(super) defaults: Defaults,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Workflow {
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) views: BTreeMap<String, View>,
    #[serde(default)]
    pub(super) styles: BTreeMap<String, crate::theme::RawStyleBinding>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct WorkflowHeader {
    #[serde(default = "default_workflow_api")]
    pub(super) api: u32,
    pub(super) name: String,
}

#[cfg(test)]
mod tests {
    use super::{
        CommandAction, CommandBinding, ResolvedScriptSource, ResolvedScriptTarget,
        ScriptSourceSpec, View, ViewPresentationMode,
    };
    use serde_json::json;

    #[test]
    fn script_source_requires_explicit_matching_target_field() {
        for (source, expected_target) in [
            (
                r#"source = "inline"
script = "foo.sh"
"#,
                ResolvedScriptTarget::Inline("foo.sh".to_string()),
            ),
            (
                r#"source = "script"
file = "scripts/run.sh"
"#,
                ResolvedScriptTarget::File("scripts/run.sh".to_string()),
            ),
        ] {
            let value: toml::Value = toml::from_str(source).unwrap();
            let spec = ScriptSourceSpec::parse(&value).unwrap();
            let resolved = ResolvedScriptSource::parse(&json!({
                "source": value["source"].as_str().unwrap(),
                "script": value.get("script").and_then(toml::Value::as_str),
                "file": value.get("file").and_then(toml::Value::as_str),
            }))
            .unwrap();
            assert_eq!(resolved.target, expected_target);
            assert_eq!(
                spec.script_value().is_some(),
                matches!(expected_target, ResolvedScriptTarget::Inline(_))
            );
        }
    }

    #[test]
    fn script_source_rejects_ambiguous_or_mismatched_fields() {
        for source in [
            r#"source = "inline"
file = "scripts/run.sh"
"#,
            r#"source = "script"
script = "printf hello"
"#,
            r#"source = "inline"
file = "scripts/run.sh"
script = "printf hello"
"#,
            r#"source = "script"
"#,
        ] {
            let value: toml::Value = toml::from_str(source).unwrap();
            assert!(
                ScriptSourceSpec::parse(&value).is_err(),
                "accepted {source}"
            );
        }
    }

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

    #[test]
    fn call_presentation_defaults_to_inline() {
        let view: View = toml::from_str(
            r#"
            [engine]
            type = "picker"
            [commands.open]
            key = "enter"
            label = "Open"
            type = "call"
            [commands.open.payload]
            target = "other:view"
            "#,
        )
        .expect("view must deserialize");

        let CommandAction::Call { payload } = &view.commands["open"].action else {
            panic!("expected call action");
        };
        assert_eq!(payload.presentation.mode, ViewPresentationMode::Inline);
    }

    #[test]
    fn built_in_commands_use_popup_presentation() {
        let binding = CommandBinding::builtin_commands();
        let action = binding
            .command_action("commands")
            .expect("built-in commands must have an action");
        let CommandAction::Call { payload } = action else {
            panic!("built-in commands must call the selector View");
        };

        assert_eq!(payload.presentation.mode, ViewPresentationMode::Popup);
        assert_eq!(payload.presentation.width, Some(72));
        assert_eq!(payload.presentation.height, Some(16));
    }

    #[test]
    fn call_presentation_deserializes_dimensions() {
        let view: View = toml::from_str(
            r#"
            [engine]
            type = "picker"
            [commands.open]
            key = "enter"
            label = "Open"
            type = "call"
            [commands.open.payload]
            target = "other:view"
            [commands.open.payload.presentation]
            mode = "popup"
            width = 72
            height = 16
            "#,
        )
        .expect("popup presentation must deserialize");

        let CommandAction::Call { payload } = &view.commands["open"].action else {
            panic!("expected call action");
        };
        assert_eq!(payload.presentation.mode, ViewPresentationMode::Popup);
        assert_eq!(payload.presentation.width, Some(72));
        assert_eq!(payload.presentation.height, Some(16));
    }
}
