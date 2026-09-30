use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn default_workflow_api() -> u32 {
    1
}

fn default_workflow_entrypoint() -> String {
    "main".to_string()
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
    Auto,
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
    #[serde(default)]
    pub(crate) embedded: EmbeddedDefaults,
    #[serde(default)]
    pub(crate) form: FormDefaults,
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

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EmbeddedDefaults {
    #[serde(default)]
    pub(crate) bindings: Option<toml::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FormDefaults {
    #[serde(default)]
    pub(crate) bindings: Option<toml::Value>,
}

/// How a View's own `[views.<name>.bindings]` table is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingMode {
    /// The table is the whole View layer. The default.
    #[default]
    View,
    /// The table is a base layer; the focused item's `bindings` override it
    /// per physical key.
    ItemMerge,
}

/// The View's own `[views.<name>.bindings]` bindings, keyed by physical key.
pub type ViewBindings = BTreeMap<String, toml::Value>;

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

#[derive(Debug, Clone, Copy, Default, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ViewPresentationMode {
    #[default]
    Inline,
    Popup,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PopupAnchor {
    #[default]
    Center,
    #[serde(alias = "top-center")]
    Top,
    #[serde(alias = "bottom-center")]
    Bottom,
    #[serde(alias = "left-center")]
    Left,
    #[serde(alias = "right-center")]
    Right,
    #[serde(alias = "left-top")]
    TopLeft,
    #[serde(alias = "right-top")]
    TopRight,
    #[serde(alias = "left-bottom")]
    BottomLeft,
    #[serde(alias = "right-bottom")]
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HorizontalAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAlign {
    Top,
    Center,
    Bottom,
}

impl PopupAnchor {
    pub fn alignments(&self) -> (HorizontalAlign, VerticalAlign) {
        match self {
            Self::Center => (HorizontalAlign::Center, VerticalAlign::Center),
            Self::Top => (HorizontalAlign::Center, VerticalAlign::Top),
            Self::Bottom => (HorizontalAlign::Center, VerticalAlign::Bottom),
            Self::Left => (HorizontalAlign::Left, VerticalAlign::Center),
            Self::Right => (HorizontalAlign::Right, VerticalAlign::Center),
            Self::TopLeft => (HorizontalAlign::Left, VerticalAlign::Top),
            Self::TopRight => (HorizontalAlign::Right, VerticalAlign::Top),
            Self::BottomLeft => (HorizontalAlign::Left, VerticalAlign::Bottom),
            Self::BottomRight => (HorizontalAlign::Right, VerticalAlign::Bottom),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DimensionConstraint {
    Cells(u16),
    Percentage(u8),
}

impl DimensionConstraint {
    pub fn resolve(&self, available: u16) -> u16 {
        match self {
            Self::Cells(cells) => *cells,
            Self::Percentage(pct) => {
                let computed = (u32::from(available) * u32::from(*pct)) / 100;
                u16::try_from(computed).unwrap_or(u16::MAX)
            }
        }
    }

    pub fn is_zero(&self) -> bool {
        match self {
            Self::Cells(c) => *c == 0,
            Self::Percentage(p) => *p == 0,
        }
    }
}

impl From<u16> for DimensionConstraint {
    fn from(cells: u16) -> Self {
        Self::Cells(cells)
    }
}

impl<'de> Deserialize<'de> for DimensionConstraint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct DimensionVisitor;

        impl<'de> serde::de::Visitor<'de> for DimensionVisitor {
            type Value = DimensionConstraint;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a positive integer or a percentage string like \"80%\"")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let cells =
                    u16::try_from(value).map_err(|_| E::custom("dimension exceeds u16 limit"))?;
                Ok(DimensionConstraint::Cells(cells))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value < 0 {
                    return Err(E::custom("dimension must be non-negative"));
                }
                self.visit_u64(value as u64)
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let trimmed = value.trim();
                if let Some(pct_str) = trimmed.strip_suffix('%') {
                    let pct: u8 = pct_str
                        .trim()
                        .parse()
                        .map_err(|_| E::custom(format!("invalid percentage: \"{value}\"")))?;
                    if pct > 100 {
                        return Err(E::custom("percentage cannot exceed 100%"));
                    }
                    Ok(DimensionConstraint::Percentage(pct))
                } else {
                    let cells: u16 = trimmed
                        .parse()
                        .map_err(|_| E::custom(format!("invalid dimension string: \"{value}\"")))?;
                    Ok(DimensionConstraint::Cells(cells))
                }
            }
        }

        deserializer.deserialize_any(DimensionVisitor)
    }
}

impl serde::Serialize for DimensionConstraint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Cells(cells) => serializer.serialize_u16(*cells),
            Self::Percentage(pct) => serializer.serialize_str(&format!("{pct}%")),
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ViewPresentation {
    #[serde(default)]
    pub mode: ViewPresentationMode,
    #[serde(default)]
    pub anchor: PopupAnchor,
    #[serde(default)]
    pub offset_x: Option<u16>,
    #[serde(default)]
    pub offset_y: Option<u16>,
    #[serde(default)]
    pub width: Option<DimensionConstraint>,
    #[serde(default)]
    pub height: Option<DimensionConstraint>,
    #[serde(default)]
    pub min_width: Option<u16>,
    #[serde(default)]
    pub max_width: Option<u16>,
    #[serde(default)]
    pub min_height: Option<u16>,
    #[serde(default)]
    pub max_height: Option<u16>,
    #[serde(default = "default_true")]
    pub show_title: bool,
}

impl Default for ViewPresentation {
    fn default() -> Self {
        Self {
            mode: ViewPresentationMode::default(),
            anchor: PopupAnchor::default(),
            offset_x: None,
            offset_y: None,
            width: None,
            height: None,
            min_width: None,
            max_width: None,
            min_height: None,
            max_height: None,
            show_title: true,
        }
    }
}

impl ViewPresentation {
    #[cfg(test)]
    pub fn popup(
        width: impl Into<DimensionConstraint>,
        height: impl Into<DimensionConstraint>,
    ) -> Self {
        Self {
            mode: ViewPresentationMode::Popup,
            width: Some(width.into()),
            height: Some(height.into()),
            ..Default::default()
        }
    }
}

/// Per-View unbinding. Command ownership (the unique command index) and
/// priority (`layer`) are separate axes, so each gets its own field instead of
/// overloading one string list with a sigil:
///
/// - `keys`: physical keys whose bound command loses that key.
/// - `commands`: command addresses (`core.page`, `@engine:picker.clear_input`, or
///   a bare current-workflow name) that lose all of their keys.
/// - `layers`: priority layers (`view` / `engine` / `host`) ignored in this View.
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Unbind {
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub layers: Vec<String>,
}

static EMPTY_TABLE: std::sync::LazyLock<toml::Table> = std::sync::LazyLock::new(toml::Table::new);

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub cancel_exit_code: Option<u8>,
    #[serde(default, rename = "query")]
    pub(crate) query: Option<toml::Table>,
    #[serde(default)]
    pub binding_mode: BindingMode,
    #[serde(default)]
    pub bindings: Option<ViewBindings>,
    #[serde(default)]
    pub unbind: Unbind,
    #[serde(default)]
    pub chrome_commands_show: Option<Vec<String>>,

    #[serde(default)]
    pub picker: Option<toml::Table>,
    #[serde(default)]
    pub capture: Option<toml::Table>,
    #[serde(default)]
    pub form: Option<toml::Table>,
    #[serde(default)]
    pub embedded: Option<toml::Table>,

    #[serde(default)]
    pub engine: Option<toml::Value>,
    #[serde(default)]
    pub preview: Option<toml::Value>,
}

