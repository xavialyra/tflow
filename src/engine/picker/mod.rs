pub(crate) mod display;
mod items;
mod keymap;
mod preview;
mod protocol;
mod render;
mod runtime;
mod session;
mod tasks;

#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use self::display::ItemDisplayInput;
pub(crate) use self::display::SlotToken;
pub(crate) use self::items::run_items_producer_raw;
use self::items::{ItemsRequest, PickerItemsDefinition, PickerItemsLoader};
use self::keymap::PickerKeymap;
pub(crate) use self::protocol::{PickerProtocolConfig, create_protocol_view};
pub(crate) use self::render::PickerRenderer;
use self::session::PickerOptions;
pub(crate) use self::session::PickerView;
use self::tasks::PickerItemsScheduler;
use super::{
    EngineValidationContext, InputBindingFactoryContext, RendererFactoryContext, validate_fields,
};
use crate::input::keymap::KeymapAction;
use crate::task::{MountTaskLease, MountTaskStarter};
use crate::workflow::config::{
    CompiledConfig, Defaults, ProducerKind, View, parse_producer_script_handler,
    toml_to_json,
};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone)]
struct PickerTaskServices {
    loader: Arc<dyn PickerItemsLoader>,
    scheduler: PickerItemsScheduler,
}

impl PickerTaskServices {
    fn start_items(
        &self,
        starter: &MountTaskStarter,
        request: ItemsRequest,
    ) -> items::ItemsTaskHandle {
        self.scheduler.submit_items(starter, &self.loader, request)
    }
}

#[derive(Clone, Default)]
pub(crate) struct PickerViewServices {
    page_commands: BTreeMap<String, BTreeMap<String, Value>>,
    workflow_roots: BTreeMap<String, PathBuf>,
    preview_sources: BTreeMap<String, preview::PreviewSource>,
    launch_input: Value,
    task_services: Option<Arc<PickerTaskServices>>,
}

impl PickerViewServices {
    fn start_items(
        &self,
        starter: &MountTaskStarter,
        request: ItemsRequest,
    ) -> items::ItemsTaskHandle {
        self.task_services
            .as_ref()
            .expect("picker task services are not installed")
            .start_items(starter, request)
    }
}

impl PickerViewServices {
    pub(crate) fn from_config(config: &CompiledConfig, root_view_ref: &str) -> Result<Self> {
        let mut services = Self::default();
        services.page_commands.insert(
            root_view_ref.to_string(),
            crate::workflow::command::collect_available_commands(config, root_view_ref, false)?,
        );

        if let Some(value) = config
            .view(root_view_ref)
            .and_then(|view| view.engine_field("preview"))
        {
            services.preview_sources.insert(
                root_view_ref.to_string(),
                preview::parse_source(toml_to_json(value)?, config.workflow_root(root_view_ref))?,
            );
        }
        if let Some(root) = config.workflow_root(root_view_ref) {
            let package = root_view_ref
                .split_once(':')
                .map_or(root_view_ref, |(package, _)| package);
            services
                .workflow_roots
                .insert(package.to_string(), root.to_path_buf());
        }
        Ok(services)
    }

    pub(crate) fn page_commands(&self, page_view: &str) -> Result<BTreeMap<String, Value>> {
        Ok(self
            .page_commands
            .get(page_view)
            .cloned()
            .unwrap_or_default())
    }

    pub(crate) fn workflow_root(&self, view_ref: &str) -> Option<&Path> {
        let package = view_ref
            .split_once(':')
            .map_or(view_ref, |(package, _)| package);
        self.workflow_roots.get(package).map(PathBuf::as_path)
    }
}

struct PickerMountPlan {
    // Definition is compiled once while preparing the mount. The opaque
    // plan itself still has no scheduler authority.
    definition: Arc<PickerItemsDefinition>,
    view_services: PickerViewServices,
}

struct ConfigPickerItemsLoader {
    definition: Arc<PickerItemsDefinition>,
}

impl PickerItemsLoader for ConfigPickerItemsLoader {
    fn load(
        &self,
        request: &ItemsRequest,
        cancellation: &crate::lifecycle::CancellationToken,
    ) -> items::ItemsLoadOutcome {
        items::load_items_for_definition_with_outcome(
            &self.definition,
            &request.identity.page_parameters,
            &request.identity.binding_raw,
            &request.engine_state,
            cancellation,
        )
    }
}

