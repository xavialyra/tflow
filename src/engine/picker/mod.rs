pub(crate) mod display;
mod items;
mod keymap;
mod preview;
mod protocol;
mod render;
mod runtime;
mod session;
mod tasks;

#[allow(unused_imports)]
pub(crate) use self::display::{ItemDisplayInput, NormalizedItemDisplay, SlotToken};
use self::items::{FeedDefinition, ItemsRequest, ItemsResult, PickerItemsLoader};
use self::keymap::PickerKeymap;
pub(crate) use self::protocol::{PickerProtocolConfig, create_protocol_view};
pub(crate) use self::render::PickerRenderer;
use self::session::PickerOptions;
pub(crate) use self::session::PickerView;
use self::tasks::PickerItemsScheduler;
use super::{
    EngineValidationContext, InputBindingFactoryContext, RendererFactoryContext, validate_fields,
};
use crate::config::{
    CommandBindingVisibility, CommandScope, Config, Defaults, ENGINE_PICKER, ScriptSourceSpec,
    View, normalize_key, toml_to_json,
};
use crate::expression::{Template, is_dynamic_string};
use crate::input::Key;
use crate::task::{MountTaskLease, MountTaskStarter};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerSelectionCommand {
    pub(crate) id: String,
    pub(crate) key: Key,
    pub(crate) label: String,
    pub(crate) requires_items: bool,
    pub(crate) visibility: CommandBindingVisibility,
}

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
        runtime_snapshot: Value,
    ) -> items::ItemsTaskHandle {
        self.scheduler
            .submit_items(starter, &self.loader, request, runtime_snapshot)
    }
}

#[derive(Clone, Default)]
pub(crate) struct PickerViewServices {
    page_commands: BTreeMap<(String, Option<String>), BTreeMap<String, Value>>,
    page_item_commands: BTreeMap<String, Vec<PickerSelectionCommand>>,
    selection_commands: BTreeMap<String, Vec<PickerSelectionCommand>>,
    non_selection_commands: BTreeSet<(String, String)>,
    feed_owners: BTreeMap<String, Vec<String>>,
    plugin_roots: BTreeMap<String, PathBuf>,
    task_services: Option<Arc<PickerTaskServices>>,
}

impl PickerViewServices {
    fn start_items(
        &self,
        starter: &MountTaskStarter,
        request: ItemsRequest,
        runtime_snapshot: Value,
    ) -> items::ItemsTaskHandle {
        self.task_services
            .as_ref()
            .expect("picker task services are not installed")
            .start_items(starter, request, runtime_snapshot)
    }
}

fn picker_command(
    view_ref: &str,
    command_id: &str,
    command: &crate::config::Command,
) -> Result<Option<PickerSelectionCommand>> {
    let Some(raw_key) = &command.key else {
        return Ok(None);
    };
    let key_name = normalize_key(raw_key)
        .with_context(|| format!("invalid command key for {view_ref}/{command_id}"))?;
    let key = Key::parse_binding(&key_name)
        .with_context(|| format!("invalid command key for {view_ref}/{command_id}"))?;
    Ok(Some(PickerSelectionCommand {
        id: command_id.to_string(),
        key,
        label: command.label.clone(),
        requires_items: command.requires == crate::config::CommandRequirement::Items,
        visibility: CommandBindingVisibility::Always,
    }))
}

fn collect_page_item_commands(
    config: &Config,
    view_ref: &str,
) -> Result<Vec<PickerSelectionCommand>> {
    let Some(view) = config.view(view_ref) else {
        return Ok(Vec::new());
    };
    view.commands
        .iter()
        .filter_map(|(id, command)| picker_command(view_ref, id, command).transpose())
        .collect()
}