impl View {
    pub fn engine_name(&self) -> Option<&str> {
        self.engine.as_ref().and_then(|v| v.as_str())
    }

    pub fn validate_engine_shape(&self, view_name: &str) -> Result<()> {
        let Some(engine_val) = &self.engine else {
            bail!(
                "view {:?} is missing required field \"engine\" (expected \"picker\", \"capture\", \"form\", or \"embedded\")",
                view_name
            );
        };

        if engine_val.is_table() {
            bail!(
                "view {:?} uses legacy [views.{}.engine]; set `engine = \"...\"` and use named engine table instead (e.g. [views.{}.picker], [views.{}.capture], [views.{}.form], [views.{}.embedded])",
                view_name,
                view_name,
                view_name,
                view_name,
                view_name,
                view_name
            );
        }

        let Some(engine_str) = engine_val.as_str() else {
            bail!(
                "view {:?} field \"engine\" must be a string (expected \"picker\", \"capture\", \"form\", or \"embedded\")",
                view_name
            );
        };

        const VALID_ENGINES: &[&str] = &["picker", "capture", "form", "embedded"];
        if !VALID_ENGINES.contains(&engine_str) {
            bail!(
                "view {:?} has unknown engine {:?} (expected \"picker\", \"capture\", \"form\", or \"embedded\")",
                view_name,
                engine_str
            );
        }

        if self.preview.is_some() {
            bail!(
                "view {:?} defines [views.{}.preview] at view level; preview belongs to the picker engine, configure [views.{}.picker.preview] instead",
                view_name,
                view_name,
                view_name
            );
        }

        let declared = [
            ("picker", self.picker.is_some()),
            ("capture", self.capture.is_some()),
            ("form", self.form.is_some()),
            ("embedded", self.embedded.is_some()),
        ];

        for (table_name, is_present) in declared {
            if is_present && table_name != engine_str {
                bail!(
                    "view {:?} declares engine = {:?}, but configures [views.{}.{}]",
                    view_name,
                    engine_str,
                    view_name,
                    table_name
                );
            }
        }

        Ok(())
    }

