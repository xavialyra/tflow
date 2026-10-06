mod bindings;
pub(crate) mod display;
mod items;
mod protocol;
mod render;
mod runtime;
mod session;
mod tasks;
#[cfg(test)]
mod tests;

use self::bindings::PickerBindings;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use self::display::ItemDisplayInput;
pub(crate) use self::display::SlotToken;
pub(crate) use self::items::run_items_script_raw;
use self::items::{ItemsRequest, PickerItemsDefinition, PickerItemsLoader};
pub(crate) use self::protocol::{PickerProtocolConfig, create_protocol_view};
pub(crate) use self::render::PickerRenderer;
use self::session::PickerOptions;
pub(crate) use self::session::{PickerView, PrefixBackspace};
use self::tasks::PickerItemsScheduler;
use super::{EngineValidationContext, RendererFactoryContext, validate_fields};
use crate::task::{MountTaskLease, MountTaskStarter};
use crate::workflow::config::{CompiledConfig, Defaults, View, parse_script_source, toml_to_json};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::Path;
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
    let view_services = PickerViewServices {
        registry,
        launch_input: input.clone(),
        ..Default::default()
    };
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
            view_services: PickerViewServices::default(),
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

const ALLOWED_PICKER_FIELDS: &[&str] = &[
    "show_input",
    "show_divider",
    "show_left_prefix",
    "input_placeholder",
    "items",
];

/// Subset of [`ALLOWED_PICKER_FIELDS`] that must deserialize as a boolean.
const BOOLEAN_FIELDS: &[&str] = &["show_input", "show_divider", "show_left_prefix"];

/// Subset of [`CONFIG_FIELDS`] that must deserialize as a string.
const STRING_FIELDS: &[&str] = &["input_placeholder"];

pub(super) fn definition() -> crate::engine::EngineDefinition {
    crate::engine::EngineDefinition::new()
        .with_binding_defaults(Some(&["picker", "bindings"]))
        .with_actions([
            crate::engine::ActionSpec::unit("picker.select_next"),
            crate::engine::ActionSpec::unit("picker.select_previous"),
            crate::engine::ActionSpec::unit("picker.cancel"),
            crate::engine::ActionSpec::unit("picker.retry"),
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
            parse_script_source(value, root)
                .context("items script is invalid")
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