impl PickerViewServices {
    pub(crate) fn from_config(config: &Config, root_view_ref: &str) -> Result<Self> {
        let mut services = Self::default();
        let feed_views = config.feed_views(root_view_ref)?;
        let owners = feed_views
            .iter()
            .map(|(owner, _)| owner.clone())
            .collect::<Vec<_>>();
        services
            .feed_owners
            .insert(root_view_ref.to_string(), owners.clone());
        services.page_commands.insert(
            (root_view_ref.to_string(), None),
            crate::command::collect_page_owner_commands(config, root_view_ref, None)?,
        );
        services.page_item_commands.insert(
            root_view_ref.to_string(),
            collect_page_item_commands(config, root_view_ref)?,
        );
        for owner in owners {
            services.page_commands.insert(
                (root_view_ref.to_string(), Some(owner.clone())),
                crate::command::collect_page_owner_commands(config, root_view_ref, Some(&owner))?,
            );
        }

        let view_refs = feed_views
            .into_iter()
            .map(|(view_ref, _)| view_ref)
            .collect::<BTreeSet<_>>();
        for view_ref in view_refs {
            let view = config
                .view(&view_ref)
                .with_context(|| format!("view {:?} is not configured", view_ref))?;
            if let Some(root) = config.plugin_root(&view_ref) {
                let package = view_ref
                    .split_once(':')
                    .map_or(view_ref.as_str(), |(package, _)| package);
                services
                    .plugin_roots
                    .insert(package.to_string(), root.to_path_buf());
            }
            let mut page_item_commands = Vec::new();
            let mut selection_commands = Vec::new();
            for (command_id, command) in &view.commands {
                if let Some(selection_command) = picker_command(&view_ref, command_id, command)? {
                    page_item_commands.push(selection_command.clone());
                    if command.scope == CommandScope::Selection {
                        selection_commands.push(selection_command);
                    }
                }
                if command.scope != CommandScope::Selection {
                    services
                        .non_selection_commands
                        .insert((view_ref.clone(), command_id.clone()));
                }
            }
            services
                .page_item_commands
                .insert(view_ref.clone(), page_item_commands);
            services
                .selection_commands
                .insert(view_ref.clone(), selection_commands);
        }
        Ok(services)
    }

    pub(crate) fn page_commands(
        &self,
        page_view: &str,
        owner_view: Option<&str>,
    ) -> Result<BTreeMap<String, Value>> {
        Ok(self
            .page_commands
            .get(&(page_view.to_string(), owner_view.map(str::to_string)))
            .cloned()
            .unwrap_or_default())
    }