pub(crate) fn mount_data(
    config: &CompiledConfig,
    input: &Value,
    view_ref: &str,
    lease: MountTaskLease,
) -> Result<PickerViewServices> {
    let projection = Arc::new(crate::workflow::config::PickerItemsProjection::from_config(
        config, input, view_ref,
    )?);
    let mut view_services = PickerViewServices::from_config(config, view_ref)?;
    view_services.launch_input = input.clone();
    let plan = PickerMountPlan {
        definition: PickerItemsDefinition::new(Arc::clone(&projection), view_ref)?,
        view_services,
    };
    let picker = PickerRuntimeServices::from_plan(plan, lease, view_ref);
    Ok(picker.view_services())
}

#[derive(Clone)]
pub(crate) struct PickerRuntimeServices {
    definition: Arc<PickerItemsDefinition>,
    scheduler: PickerItemsScheduler,
    view_services: PickerViewServices,
}

impl PickerRuntimeServices {
    #[cfg(test)]
    pub(crate) fn new(
        config: Arc<CompiledConfig>,
        starter: MountTaskStarter,
        view_ref: &str,
    ) -> Self {
        let projection = Arc::new(
            crate::workflow::config::PickerItemsProjection::from_config(
                &config,
                &Value::Null,
                view_ref,
            )
            .unwrap_or_else(|_| panic!("test config projection must compile")),
        );
        let definition = PickerItemsDefinition::new(Arc::clone(&projection), view_ref)
            .unwrap_or_else(|_| panic!("test presentation definition must compile"));
        let plan = PickerMountPlan {
            definition,
            view_services: PickerViewServices::from_config(&config, view_ref).unwrap_or_default(),
        };
        Self::from_plan(plan, MountTaskLease::new(starter.mount_id()), view_ref)
    }

    fn from_plan(plan: PickerMountPlan, lease: MountTaskLease, view_ref: &str) -> Self {
        let definition = plan.definition;
        Self {
            definition,
            scheduler: PickerItemsScheduler::new(lease, view_ref),
            view_services: plan.view_services,
        }
    }

    pub(crate) fn view_services(&self) -> PickerViewServices {
        let mut services = self.view_services.clone();
        services.task_services = Some(Arc::new(PickerTaskServices {
            loader: Arc::new(ConfigPickerItemsLoader {
                definition: Arc::clone(&self.definition),
            }),
            scheduler: self.scheduler.clone(),
        }));
        services
    }
}

impl From<PickerRuntimeServices> for PickerViewServices {
    fn from(services: PickerRuntimeServices) -> Self {
        services.view_services
    }
}

pub(crate) use items::Item;

pub(super) fn definition() -> crate::engine::EngineDefinition {
    crate::engine::EngineDefinition::new()
        .with_factory_fields(crate::engine::FactoryFieldPlan {
            runtime: &[
                "preview_ratio",
                "preview_min_width",
                "preview_default_open",
                "preview",
                "show_input",
                "show_divider",
            ],
            binding: &["preview_ratio", "preview_min_width", "preview"],
            binding_defaults: Some(&["defaults", "picker", "bindings"]),
        })
        .with_actions([
            crate::engine::ActionSpec::unit("picker.select_next"),
            crate::engine::ActionSpec::unit("picker.select_previous"),
            crate::engine::ActionSpec::unit("picker.cancel"),
            crate::engine::ActionSpec::unit("picker.retry"),
            crate::engine::ActionSpec::unit("picker.toggle_preview"),
            crate::engine::ActionSpec::unit("picker.preview_scroll_up"),
            crate::engine::ActionSpec::unit("picker.preview_scroll_down"),
            crate::engine::ActionSpec::unit("picker.back"),
            crate::engine::ActionSpec::unit("picker.exit"),
        ])
}

pub(super) fn preview_options(
    preview_ratio: Option<&Value>,
    preview_min_width: Option<&Value>,
    preview_default_open: Option<&Value>,
) -> Result<(f64, u16, bool)> {
    let ratio = preview_ratio
        .map(|value| {
            value
                .as_f64()
                .context("picker preview_ratio must be a number")
        })
        .transpose()?
        .unwrap_or(0.35);
    anyhow::ensure!(
        ratio.is_finite() && (0.0..=1.0).contains(&ratio),
        "picker preview_ratio must be between 0 and 1"
    );

    let min_width = preview_min_width
        .map(|value| {
            value
                .as_u64()
                .context("picker preview_min_width must be an unsigned 16-bit integer")
        })
        .transpose()?
        .unwrap_or(24);
    anyhow::ensure!(
        min_width <= u16::MAX as u64,
        "picker preview_min_width must be an unsigned 16-bit integer"
    );

    let default_open = preview_default_open
        .map(|value| {
            value
                .as_bool()
                .context("picker preview_default_open must be a boolean")
        })
        .transpose()?
        .unwrap_or(false);

    Ok((ratio, min_width as u16, default_open))
}