    pub(crate) fn selected_engine_type(&self) -> &str {
        self.engine_name().unwrap_or("picker")
    }

    pub(crate) fn selected_engine_config(&self) -> &toml::Table {
        match self.selected_engine_type() {
            "picker" => self.picker.as_ref().unwrap_or(&EMPTY_TABLE),
            "capture" => self.capture.as_ref().unwrap_or(&EMPTY_TABLE),
            "form" => self.form.as_ref().unwrap_or(&EMPTY_TABLE),
            "embedded" => self.embedded.as_ref().unwrap_or(&EMPTY_TABLE),
            _ => &EMPTY_TABLE,
        }
    }

    pub(crate) fn selected_items(&self) -> Option<&toml::Value> {
        self.picker.as_ref().and_then(|t| t.get("items"))
    }

    pub(crate) fn selected_preview(&self) -> Option<&toml::Value> {
        self.picker.as_ref().and_then(|t| t.get("preview"))
    }

    pub(crate) fn engine_field(&self, field: &str) -> Option<&toml::Value> {
        self.selected_engine_config().get(field)
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ProducerKind {
    #[default]
    Declared,
    Script,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReturnProcessor {
    #[serde(rename = "type")]
    #[serde(default)]
    pub(crate) operation: Option<String>,
    #[serde(default)]
    pub(crate) producer: ProducerKind,
    pub(crate) handler: toml::Value,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CommandAction {
    Run {
        #[serde(default)]
        producer: ProducerKind,
        handler: toml::Value,
    },
    Navigate {
        #[serde(default)]
        producer: ProducerKind,
        handler: toml::Value,
    },
    Call {
        #[serde(default)]
        producer: ProducerKind,
        handler: toml::Value,
        #[serde(default)]
        return_processor: Option<ReturnProcessor>,
    },
    Return {
        #[serde(default)]
        producer: ProducerKind,
        handler: toml::Value,
    },
}

impl CommandAction {
    pub(crate) fn producer(&self) -> Option<ProducerKind> {
        match self {
            CommandAction::Run { producer, .. }
            | CommandAction::Navigate { producer, .. }
            | CommandAction::Call { producer, .. }
            | CommandAction::Return { producer, .. } => Some(*producer),
        }
    }

    pub(crate) fn operation_type(&self) -> &'static str {
        match self {
            CommandAction::Run { .. } => "run",
            CommandAction::Navigate { .. } => "navigate",
            CommandAction::Call { .. } => "call",
            CommandAction::Return { .. } => "return",
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct Command {
    #[serde(default)]
    pub label: String,
    #[serde(flatten)]
    pub action: CommandAction,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawConfig {
    #[serde(default)]
    pub(super) chrome_commands_show: Option<Vec<String>>,
    #[serde(default)]
    pub(super) entrypoint: Option<String>,
    #[serde(default)]
    pub(super) image_protocol: ImageProtocol,
    #[serde(default)]
    pub(super) log_file: Option<PathBuf>,
    /// Host-layer bindings as references into `CompiledConfig::all_commands`:
    /// canonical physical key -> command FQID. The loader resolves and
    /// canonicalizes the raw `[host.bindings]` table into this map so the
    /// command definition is stored exactly once.
    #[serde(default)]
    pub(super) host_bindings: BTreeMap<String, String>,
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
pub(crate) struct HostConfig {
    #[serde(default)]
    pub(crate) bindings: Option<BTreeMap<String, toml::Value>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSettings {
    #[serde(default)]
    pub(crate) chrome_commands_show: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) image_protocol: ImageProtocol,
    #[serde(default)]
    pub(crate) theme: Option<String>,
    #[serde(default)]
    pub(crate) log_file: Option<PathBuf>,
    #[serde(default)]
    pub(crate) host: Option<HostConfig>,
    #[serde(default)]
    pub(crate) picker: Option<PickerDefaults>,
    #[serde(default)]
    pub(crate) capture: Option<CaptureDefaults>,
    #[serde(default)]
    pub(crate) embedded: Option<EmbeddedDefaults>,
    #[serde(default)]
    pub(crate) form: Option<FormDefaults>,
    #[serde(default)]
    pub(crate) styles: BTreeMap<String, BTreeMap<String, crate::ui::theme::RawStyleBinding>>,
}

impl RawSettings {
    /// Engine defaults are declared directly at the settings root (no
    /// redundant `defaults.` prefix) to mirror the runtime layer hierarchy.
    pub(crate) fn resolve_defaults(&self) -> Defaults {
        let mut defaults = Defaults::default();
        if let Some(picker) = &self.picker {
            defaults.picker.left_prefix = picker.left_prefix.clone();
            defaults.picker.left_prefix_backspace = picker.left_prefix_backspace;
            defaults.picker.bindings = picker.bindings.clone();
        }
        if let Some(capture) = &self.capture {
            defaults.capture.bindings = capture.bindings.clone();
        }
        if let Some(embedded) = &self.embedded {
            defaults.embedded.bindings = embedded.bindings.clone();
        }
        if let Some(form) = &self.form {
            defaults.form.bindings = form.bindings.clone();
        }
        defaults
    }
}

pub(crate) fn validate_settings_purity(table: &toml::Table, path: &Path) -> Result<()> {
    for forbidden in ["default_view", "workflows", "views", "suite", "workflow"] {
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
    pub(super) host: Option<HostConfig>,
    #[serde(default)]
    pub(super) chrome_commands_show: Option<Vec<String>>,
    #[serde(default)]
    pub(super) styles: BTreeMap<String, BTreeMap<String, crate::ui::theme::RawStyleBinding>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Workflow {
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
pub(crate) struct WorkflowHeader {
    #[serde(default = "default_workflow_api")]
    pub(super) api: u32,
    pub(super) name: String,
    #[serde(default = "default_workflow_entrypoint")]
    pub(super) entrypoint: String,
}

#[cfg(test)]
mod tests {
    use super::{CommandAction, ProducerKind, ResolvedScriptTarget, View, Workflow};

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
    fn raw_host_bindings_map_canonical_keys_to_command_fqids() {
        let raw: super::RawConfig = toml::from_str(
            r#"
            [host_bindings]
            "ctrl+k" = "core.open"
            "ctrl+g" = "__parameters.edit"
            "#,
        )
        .unwrap();
        assert_eq!(raw.host_bindings["ctrl+k"], "core.open");
        assert_eq!(raw.host_bindings["ctrl+g"], "__parameters.edit");
    }

    #[test]
    fn presentation_show_title_defaults_true_and_deserializes() {
        let pres: super::ViewPresentation = toml::from_str(
            r#"
            mode = "popup"
            "#,
        )
        .unwrap();
        assert!(pres.show_title);

        let pres_false: super::ViewPresentation = toml::from_str(
            r#"
            mode = "popup"
            show_title = false
            "#,
        )
        .unwrap();
        assert!(!pres_false.show_title);

        let pres_true: super::ViewPresentation = toml::from_str(
            r#"
            mode = "popup"
            show_title = true
            "#,
        )
        .unwrap();
        assert!(pres_true.show_title);
    }
}