    pub(crate) fn page_item_commands(&self, page_view: &str) -> &[PickerSelectionCommand] {
        self.page_item_commands
            .get(page_view)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn is_non_selection_command(&self, view_ref: &str, command_id: &str) -> bool {
        self.non_selection_commands
            .contains(&(view_ref.to_string(), command_id.to_string()))
    }

    pub(crate) fn command_requires_items(&self, view_ref: &str, command_id: &str) -> bool {
        self.page_item_commands(view_ref)
            .iter()
            .find(|command| command.id == command_id)
            .is_some_and(|command| command.requires_items)
    }

    pub(crate) fn selection_commands(&self, owner: &str) -> &[PickerSelectionCommand] {
        self.selection_commands
            .get(owner)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn feed_owners(&self, page: &str) -> &[String] {
        self.feed_owners.get(page).map(Vec::as_slice).unwrap_or(&[])
    }

    pub(crate) fn plugin_root(&self, view_ref: &str) -> Option<&Path> {
        let package = view_ref
            .split_once(':')
            .map_or(view_ref, |(package, _)| package);
        self.plugin_roots.get(package).map(PathBuf::as_path)
    }
}

struct PickerMountPlan {
    // Definitions are compiled once while preparing the mount. The opaque
    // plan itself still has no scheduler authority.
    definitions: Arc<Vec<Arc<FeedDefinition>>>,
    view_services: PickerViewServices,
}

struct ConfigPickerItemsLoader {
    definitions: Arc<Vec<Arc<FeedDefinition>>>,
}

impl PickerItemsLoader for ConfigPickerItemsLoader {
    fn load(
        &self,
        request: &ItemsRequest,
        runtime: &Value,
        cancellation: &crate::lifecycle::CancellationToken,
    ) -> Result<ItemsResult> {
        items::load_items_for_definitions(
            &self.definitions,
            &request.view,
            &request.identity.page_parameters,
            &request.identity.binding_raw,
            runtime,
            cancellation,
        )
    }
}

pub(super) fn mount_data(
    config: &Config,
    view_ref: &str,
    lease: MountTaskLease,
) -> Result<PickerViewServices> {
    let projection = Arc::new(crate::config::PickerItemsProjection::from_config(
        config, view_ref,
    )?);
    let plan = PickerMountPlan {
        definitions: FeedDefinition::collection(Arc::clone(&projection), view_ref)?,
        view_services: PickerViewServices::from_config(config, view_ref)?,
    };
    let picker = PickerRuntimeServices::from_plan(plan, lease, view_ref);
    Ok(picker.view_services())
}

#[derive(Clone)]
pub(crate) struct PickerRuntimeServices {
    definitions: Arc<Vec<Arc<FeedDefinition>>>,
    scheduler: PickerItemsScheduler,
    view_services: PickerViewServices,
}

impl PickerRuntimeServices {
    #[cfg(test)]
    pub(crate) fn new(config: Arc<Config>, starter: MountTaskStarter, view_ref: &str) -> Self {
        let projection = Arc::new(
            crate::config::PickerItemsProjection::from_config(&config, view_ref)
                .unwrap_or_else(|_| panic!("test config projection must compile")),
        );
        let definitions = FeedDefinition::collection(Arc::clone(&projection), view_ref)
            .unwrap_or_else(|_| panic!("test feed definitions must compile"));
        let plan = PickerMountPlan {
            definitions,
            view_services: PickerViewServices::from_config(&config, view_ref).unwrap_or_default(),
        };
        Self::from_plan(plan, MountTaskLease::new(starter.mount_id()), view_ref)
    }

    fn from_plan(plan: PickerMountPlan, lease: MountTaskLease, view_ref: &str) -> Self {
        let definitions = plan.definitions;
        Self {
            definitions,
            scheduler: PickerItemsScheduler::new(lease, view_ref),
            view_services: plan.view_services,
        }
    }

    pub(crate) fn view_services(&self) -> PickerViewServices {
        let mut services = self.view_services.clone();
        services.task_services = Some(Arc::new(PickerTaskServices {
            loader: Arc::new(ConfigPickerItemsLoader {
                definitions: Arc::clone(&self.definitions),
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
        .with_current_fields(&[
            "item",
            "source",
            "text",
            "value",
            "metadata",
            "selected_index",
        ])
        .with_factory_fields(crate::engine::FactoryFieldPlan {
            runtime: &["layout", "preview", "source_badge"],
            binding: &["layout", "preview", "source_badge"],
            deferred_runtime_errors: &[],
            binding_defaults: Some(&["defaults", "picker", "bindings"]),
        })
        .with_actions([
            crate::engine::ActionSpec::unit("picker.select_next"),
            crate::engine::ActionSpec::unit("picker.select_previous"),
            crate::engine::ActionSpec::unit("picker.accept"),
            crate::engine::ActionSpec::unit("picker.cancel"),
            crate::engine::ActionSpec::unit("picker.retry"),
            crate::engine::ActionSpec::unit("picker.toggle_preview"),
            crate::engine::ActionSpec::unit("picker.back"),
            crate::engine::ActionSpec::unit("picker.exit"),
        ])
}

pub(super) fn validate_config(context: EngineValidationContext<'_>) -> Result<()> {
    let name = context.view_ref;
    let view = context.view;
    validate_fields(name, view, &["layout", "preview", "source_badge"])?;
    validate_picker_bool(view.engine_field("source_badge"), "source_badge")?;
    validate_picker_table(view.engine_field("layout"), "layout")?;
    validate_picker_table(view.engine_field("preview"), "preview")?;
    if let Some(items) = view.selected_items() {
        validate_items_source_config(items, context.script_root)
            .with_context(|| format!("view {:?} has invalid items source configuration", name))?;
    }
    Ok(())
}

pub(super) fn validate_relations(config: &Config) -> Result<()> {
    for (view_ref, view) in config.iter_views() {
        if config.engine(view_ref)? != ENGINE_PICKER {
            continue;
        }
        let feeds = view.selected_feeds();
        if feeds.is_empty() {
            continue;
        }
        if view.selected_items().is_some() {
            bail!("feeds view {:?} cannot define items", view_ref);
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
            let feed_view = config.view(feed_ref).with_context(|| {
                format!("view {:?} references missing feed {:?}", view_ref, feed_ref)
            })?;
            if config.engine(feed_ref)? != ENGINE_PICKER {
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
    }
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

pub(super) fn validate_keymap(name: &str, view: &View) -> Result<()> {
    let keymap = view.keymap.as_ref().map(toml_to_json).transpose()?;
    PickerKeymap::validate_values(None, keymap.as_ref())
        .with_context(|| format!("view {:?} picker keymap", name))
}

pub(super) fn create_renderer(
    _context: RendererFactoryContext,
) -> Result<Box<dyn crate::engine::ViewRenderer>> {
    Ok(Box::new(PickerRenderer::new()))
}

pub(crate) fn create_input_bindings(
    context: InputBindingFactoryContext,
) -> Result<Vec<crate::command::InputActionBinding>> {
    let bindings = context.bindings;
    let has_preview = self::preview::parse(
        bindings.engine_field("layout").cloned(),
        bindings.engine_field("preview").cloned(),
    )?
    .is_some();
    let keymap = PickerKeymap::from_values(bindings.defaults, bindings.view_keymap)?;
    Ok(keymap
        .bindings()
        .map(|(key, action)| {
            let (action, enabled) = match action {
                self::keymap::PickerAction::Exit => (
                    crate::command::ResolvedInputAction::Engine(crate::engine::ActionId::new(
                        "picker.exit",
                    )),
                    true,
                ),
                self::keymap::PickerAction::Back => (
                    crate::command::ResolvedInputAction::Engine(crate::engine::ActionId::new(
                        "picker.back",
                    )),
                    true,
                ),
                self::keymap::PickerAction::ClearInput => (
                    crate::command::ResolvedInputAction::Edit(
                        crate::command::EditorAction::ClearInput,
                    ),
                    true,
                ),
                self::keymap::PickerAction::DeleteBackward => (
                    crate::command::ResolvedInputAction::Edit(
                        crate::command::EditorAction::DeleteBackward,
                    ),
                    true,
                ),
                self::keymap::PickerAction::DeleteWord => (
                    crate::command::ResolvedInputAction::Edit(
                        crate::command::EditorAction::DeleteWord,
                    ),
                    true,
                ),
                self::keymap::PickerAction::SelectPrevious => (
                    crate::command::ResolvedInputAction::Engine(crate::engine::ActionId::new(
                        "picker.select_previous",
                    )),
                    true,
                ),
                self::keymap::PickerAction::SelectNext => (
                    crate::command::ResolvedInputAction::Engine(crate::engine::ActionId::new(
                        "picker.select_next",
                    )),
                    true,
                ),
                self::keymap::PickerAction::Activate => (
                    crate::command::ResolvedInputAction::Engine(crate::engine::ActionId::new(
                        "picker.accept",
                    )),
                    true,
                ),
                self::keymap::PickerAction::TogglePreview => (
                    crate::command::ResolvedInputAction::Engine(crate::engine::ActionId::new(
                        "picker.toggle_preview",
                    )),
                    has_preview,
                ),
            };
            crate::command::InputActionBinding {
                key,
                action,
                label: None,
                enabled,
            }
        })
        .collect())
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
            if spec
                .file_value()
                .is_some_and(|file| !is_dynamic_string(file))
            {
                let root = root.context("script items source has no plugin root")?;
                spec.validate_target(root)?;
            }
            Ok(())
        }
        _ => bail!("items must be an array, complete dynamic path, or script source object"),
    }
}


fn validate_picker_bool(value: Option<&toml::Value>, name: &str) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_bool() || is_complete_dynamic_path(value)? {
        return Ok(());
    }
    bail!(
        "picker field {:?} must be a boolean or complete dynamic path",
        name
    )
}

fn validate_picker_table(value: Option<&toml::Value>, name: &str) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_table() || is_complete_dynamic_path(value)? {
        return Ok(());
    }
    bail!(
        "picker field {:?} must be a table or complete dynamic path",
        name
    )
}

fn is_complete_dynamic_path(value: &toml::Value) -> Result<bool> {
    let Some(source) = value.as_str() else {
        return Ok(false);
    };
    Ok(Template::parse(source)?.is_complete_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_runtime_fields_allow_dynamic_values_for_post_resolution_validation() {
        let view: View = toml::from_str(
            r#"
            [engine]
            type = "picker"
            [engine.config]
            layout = "{{ page.query.layout }}"
            preview = "{{ page.query.preview }}"
            "#,
        )
        .unwrap();
        validate_config(EngineValidationContext {
            view_ref: "core:dynamic",
            view: &view,
            script_root: None,
        })
        .unwrap();
    }
}