pub(super) fn validate_config(context: EngineValidationContext<'_>) -> Result<()> {
    let name = context.view_ref;
    let view = context.view;
    validate_fields(
        name,
        view,
        &[
            "preview_ratio",
            "preview_min_width",
            "preview_default_open",
            "preview",
            "show_input",
            "show_divider",
        ],
    )?;
    for field in ["show_input", "show_divider"] {
        if let Some(value) = view.engine_field(field)
            && !matches!(value, toml::Value::Boolean(_))
        {
            bail!("view {:?} picker {} must be a boolean", name, field);
        }
    }
    let preview_ratio = view
        .engine_field("preview_ratio")
        .map(toml_to_json)
        .transpose()?;
    let preview_min_width = view
        .engine_field("preview_min_width")
        .map(toml_to_json)
        .transpose()?;
    let preview_default_open = view
        .engine_field("preview_default_open")
        .map(toml_to_json)
        .transpose()?;
    let (preview_ratio, preview_min_width, _) = preview_options(
        preview_ratio.as_ref(),
        preview_min_width.as_ref(),
        preview_default_open.as_ref(),
    )?;
    let preview = view.engine_field("preview").map(toml_to_json).transpose()?;
    let preview = self::preview::parse(preview_ratio, preview_min_width, preview)?;
    if let self::preview::PreviewSource::Script(source) = &preview.source {
        source.validate_target(context.script_root)?;
    }
    if let Some(items) = view.selected_items() {
        validate_items_source_config(items, context.script_root)
            .with_context(|| format!("view {:?} has invalid items source configuration", name))?;
    }
    Ok(())
}

pub(super) fn validate_relations(_config: &CompiledConfig) -> Result<()> {
    Ok(())
}

pub(super) fn validate_defaults(defaults: &Defaults) -> Result<()> {
    let bindings = defaults
        .picker
        .bindings
        .as_ref()
        .map(toml_to_json)
        .transpose()?;
    PickerKeymap::validate_values(bindings.as_ref(), None).context("picker bindings")
}

pub(super) fn validate_keymap(_name: &str, _view: &View) -> Result<()> {
    Ok(())
}

pub(crate) fn is_picker_action(name: &str) -> bool {
    self::keymap::PickerAction::parse(name).is_some()
}

pub(super) fn create_renderer(
    _context: RendererFactoryContext,
) -> Result<Box<dyn crate::engine::ViewRenderer>> {
    Ok(Box::new(PickerRenderer::new()))
}

