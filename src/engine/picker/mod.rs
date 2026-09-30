mod bindings;
pub(crate) mod display;
mod items;
mod preview;
mod protocol;
mod render;
mod runtime;
mod session;
mod tasks;

use self::bindings::PickerBindings;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use self::display::ItemDisplayInput;
pub(crate) use self::display::SlotToken;
pub(crate) use self::items::run_items_producer_raw;
use self::items::{ItemsRequest, PickerItemsDefinition, PickerItemsLoader};
pub(crate) use self::preview::PreviewDocumentCache;
pub(crate) use self::protocol::{PickerProtocolConfig, create_protocol_view};
pub(crate) use self::render::PickerRenderer;
use self::session::PickerOptions;
pub(crate) use self::session::{PickerView, PrefixBackspace};
use self::tasks::PickerItemsScheduler;
use super::{EngineValidationContext, RendererFactoryContext, validate_fields};
use crate::task::{MountTaskLease, MountTaskStarter};
use crate::workflow::config::{
    CompiledConfig, Defaults, View, parse_producer_script_handler, toml_to_json,
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
    /// Session command registry, when the host exposes one. When present the
    /// Picker publishes the live command envelope; otherwise it publishes an
    /// empty envelope until the first registry update.
    registry: Option<std::sync::Arc<std::sync::RwLock<crate::command::CommandRegistry>>>,
    workflow_roots: BTreeMap<String, PathBuf>,
    /// Session-scoped cache shared by every Picker instance mounted by one
    /// factory, so a remount with the same request identity does not flash.
    preview_cache: PreviewDocumentCache,
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

    pub(crate) fn set_preview_cache(&mut self, cache: PreviewDocumentCache) {
        self.preview_cache = cache;
    }
}