pub(crate) fn create_input_bindings(
    context: InputBindingFactoryContext,
) -> Result<Vec<crate::workflow::command::InputActionBinding>> {
    let bindings = context.bindings;
    let (preview_ratio, preview_min_width, _) = preview_options(
        bindings.engine_field("preview_ratio"),
        bindings.engine_field("preview_min_width"),
        None,
    )?;
    self::preview::parse(
        preview_ratio,
        preview_min_width,
        bindings.engine_field("preview").cloned(),
    )?;
    let keymap = PickerKeymap::from_values(bindings.defaults, bindings.view_keymap)?;
    Ok(keymap
        .bindings()
        .map(|(key, action)| {
            let (action, enabled) = match action {
                self::keymap::PickerAction::Exit => (
                    crate::workflow::command::ResolvedInputAction::Engine(
                        crate::engine::ActionId::new("picker.exit"),
                    ),
                    true,
                ),
                self::keymap::PickerAction::Back => (
                    crate::workflow::command::ResolvedInputAction::Engine(
                        crate::engine::ActionId::new("picker.back"),
                    ),
                    true,
                ),
                self::keymap::PickerAction::ClearInput => (
                    crate::workflow::command::ResolvedInputAction::Edit(
                        crate::workflow::command::EditorAction::ClearInput,
                    ),
                    true,
                ),
                self::keymap::PickerAction::DeleteBackward => (
                    crate::workflow::command::ResolvedInputAction::Edit(
                        crate::workflow::command::EditorAction::DeleteBackward,
                    ),
                    true,
                ),
                self::keymap::PickerAction::DeleteWord => (
                    crate::workflow::command::ResolvedInputAction::Edit(
                        crate::workflow::command::EditorAction::DeleteWord,
                    ),
                    true,
                ),
                self::keymap::PickerAction::SelectPrevious => (
                    crate::workflow::command::ResolvedInputAction::Engine(
                        crate::engine::ActionId::new("picker.select_previous"),
                    ),
                    true,
                ),
                self::keymap::PickerAction::SelectNext => (
                    crate::workflow::command::ResolvedInputAction::Engine(
                        crate::engine::ActionId::new("picker.select_next"),
                    ),
                    true,
                ),
                self::keymap::PickerAction::PreviewScrollUp => (
                    crate::workflow::command::ResolvedInputAction::Engine(
                        crate::engine::ActionId::new("picker.preview_scroll_up"),
                    ),
                    true,
                ),
                self::keymap::PickerAction::PreviewScrollDown => (
                    crate::workflow::command::ResolvedInputAction::Engine(
                        crate::engine::ActionId::new("picker.preview_scroll_down"),
                    ),
                    true,
                ),
                self::keymap::PickerAction::TogglePreview => (
                    crate::workflow::command::ResolvedInputAction::Engine(
                        crate::engine::ActionId::new("picker.toggle_preview"),
                    ),
                    true,
                ),
            };
            crate::workflow::command::InputActionBinding {
                key,
                action,
                label: None,
                enabled,
            }
        })
        .collect())
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemsProducerConfig {
    producer: ProducerKind,
    handler: toml::Value,
}

fn validate_items_source_config(value: &toml::Value, root: Option<&Path>) -> Result<()> {
    match value {
        toml::Value::Array(_) => {
            let items = toml_to_json(value)?;
            items::validate_item_array(&items)
        }
        toml::Value::Table(fields) if fields.contains_key("producer") => {
            let provider: ItemsProducerConfig = value
                .clone()
                .try_into()
                .context("items producer must define producer and handler")?;
            match provider.producer {
                ProducerKind::Declared => validate_declared_items_handler(&provider.handler),
                ProducerKind::Script => parse_producer_script_handler(&provider.handler, root)
                    .context("items script handler is invalid")
                    .map(|_| ()),
            }
        }
        _ => bail!("items must be an array or a producer object"),
    }
}

fn validate_declared_items_handler(value: &toml::Value) -> Result<()> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Handler {
        items: toml::Value,
    }
    let handler: Handler = value
        .clone()
        .try_into()
        .context("declared items handler must define items")?;
    let items = toml_to_json(&handler.items)?;
    items::validate_item_array(&items)
}

#[cfg(test)]
pub(crate) fn create_preview_test_suite(
    temp: &std::path::Path,
) -> crate::workflow::config::CompiledConfig {
    let wf_dir = temp.join("workflows");
    let browser_dir = wf_dir.join("browser");
    let library_dir = wf_dir.join("library");
    let library_scripts = library_dir.join("scripts");
    let browser_scripts = browser_dir.join("scripts");
    std::fs::create_dir_all(&browser_scripts).unwrap();
    std::fs::create_dir_all(&library_scripts).unwrap();

    std::fs::write(
        temp.join("suite.toml"),
        r#"[suite]
api = 1
name = "Preview Fixtures"
entrypoint = "browser:main"

[workflows]
browser = { dir = "./workflows/browser" }
library = { dir = "./workflows/library" }

[aliases]
preview = "browser:main"
library = "library:main"
"#,
    )
    .unwrap();

    std::fs::write(
        browser_dir.join("workflow.toml"),
        r#"[workflow]
api = 1
name = "Preview browser"
entrypoint = "main"

[views.main]
[views.main.query]
type = "object"
input_order = ["search"]
search = { type = "string", default = "" }
owner = { type = "string", default = "browser" }
[views.main.engine]
type = "picker"
[views.main.engine.config]
items = [
  { display = "Mixed preview", value = "mixed", metadata = { summary = "Rich paragraphs wrap inside a nested layout.", image = "art.png" } },
  { display = "Empty preview", value = "empty", metadata = {} },
]
preview_ratio = 0.35
preview_min_width = 24
[views.main.engine.config.preview]
producer = "script"
[views.main.engine.config.preview.handler]
file = "scripts/preview.py"

[views.override.engine]
type = "picker"
[views.override.engine.config]
items = [
  { display = "Mixed preview", value = "mixed", metadata = { summary = "Rich paragraphs wrap inside a nested layout.", image = "art.png" } },
  { display = "Empty preview", value = "empty", metadata = {} },
]
[views.override.engine.config.preview]
producer = "script"
[views.override.engine.config.preview.handler]
file = "scripts/preview.py"

[views.declared.engine]
type = "picker"
[views.declared.engine.config]
items = [{ display = "Static document" }]
[views.declared.engine.config.preview]
producer = "declared"
document = { type = "paragraph", text = "This document is declared in TOML.", border = true, title = "About" }
"#,
    )
    .unwrap();

    std::fs::write(
        library_dir.join("workflow.toml"),
        r#"[workflow]
api = 1
name = "Preview library"
entrypoint = "main"

[views.main]
[views.main.query]
type = "object"
input_order = ["search"]
search = { type = "string", default = "" }
owner = { type = "string", default = "library" }
[views.main.engine]
type = "picker"
[views.main.engine.config]
items = [
  { display = "Mixed preview", value = "mixed", metadata = { summary = "Rich paragraphs wrap inside a nested layout.", image = "art.png" } },
  { display = "Empty preview", value = "empty", metadata = {} },
]
[views.main.engine.config.preview]
producer = "script"
[views.main.engine.config.preview.handler]
file = "scripts/preview.py"
[views.main.keymap]
"ctrl+p" = "toggle_preview"
"alt+k" = "preview_scroll_up"
"alt+j" = "preview_scroll_down"
"#,
    )
    .unwrap();

    image::DynamicImage::new_rgb8(2, 2)
        .save(library_dir.join("art.png"))
        .unwrap();
    image::DynamicImage::new_rgb8(2, 2)
        .save(browser_dir.join("art.png"))
        .unwrap();

    let preview_script = r#"#!/usr/bin/env python3
import json, sys
request = json.load(sys.stdin)
item = request.get("context", {}).get("engine", {}).get("state", {}).get("item", {})
preview = None
if item.get("value") != "empty":
    preview = {
        "type": "layout",
        "direction": "vertical",
        "constraints": [{"Length": 2}, {"Length": 1}, {"Length": 8}, {"Fill": 1}],
        "children": [
            {"type": "display", "display": {"rows": [
                {"cells": [{"text": item.get("text", "")}]},
            ]}},
            {"type": "separator"},
            {"type": "paragraph", "border": True, "title": "Details", "text": item.get("metadata", {}).get("summary", "Ready")},
            {"type": "paragraph", "text": "Line 1: generated preview content\nLine 2: more"},
        ],
    }
json.dump({"version": 1, "preview": preview}, sys.stdout)
sys.stdout.write("\n")
"#;

    std::fs::write(library_scripts.join("preview.py"), preview_script).unwrap();
    std::fs::write(browser_scripts.join("preview.py"), preview_script).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(library_scripts.join("preview.py"), perms.clone()).unwrap();
        std::fs::set_permissions(browser_scripts.join("preview.py"), perms).unwrap();
    }

    crate::workflow::config::CompiledConfig::load_suite_unvalidated(
        &temp.join("suite.toml"),
        None,
    )
    .unwrap()
    .compile()
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_options_must_be_boolean() {
        for field in ["show_input", "show_divider"] {
            for value in ["true", "false", "\"false\"", "0", "[]", "{}"] {
                let view: View = toml::from_str(&format!(
                    "[engine]\ntype = \"picker\"\n[engine.config]\n{field} = {value}\n"
                ))
                .unwrap();
                let result = validate_config(EngineValidationContext {
                    view_ref: "core:menu",
                    view: &view,
                    script_root: None,
                });
                if matches!(value, "true" | "false") {
                    result.unwrap();
                } else {
                    let error = result.unwrap_err().to_string();
                    assert!(error.contains("core:menu"), "{error}");
                    assert!(
                        error.contains(&format!("{field} must be a boolean")),
                        "{error}"
                    );
                }
            }
        }
    }

    #[test]
    fn static_item_shapes_are_validated_during_engine_validation() {
        let valid: View = toml::from_str(
            r#"
            [engine]
            type = "picker"
            [engine.config]
            items = [{ display = "Example item", value = "example-value" }]
            "#,
        )
        .unwrap();
        validate_config(EngineValidationContext {
            view_ref: "core:static",
            view: &valid,
            script_root: None,
        })
        .unwrap();

        let invalid: View = toml::from_str(
            r#"
            [engine]
            type = "picker"
            [engine.config]
            items = [{ value = "missing-display" }]
            "#,
        )
        .unwrap();
        assert!(
            validate_config(EngineValidationContext {
                view_ref: "core:invalid",
                view: &invalid,
                script_root: None,
            })
            .is_err()
        );
    }
}