impl PickerViewServices {
    pub(crate) fn from_config(config: &CompiledConfig, root_view_ref: &str) -> Result<Self> {
        let mut services = Self::default();
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
    registry: Option<std::sync::Arc<std::sync::RwLock<crate::command::CommandRegistry>>>,
) -> Result<PickerViewServices> {
    let projection = Arc::new(crate::workflow::config::PickerItemsProjection::from_config(
        config, input, view_ref,
    )?);
    let mut view_services = PickerViewServices::from_config(config, view_ref)?;
    view_services.registry = registry;
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

/// Engine config fields the Picker accepts. Shared by the factory plan and
/// validation so the two cannot drift apart.
const CONFIG_FIELDS: &[&str] = &[
    "show_input",
    "show_divider",
    "show_left_prefix",
    "input_placeholder",
];

const ALLOWED_PICKER_FIELDS: &[&str] = &[
    "show_input",
    "show_divider",
    "show_left_prefix",
    "input_placeholder",
    "items",
    "preview",
];

/// Subset of [`CONFIG_FIELDS`] that must deserialize as a boolean.
const BOOLEAN_FIELDS: &[&str] = &["show_input", "show_divider", "show_left_prefix"];

/// Subset of [`CONFIG_FIELDS`] that must deserialize as a string.
const STRING_FIELDS: &[&str] = &["input_placeholder"];

pub(super) fn definition() -> crate::engine::EngineDefinition {
    crate::engine::EngineDefinition::new()
        .with_factory_fields(crate::engine::FactoryFieldPlan {
            runtime: CONFIG_FIELDS,
            binding_defaults: Some(&["picker", "bindings"]),
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

pub(super) fn validate_config(context: EngineValidationContext<'_>) -> Result<()> {
    let name = context.view_ref;
    let view = context.view;
    validate_fields(name, view, ALLOWED_PICKER_FIELDS)?;
    for field in BOOLEAN_FIELDS {
        if let Some(value) = view.engine_field(field)
            && !matches!(value, toml::Value::Boolean(_))
        {
            bail!("view {:?} picker {} must be a boolean", name, field);
        }
    }
    for field in STRING_FIELDS {
        if let Some(value) = view.engine_field(field)
            && !matches!(value, toml::Value::String(_))
        {
            bail!("view {:?} picker {} must be a string", name, field);
        }
    }
    if let Some(preview_val) = view.selected_preview() {
        let preview_json = toml_to_json(preview_val)?;
        let preview = self::preview::parse(Some(&preview_json))?;
        if let self::preview::PreviewSource::Script(source) = &preview.source {
            source.validate_target(context.script_root)?;
        }
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
    PickerBindings::validate_defaults(bindings.as_ref()).context("picker bindings")
}

pub(super) fn validate_bindings(_name: &str, _view: &View) -> Result<()> {
    Ok(())
}

/// Resolves a bare Picker action name into `("picker.<action>", label)`.
pub(crate) fn engine_action(name: &str) -> Option<(String, &'static str)> {
    crate::input::bindings::binding_action_spec::<self::bindings::PickerAction>(name)
}

pub(super) fn create_renderer(
    _context: RendererFactoryContext,
) -> Result<Box<dyn crate::engine::ViewRenderer>> {
    Ok(Box::new(PickerRenderer::new()))
}

fn validate_items_source_config(value: &toml::Value, root: Option<&Path>) -> Result<()> {
    match value {
        toml::Value::Array(_) => {
            let items = toml_to_json(value)?;
            items::validate_item_array(&items)
        }
        toml::Value::Table(fields)
            if fields.contains_key("file") || fields.contains_key("script") =>
        {
            parse_producer_script_handler(value, root)
                .context("items script handler is invalid")
                .map(|_| ())
        }
        toml::Value::Table(fields) if fields.contains_key("items") => {
            validate_declared_items_handler(value)
        }
        _ => bail!("items must be an array or a table with file/script/items"),
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
engine = "picker"

[views.main.query]
type = "object"
input = "search"
search = { type = "string", default = "" }
owner = { type = "string", default = "browser" }
[views.main.picker]
items = [
  { display = "Mixed preview", value = "mixed", metadata = { summary = "Rich paragraphs wrap inside a nested layout.", image = "art.png" } },
  { display = "Empty preview", value = "empty", metadata = {} },
]
[views.main.picker.preview]
file = "scripts/preview.py"
width = "35%"
min_width = 24

[views.override]
engine = "picker"

[views.override.picker]
items = [
  { display = "Mixed preview", value = "mixed", metadata = { summary = "Rich paragraphs wrap inside a nested layout.", image = "art.png" } },
  { display = "Empty preview", value = "empty", metadata = {} },
]
[views.override.picker.preview]
file = "scripts/preview.py"

[views.declared]
engine = "picker"

[views.declared.picker]
items = [{ display = "Static document" }]
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
engine = "picker"

[views.main.query]
type = "object"
input = "search"
search = { type = "string", default = "" }
owner = { type = "string", default = "library" }
[views.main.picker]
items = [
  { display = "Mixed preview", value = "mixed", metadata = { summary = "Rich paragraphs wrap inside a nested layout.", image = "art.png" } },
  { display = "Empty preview", value = "empty", metadata = {} },
]
[views.main.picker.preview]
file = "scripts/preview.py"
[views.main.bindings]
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

    crate::workflow::config::CompiledConfig::load_suite_unvalidated(&temp.join("suite.toml"), None)
        .unwrap()
        .compile()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_placeholder_must_be_a_string() {
        for field in STRING_FIELDS {
            for value in ["true", "0", "[]", "{}"] {
                let view: View = toml::from_str(&format!(
                    "engine = \"picker\"\n[picker]\n{field} = {value}\n"
                ))
                .unwrap();
                let result = validate_config(EngineValidationContext {
                    view_ref: "core:menu",
                    view: &view,
                    script_root: None,
                });
                let error = result.unwrap_err().to_string();
                assert!(error.contains("core:menu"), "{error}");
                assert!(
                    error.contains(&format!("{field} must be a string")),
                    "{error}"
                );
            }

            let view: View = toml::from_str(&format!(
                "engine = \"picker\"\n[picker]\n{field} = \"Search\"\n"
            ))
            .unwrap();
            validate_config(EngineValidationContext {
                view_ref: "core:menu",
                view: &view,
                script_root: None,
            })
            .unwrap();
        }
    }

    #[test]
    fn display_options_must_be_boolean() {
        for field in BOOLEAN_FIELDS {
            for value in ["true", "false", "\"false\"", "0", "[]", "{}"] {
                let view: View = toml::from_str(&format!(
                    "engine = \"picker\"\n[picker]\n{field} = {value}\n"
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
            engine = "picker"
            [picker]
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
            engine = "picker"
            [picker]
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
