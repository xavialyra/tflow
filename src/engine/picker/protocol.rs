//! Protocol-native adapter for the Picker engine.
//!
//! The adapter owns the interactive editor and route completion. The existing
//! PickerView remains responsible for item loading, selection, and preview

use super::{
    PickerKeymap, PickerOptions, PickerView, PickerViewServices, create_input_bindings,
    create_renderer,
};
use crate::engine::{
    ActionId, BackgroundOutcome, EngineActionInput, EngineDecision, EngineEmission,
    EngineNavigationRequest, EngineRuntime, EngineRuntimeSnapshot, EngineTick,
    InputBindingFactoryContext, ProjectedBindingConfig, ProjectedEngineConfig,
    RendererFactoryContext, ViewContext as EngineContext, ViewIdentity,
};
use crate::input::keymap::KeymapAction;
use crate::input::{EditorBuffer, InputEvent, InputSourceIdentity, Key, ViewMountId};
#[cfg(test)]
use crate::protocol::contracts::{TaskEvent, TaskOutcome};
use crate::protocol::contracts::{TaskId, ViewInstanceId};
use crate::task::{MountTaskLease, MountTaskStarter, TaskRuntime};
use crate::ui::theme::ResolvedTheme;
use crate::view::{
    Binding, BindingSet, EffectRequest, LifecycleEvent, NavigationRequest, ParsedQuery,
    RenderContext, RenderResult, RouteCandidate, RouteCatalog, TransitionRequest, View,
    ViewCommandSnapshot, ViewContext, ViewDecision, ViewEvent, ViewPublication, ViewTaskRegistry,
};
use crate::workflow::parameter::{ParameterBinding, ParameterSnapshot};
use anyhow::{Context, Result, bail};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::Paragraph,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    ops::Range,
};

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone)]
pub(crate) struct PickerProtocolConfig {
    pub(crate) commands: crate::protocol::ViewCommandBindings,
    pub(crate) identity: ViewIdentity,
    pub(crate) engine: ProjectedEngineConfig,
    pub(crate) bindings: ProjectedBindingConfig,
    pub(crate) services: PickerViewServices,
    pub(crate) parameter_binding: ParameterBinding,
    /// Bindings for route targets used by route-entry submission. The map is
    /// optional so existing Picker mounts remain valid when route entry is off.
    pub(crate) parameter_bindings: BTreeMap<String, ParameterBinding>,
    pub(crate) theme: ResolvedTheme,
    pub(crate) route_entry: bool,
    pub(crate) query_prefix: Option<String>,
    pub(crate) runtime_snapshot: Value,
    pub(crate) tasks: TaskRuntime,
}

pub(crate) fn create_protocol_view(
    config: PickerProtocolConfig,
    request: &NavigationRequest,
    instance: ViewInstanceId,
    routes: &dyn RouteCatalog,
) -> Result<Box<dyn View>> {
    anyhow::ensure!(
        request.query.target == config.identity.view_ref,
        "picker request target {:?} does not match configured View {:?}",
        request.query.target,
        config.identity.view_ref
    );
    let mount_id = ViewMountId(instance.0);
    let options = PickerOptions {
        show_input: config
            .engine
            .field("show_input")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        show_divider: config
            .engine
            .field("show_divider")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    };
    let (preview_ratio, preview_min_width, preview_visible) = super::preview_options(
        config.engine.field("preview_ratio"),
        config.engine.field("preview_min_width"),
        config.engine.field("preview_default_open"),
    )?;
    let preview = super::preview::parse(
        preview_ratio,
        preview_min_width,
        config.engine.field("preview").cloned(),
    )?;
    let mut runtime_services = config.services.clone();
    runtime_services.launch_input = config.engine.launch_input.clone();
    let mut runtime =
        PickerView::new_with_preview(&config.identity.view_ref, runtime_services, preview);
    runtime.set_preview_visible(preview_visible);
    let runtime: Box<dyn EngineRuntime> = Box::new(runtime);
    let route_candidates = routes.complete("");
    let mut completion_prefixes = BTreeMap::from([(String::new(), route_candidates.clone())]);
    for candidate in &route_candidates {
        for text in [&candidate.label, &candidate.target.reference] {
            for (index, _) in text.char_indices().skip(1) {
                let prefix = &text[..index];
                completion_prefixes
                    .entry(prefix.to_string())
                    .or_insert_with(|| routes.complete(prefix));
            }
            completion_prefixes
                .entry(text.to_string())
                .or_insert_with(|| routes.complete(text));
        }
    }
    let recognized_route_selectors = route_candidates
        .iter()
        .flat_map(|candidate| {
            [candidate.target.reference.clone(), candidate.label.clone()].into_iter()
        })
        .collect::<HashSet<_>>();
    let current_view = config.identity.view_ref.as_str();
    let route_candidates = route_candidates
        .into_iter()
        .filter(|candidate| candidate.target.reference != current_view)
        .collect::<Vec<_>>();
    let route_resolutions = route_candidates
        .iter()
        .flat_map(|candidate| {
            [candidate.target.reference.clone(), candidate.label.clone()]
                .into_iter()
                .filter_map(|selector| routes.resolve(&selector).map(|target| (selector, target)))
        })
        .collect::<BTreeMap<_, _>>();
    let route_schemas = route_candidates
        .iter()
        .filter_map(|candidate| {
            routes
                .query_schema(&candidate.target.reference)
                .map(|schema| (candidate.target.reference.clone(), schema))
        })
        .collect::<BTreeMap<_, _>>();
    let mut disabled_keys = explicitly_disabled_keys(
        config.bindings.defaults.as_ref(),
        config.bindings.view_keymap.as_ref(),
    );
    let keymap = PickerKeymap::from_values(
        config.bindings.defaults.clone(),
        config.bindings.view_keymap.clone(),
    )?;
    let bindings = create_input_bindings(InputBindingFactoryContext {
        identity: config.identity.clone(),
        bindings: config.bindings,
    })?;
    disabled_keys.extend(
        bindings
            .iter()
            .filter(|binding| !binding.enabled)
            .map(|binding| binding.key.binding_identity()),
    );
    let renderer = create_renderer(RendererFactoryContext)?;
    let snapshot = parameter_snapshot(mount_id, &request.query, request.input.as_ref(), 0);
    let editor = initial_editor(&config.parameter_binding, &snapshot, request.input.as_ref())?;
    let parameters = ParameterSnapshot::from_parts(
        request.query.values.clone(),
        editor.raw.clone(),
        InputSourceIdentity {
            frame: mount_id,
            generation: editor.revision,
        },
        0,
    );
    let engine_context = engine_context(
        instance,
        &config.identity,
        &parameters,
        &Value::Null,
        0,
        None,
        editor.snapshot(),
    );
    let starter = MountTaskStarter::from_lease(&config.tasks, MountTaskLease::new(mount_id));
    Ok(Box::new(PickerProtocolView {
        runtime,
        renderer,
        keymap,
        bindings,
        commands: config.commands,
        route_candidates,
        recognized_route_selectors,
        route_schemas,
        route_resolutions,
        completion_prefixes,
        disabled_keys,
        parameter_bindings: config.parameter_bindings,
        route_entry: config.route_entry,
        query_prefix: config.query_prefix,
        theme: config.theme,
        parameter_binding: config.parameter_binding,
        editor,
        parameters,
        engine_context,
        runtime_snapshot: config.runtime_snapshot,
        publication: None,
        state_revision: 0,
        starter,
        instance,
        task_registry: ViewTaskRegistry::new(instance),
        task_generation: 0,
        active: false,
        activated_once: false,
        closed: false,
        completion: None,
        pending_command: None,
        route_transition_pending: false,
        defer_work_poll: false,
        task_completion_pending: false,
        publication_ready: false,
        diagnostic: None,
        content_size: (1, 1),
        options,
    }))
}

fn parameter_input(query: &ParsedQuery, input: Option<&crate::view::ViewInputSeed>) -> String {
    input
        .map(|seed| seed.text.clone())
        .or_else(|| query.values.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn initial_editor(
    binding: &ParameterBinding,
    snapshot: &ParameterSnapshot,
    input: Option<&crate::view::ViewInputSeed>,
) -> Result<EditorBuffer> {
    if let Some(seed) = input {
        return Ok(EditorBuffer::from_raw(seed.text.clone(), seed.cursor));
    }
    let state = binding.state_from_snapshot(snapshot, false)?;
    let rendered = binding.render_input(&state)?;
    Ok(EditorBuffer::from_raw(rendered.clone(), rendered.len()))
}

fn parsed_route_query(
    binding: &ParameterBinding,
    target: &str,
    schema: &str,
    text: &str,
) -> Result<ParsedQuery> {
    let mut state = binding.instantiate()?;
    binding.parse_input(&mut state, text)?;
    let values = binding.parameter_values(&state)?;
    Ok(ParsedQuery::new(target, schema, values))
}

fn parameter_snapshot(
    mount_id: ViewMountId,
    query: &ParsedQuery,
    input: Option<&crate::view::ViewInputSeed>,
    revision: u64,
) -> ParameterSnapshot {
    ParameterSnapshot::from_parts(
        query.values.clone(),
        parameter_input(query, input),
        InputSourceIdentity {
            frame: mount_id,
            generation: 0,
        },
        revision,
    )
}

fn engine_context(
    instance: ViewInstanceId,
    identity: &ViewIdentity,
    parameters: &ParameterSnapshot,
    runtime: &Value,
    revision: u64,
    current: Option<&Value>,
    editor: crate::input::EditorSnapshot,
) -> EngineContext {
    EngineContext::from_parts(crate::engine::ViewContextParts {
        mount_id: ViewMountId(instance.0),
        identity: identity.clone(),
        input: editor,
        input_generation: 0,
        parameters: parameters.clone(),
        input_rejected: false,
        runtime: EngineRuntimeSnapshot::new(runtime.clone()),
        current: current.cloned().unwrap_or(Value::Null),
        revision,
    })
}

#[derive(Clone)]
struct CompletionState {
    source_instance: ViewInstanceId,
    source_revision: u64,
    candidates: Vec<RouteCandidate>,
    selected: usize,
    range: Range<usize>,
}

struct PendingCommand {
    invocation: crate::workflow::command::CommandInvocation,
    dynamic_owner: Option<String>,
    editor_generation: u64,
    projected_command: Option<(String, String)>,
}

struct PickerProtocolView {
    runtime: Box<dyn EngineRuntime>,
    renderer: Box<dyn crate::engine::ViewRenderer>,
    options: PickerOptions,
    keymap: PickerKeymap,
    bindings: Vec<crate::workflow::command::InputActionBinding>,
    commands: crate::protocol::ViewCommandBindings,
    route_candidates: Vec<RouteCandidate>,
    recognized_route_selectors: HashSet<String>,
    route_schemas: BTreeMap<String, crate::view::QuerySchema>,
    route_resolutions: BTreeMap<String, crate::view::RouteTarget>,
    completion_prefixes: BTreeMap<String, Vec<RouteCandidate>>,
    disabled_keys: HashSet<crate::input::BindingKey>,
    parameter_bindings: BTreeMap<String, ParameterBinding>,
    route_entry: bool,
    query_prefix: Option<String>,
    theme: ResolvedTheme,
    parameter_binding: ParameterBinding,
    editor: EditorBuffer,
    parameters: ParameterSnapshot,
    engine_context: EngineContext,
    runtime_snapshot: Value,
    publication: Option<ViewPublication>,
    state_revision: u64,
    starter: MountTaskStarter,
    instance: ViewInstanceId,
    task_registry: ViewTaskRegistry,
    task_generation: u64,
    active: bool,
    activated_once: bool,
    closed: bool,
    completion: Option<CompletionState>,
    pending_command: Option<PendingCommand>,
    route_transition_pending: bool,
    defer_work_poll: bool,
    task_completion_pending: bool,
    publication_ready: bool,
    diagnostic: Option<String>,
    content_size: (u16, u16),
}

impl PickerProtocolView {
    fn rebuild_context(&mut self, context: &ViewContext) {
        debug_assert_eq!(context.instance, self.instance);
        self.parameters = ParameterSnapshot::from_parts(
            self.parameters.values().clone(),
            self.editor.raw.clone(),
            InputSourceIdentity {
                frame: ViewMountId(self.instance.0),
                generation: self.editor.revision,
            },
            self.parameters.revision(),
        );
        self.engine_context = engine_context(
            self.instance,
            self.engine_context.view_identity(),
            &self.parameters,
            &self.runtime_snapshot,
            self.state_revision,
            self.publication
                .as_ref()
                .map(|publication| &publication.current),
            self.editor.snapshot(),
        );
    }

    fn parse_editor(&mut self, context: &ViewContext) -> Result<bool> {
        let mut state = self
            .parameter_binding
            .state_from_snapshot(&self.parameters, false)?;
        match self
            .parameter_binding
            .parse_input(&mut state, &self.editor.raw)
        {
            Ok(_) => {
                self.diagnostic = None;
                let values = self.parameter_binding.parameter_values(&state)?;
                self.state_revision = self.state_revision.wrapping_add(1);
                self.parameters = ParameterSnapshot::from_parts(
                    values,
                    self.editor.raw.clone(),
                    InputSourceIdentity {
                        frame: ViewMountId(self.instance.0),
                        generation: self.editor.revision,
                    },
                    self.parameters.revision().wrapping_add(1),
                );
                self.rebuild_context(context);
                Ok(true)
            }
            Err(error) => {
                self.diagnostic = Some(error.to_string());
                let rejected = self
                    .runtime
                    .input_rejected(self.engine_context.identity())?;
                self.map_emission(context, rejected)?;
                Ok(false)
            }
        }
    }

    fn edit_changed(&mut self, context: &ViewContext) -> Result<ViewDecision> {
        self.completion = None;
        self.pending_command = None;
        self.task_completion_pending = false;
        if self.parse_editor(context)? {
            let committed = self.runtime.input_committed(self.engine_context.clone())?;
            let committed = self.map_emission(context, committed)?;
            let ready = self.runtime.input_ready(self.engine_context.clone())?;
            let ready = self.map_emission(context, ready)?;
            self.start_prepared_work();
            self.defer_work_poll = true;
            Ok(ViewDecision::Batch(vec![committed, ready]))
        } else {
            Ok(ViewDecision::Invalidate)
        }
    }

    fn activate_ready_work(&mut self, context: &ViewContext) -> Result<ViewDecision> {
        let ready = self.runtime.input_ready(self.engine_context.clone())?;
        let ready = self.map_emission(context, ready)?;
        self.start_prepared_work();
        self.defer_work_poll = true;
        Ok(ready)
    }

    fn body_layout(&self, area: Rect) -> [Rect; 4] {
        let query_height = if self.options.show_input {
            area.height.min(1)
        } else {
            0
        };
        let divider_height = if self.options.show_divider && query_height > 0 {
            area.height.saturating_sub(query_height).min(1)
        } else {
            0
        };
        let completion_available = area
            .height
            .saturating_sub(query_height)
            .saturating_sub(divider_height);
        let completion_height = self
            .completion
            .as_ref()
            .map(|_| completion_available)
            .unwrap_or(0);
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(query_height),
                Constraint::Length(divider_height),
                Constraint::Length(completion_height),
                Constraint::Min(0),
            ])
            .split(area);

        std::array::from_fn(|i| layout[i])
    }

    fn sync_auxiliary_size(&mut self) {
        let body = self.body_layout(Rect::new(0, 0, self.content_size.0, self.content_size.1))[3];
        self.runtime
            .set_auxiliary_content_size((body.width, body.height));
    }

    fn start_prepared_work(&mut self) {
        self.sync_auxiliary_size();
        let task = TaskId(1);
        let generation = self.task_generation.wrapping_add(1).max(1);
        let starter = self.starter.for_task(task, generation);
        if self.runtime.start_prepared_work(&starter) {
            self.task_generation = generation;
            self.task_registry.register(task, generation);
        }
        for (task, generation) in self.runtime.start_prepared_auxiliary_work(&self.starter) {
            self.task_registry.register(task, generation);
        }
    }

    fn route_submission(&mut self) -> Result<Option<ViewDecision>> {
        if !self.route_entry {
            return Ok(None);
        }
        let Some(separator) = self.editor.raw.find(char::is_whitespace) else {
            return Ok(None);
        };
        let selector = &self.editor.raw[..separator];
        if selector.is_empty() {
            return Ok(None);
        }
        let Some(target) = self.route_resolutions.get(selector).cloned() else {
            return Ok(None);
        };
        let target = target.reference;
        let Some(schema) = self.route_schemas.get(&target) else {
            self.diagnostic = Some(format!("route {:?} does not expose a query schema", target));
            return Ok(Some(ViewDecision::Invalidate));
        };
        let Some(binding) = self.parameter_bindings.get(&target).or_else(|| {
            (target == self.engine_context.view_identity().view_ref)
                .then_some(&self.parameter_binding)
        }) else {
            self.diagnostic = Some(format!("route {:?} has no parameter binding", target));
            return Ok(Some(ViewDecision::Invalidate));
        };
        let query_start = self.editor.raw[separator..]
            .char_indices()
            .find_map(|(offset, character)| {
                (!character.is_whitespace()).then_some(separator + offset)
            })
            .unwrap_or(self.editor.raw.len());
        let query_text = &self.editor.raw[query_start..];
        let query_cursor = self
            .editor
            .cursor
            .saturating_sub(query_start)
            .min(query_text.len());
        let query = match parsed_route_query(binding, &target, &schema.id, query_text) {
            Ok(query) => query,
            Err(error) => {
                self.diagnostic = Some(error.to_string());
                return Ok(Some(ViewDecision::Invalidate));
            }
        };
        self.diagnostic = None;
        self.route_transition_pending = true;
        let request = NavigationRequest::new(target, query)
            .with_input(query_text, query_cursor)
            .map_err(crate::view::operation_failure)?;
        Ok(Some(ViewDecision::Transition(TransitionRequest::Push(
            request,
        ))))
    }

    fn recognized_editor_prefix_end(&self) -> Option<usize> {
        if self.query_prefix.is_some() {
            return None;
        }
        let separator = self.editor.raw.find(char::is_whitespace)?;
        let selector = &self.editor.raw[..separator];
        self.recognized_route_selectors
            .contains(selector)
            .then_some(selector.len())
    }

    fn has_command_output(&self, _: &ViewContext) -> bool {
        self.publication_ready
            && self.publication.as_ref().is_some_and(|publication| {
                let current = &publication.current;
                !current.is_null()
                    && current
                        .as_object()
                        .and_then(|values| values.get("item"))
                        .is_some_and(|item| !item.is_null())
            })
    }

    fn can_dispatch_command(
        &self,
        context: &ViewContext,
        _invocation: &crate::workflow::command::CommandInvocation,
    ) -> bool {
        self.publication_ready
            && self.publication.is_some()
            && (self.has_command_output(context) || !self.editor.raw.is_empty())
    }

    fn dispatch_command_key(
        &mut self,
        context: &ViewContext,
        key: crate::input::Key,
        defer_until_ready: bool,
    ) -> Result<Option<ViewDecision>> {
        let projection = self.runtime.command_projection(&self.engine_context)?;
        anyhow::ensure!(
            projection.based_on == self.engine_context.identity(),
            "picker command projection is based on a stale ViewContext"
        );
        let static_binding = self.commands.binding(key).cloned();
        let static_global = static_binding
            .as_ref()
            .is_some_and(|binding| binding.invocation.view_reference().is_none());
        let projected = (!static_global)
            .then(|| {
                projection
                    .bindings
                    .into_iter()
                    .find(|binding| binding.key.binding_identity() == key.binding_identity())
            })
            .flatten();
        if projected.is_none() && static_binding.is_none() {
            return Ok(None);
        }
        let projected_enabled = projected.as_ref().map(|binding| binding.enabled);
        let projected_command = projected
            .as_ref()
            .map(|binding| (binding.command.owner.clone(), binding.command.id.clone()));
        let (invocation, dynamic_owner) = if let Some(binding) = projected {
            if binding.command.owner == context.location.target {
                let binding = static_binding.with_context(|| {
                    format!(
                        "projected page command {}/{} has no configured binding",
                        binding.command.owner, binding.command.id
                    )
                })?;
                (binding.invocation, None)
            } else {
                let invocation = self
                    .commands
                    .view_invocation(&binding.command.owner, &binding.command.id)?;
                (invocation, Some(binding.command.owner))
            }
        } else {
            (
                static_binding
                    .expect("a command binding was checked above")
                    .invocation,
                None,
            )
        };
        let pending = PendingCommand {
            invocation,
            dynamic_owner,
            editor_generation: self.editor.revision,
            projected_command,
        };
        if defer_until_ready && !self.can_dispatch_command(context, &pending.invocation) {
            self.pending_command = Some(pending);
            return Ok(Some(ViewDecision::Stay));
        }
        if projected_enabled == Some(false) {
            return Ok(Some(ViewDecision::Stay));
        }
        Ok(Some(self.command_request(pending)?))
    }

    fn command_request(&self, pending: PendingCommand) -> Result<ViewDecision> {
        let owner = pending
            .dynamic_owner
            .as_deref()
            .map(|owner| {
                self.runtime
                    .command_owner_context(&self.engine_context, owner)?
                    .with_context(|| {
                        format!(
                            "command owner {:?} is unavailable in the current ViewContext",
                            owner
                        )
                    })
            })
            .transpose()?;
        Ok(self
            .commands
            .request_for_invocation(pending.invocation, owner))
    }

    fn dispatch_pending_command(&mut self, context: &ViewContext) -> Result<Option<ViewDecision>> {
        let Some(pending) = self.pending_command.as_ref() else {
            return Ok(None);
        };
        if pending.editor_generation != self.editor.revision {
            self.pending_command = None;
            return Ok(Some(ViewDecision::Stay));
        }
        if !self.can_dispatch_command(context, &pending.invocation) {
            return Ok(None);
        }
        if let Some((owner, id)) = &pending.projected_command {
            let projection = self.runtime.command_projection(&self.engine_context)?;
            anyhow::ensure!(
                projection.based_on == self.engine_context.identity(),
                "picker command projection is based on a stale ViewContext"
            );
            let enabled = projection.bindings.into_iter().find_map(|binding| {
                (binding.command.owner == *owner && binding.command.id == *id)
                    .then_some(binding.enabled)
            });
            if enabled != Some(true) {
                self.pending_command = None;
                return Ok(Some(ViewDecision::Stay));
            }
        }
        self.pending_command
            .take()
            .map(|pending| self.command_request(pending))
            .transpose()
    }

    fn combine_pending_command(
        &mut self,
        context: &ViewContext,
        decision: ViewDecision,
    ) -> Result<ViewDecision> {
        let Some(pending) = self.dispatch_pending_command(context)? else {
            return Ok(decision);
        };
        Ok(match decision {
            ViewDecision::Stay => pending,
            ViewDecision::Invalidate => {
                ViewDecision::Batch(vec![ViewDecision::Invalidate, pending])
            }
            ViewDecision::Batch(mut decisions) if decisions.iter().all(passive_decision) => {
                decisions.push(pending);
                ViewDecision::Batch(decisions)
            }
            _ => bail!("a pending Picker command cannot follow a structural decision"),
        })
    }

    fn map_emission(
        &mut self,
        context: &ViewContext,
        emission: EngineEmission,
    ) -> Result<ViewDecision> {
        if let Some(publication) = emission.publication() {
            self.publication_ready = publication.ready;
            let current = publication.current().clone();
            if self.publication.as_ref().map(|snapshot| &snapshot.current) != Some(&current) {
                self.state_revision = self.state_revision.wrapping_add(1);
            }
            self.publication = Some(ViewPublication::new(current, publication.ready));
            self.rebuild_context(context);
        }
        self.map_decision(context, emission.decision_ref().clone())
    }

    fn map_background(
        &mut self,
        context: &ViewContext,
        outcome: BackgroundOutcome,
    ) -> ViewDecision {
        for notice in outcome.notices {
            if let crate::engine::EngineNotice::Error { message, .. } = notice {
                self.diagnostic = Some(message);
            }
        }
        if let Some(publication) = outcome.publication {
            self.publication_ready = publication.ready;
            self.publication = Some(ViewPublication::new(publication.current, publication.ready));
            self.state_revision = self.state_revision.wrapping_add(1);
            self.rebuild_context(context);
        }
        ViewDecision::Invalidate
    }

    fn map_decision(
        &mut self,
        context: &ViewContext,
        decision: EngineDecision,
    ) -> Result<ViewDecision> {
        match decision {
            EngineDecision::Continue => Ok(ViewDecision::Stay),
            EngineDecision::Invalidate => Ok(ViewDecision::Invalidate),
            EngineDecision::Report(notice) => {
                self.diagnostic = match notice {
                    crate::engine::EngineNotice::Error { message, .. } => Some(message),
                    crate::engine::EngineNotice::ClearError => None,
                    crate::engine::EngineNotice::Info { .. } => None,
                };
                Ok(ViewDecision::Invalidate)
            }
            EngineDecision::RuntimeUpdate(update) => {
                self.runtime_snapshot = crate::workflow::runtime::apply_pointer(
                    &self.runtime_snapshot,
                    &update.path,
                    update.value,
                )?;
                self.state_revision = self.state_revision.wrapping_add(1);
                self.rebuild_context(context);
                Ok(ViewDecision::Invalidate)
            }
            EngineDecision::Close => {
                self.pending_command = None;
                Ok(ViewDecision::Close)
            }
            EngineDecision::Exit => Ok(ViewDecision::Exit),
            EngineDecision::Execute(crate::engine::EffectRequest::CopyToClipboard(value)) => {
                Ok(ViewDecision::Effect(EffectRequest::CopyToClipboard(value)))
            }
            EngineDecision::Navigate(EngineNavigationRequest {
                target,
                parameters,
                replace,
            }) => {
                let schema = self.route_schemas.get(&target).with_context(|| {
                    format!(
                        "Picker navigation target {:?} is not in the route catalog",
                        target
                    )
                })?;
                let query = ParsedQuery::new(
                    target.clone(),
                    schema.id.clone(),
                    parameters.unwrap_or(Value::Null),
                );
                let request = NavigationRequest::new(target, query);
                Ok(ViewDecision::Transition(if replace {
                    TransitionRequest::Replace(request)
                } else {
                    TransitionRequest::Push(request)
                }))
            }
            EngineDecision::Batch(decisions) => {
                let mut mapped = Vec::with_capacity(decisions.len());
                for decision in decisions {
                    mapped.push(self.map_decision(context, decision)?);
                }
                Ok(ViewDecision::Batch(mapped))
            }
        }
    }

    fn action(&mut self, context: &ViewContext, id: &str) -> Result<ViewDecision> {
        let emission = self.runtime.action(EngineActionInput {
            invocation: crate::engine::ActionInvocation::new(ActionId::new(id)),
            context: self.engine_context.clone(),
        })?;
        self.map_emission(context, emission)
    }

    fn completion_range(&self) -> Option<Range<usize>> {
        if !self.route_entry
            || !self.editor.raw[..self.editor.cursor]
                .chars()
                .all(|c| !c.is_whitespace())
        {
            return None;
        }
        Some(0..self.editor.raw[..self.editor.cursor].len())
    }

    fn open_completion(&mut self) {
        let Some(range) = self.completion_range() else {
            self.completion = None;
            return;
        };
        let prefix = &self.editor.raw[range.clone()];
        let folded = prefix.to_lowercase();
        let candidates = self
            .completion_prefixes
            .get(prefix)
            .cloned()
            .unwrap_or_else(|| {
                self.route_candidates
                    .iter()
                    .filter(|candidate| {
                        candidate.label.to_lowercase().starts_with(&folded)
                            || candidate
                                .target
                                .reference
                                .to_lowercase()
                                .starts_with(&folded)
                    })
                    .cloned()
                    .collect()
            })
            .into_iter()
            .filter(|candidate| {
                candidate.target.reference != self.engine_context.view_identity().view_ref
            })
            .collect();
        self.completion = Some(CompletionState {
            source_instance: self.instance,
            source_revision: self.editor.revision,
            candidates,
            selected: 0,
            range,
        });
    }

    fn completion_move(&mut self, direction: isize) -> bool {
        let Some(completion) = &mut self.completion else {
            return false;
        };
        if completion.candidates.is_empty() {
            return true;
        }
        completion.selected =
            cycle_completion_selection(completion.selected, completion.candidates.len(), direction);
        true
    }

    fn completion_accept(&mut self, context: &ViewContext) -> Result<ViewDecision> {
        let Some(completion) = self.completion.take() else {
            return Ok(ViewDecision::Stay);
        };
        if completion.source_instance != self.instance
            || completion.source_revision != self.editor.revision
        {
            return Ok(ViewDecision::Stay);
        }
        let Some(candidate) = completion.candidates.get(completion.selected) else {
            return Ok(ViewDecision::Stay);
        };
        self.editor
            .replace_range(
                completion.source_revision,
                completion.range,
                &format!("{} ", candidate.target.reference),
                candidate.target.reference.len() + 1,
            )
            .map_err(|error| anyhow::anyhow!("completion edit was rejected: {error}"))?;
        let changed = self.edit_changed(context)?;
        if let Some(route) = self.route_submission()? {
            Ok(ViewDecision::Batch(vec![changed, route]))
        } else {
            Ok(changed)
        }
    }

    fn apply_key(&mut self, context: &ViewContext, key: Key) -> Result<ViewDecision> {
        if !self.options.show_input {
            return match key {
                Key::Backspace if context.has_parent => self.action(context, "picker.back"),
                _ => Ok(ViewDecision::Stay),
            };
        }
        match key {
            Key::Char(character) => {
                self.editor.insert(character);
                let changed = self.edit_changed(context)?;
                if character.is_whitespace()
                    && let Some(route) = self.route_submission()?
                {
                    Ok(ViewDecision::Batch(vec![changed, route]))
                } else {
                    Ok(changed)
                }
            }
            Key::Left => {
                self.editor.move_left();
                Ok(ViewDecision::Invalidate)
            }
            Key::Right => {
                self.editor.move_right();
                Ok(ViewDecision::Invalidate)
            }
            Key::Home => {
                self.editor.move_home();
                Ok(ViewDecision::Invalidate)
            }
            Key::End => {
                self.editor.move_end();
                Ok(ViewDecision::Invalidate)
            }
            Key::Backspace => {
                if self.editor.delete_backward() {
                    self.edit_changed(context)
                } else if self.editor.raw.is_empty() && context.has_parent && !self.route_entry {
                    Ok(ViewDecision::CloseToRoot)
                } else {
                    Ok(ViewDecision::Invalidate)
                }
            }
            Key::Delete => {
                if self.editor.delete_forward() {
                    self.edit_changed(context)
                } else {
                    Ok(ViewDecision::Invalidate)
                }
            }
            _ => Ok(ViewDecision::Stay),
        }
    }

    fn map_action(&mut self, context: &ViewContext, key: Key) -> Result<Option<ViewDecision>> {
        let Some(action) = self.keymap.action(key) else {
            return Ok(None);
        };
        let id = match action {
            super::keymap::PickerAction::Exit => "picker.exit",
            super::keymap::PickerAction::Back => "picker.back",
            super::keymap::PickerAction::SelectPrevious => "picker.select_previous",
            super::keymap::PickerAction::SelectNext => "picker.select_next",
            super::keymap::PickerAction::TogglePreview => "picker.toggle_preview",
            super::keymap::PickerAction::PreviewScrollUp => "picker.preview_scroll_up",
            super::keymap::PickerAction::PreviewScrollDown => "picker.preview_scroll_down",
            super::keymap::PickerAction::DeleteBackward => {
                return Ok(Some(self.apply_key(context, Key::Backspace)?));
            }
            super::keymap::PickerAction::ClearInput => {
                self.editor.clear();
                return Ok(Some(self.edit_changed(context)?));
            }
            super::keymap::PickerAction::DeleteWord => {
                self.editor.delete_word();
                return Ok(Some(self.edit_changed(context)?));
            }
        };
        Ok(Some(match id {
            "picker.select_previous" => {
                if self.completion_move(-1) {
                    ViewDecision::Invalidate
                } else {
                    self.action(context, id)?
                }
            }
            "picker.select_next" => {
                if self.completion_move(1) {
                    ViewDecision::Invalidate
                } else {
                    self.action(context, id)?
                }
            }
            _ => self.action(context, id)?,
        }))
    }
}

impl View for PickerProtocolView {
    fn preferred_top_inset(&self) -> u16 {
        1
    }

    fn bindings(&self, _: &ViewContext) -> BindingSet {
        let commands = self.commands.view_bindings();
        let mut entries = commands
            .entries()
            .iter()
            .cloned()
            .chain(
                self.bindings
                    .iter()
                    .filter(|binding| binding.enabled)
                    .map(|binding| Binding {
                        key: binding.key,
                        label: binding.label.clone(),
                    }),
            )
            .collect::<Vec<_>>();
        if let Ok(projection) = self.runtime.command_projection(&self.engine_context) {
            for command in projection.bindings {
                let has_global_binding = self.commands.bindings.iter().any(|binding| {
                    binding.key.binding_identity() == command.key.binding_identity()
                        && binding.invocation.view_reference().is_none()
                });
                if has_global_binding {
                    continue;
                }
                entries.retain(|binding| {
                    binding.key.binding_identity() != command.key.binding_identity()
                });
                if command.enabled {
                    entries.push(Binding {
                        key: command.key,
                        label: command.label,
                    });
                }
            }
        }
        BindingSet::new(entries)
    }

    fn command_bindings(&self) -> Option<&crate::protocol::ViewCommandBindings> {
        Some(&self.commands)
    }

    fn command_owner_context(
        &self,
        _context: &ViewContext,
        owner: &str,
    ) -> Result<Option<crate::workflow::command::CommandOwnerContext>> {
        self.runtime
            .command_owner_context(&self.engine_context, owner)
    }

    fn business_bindings(&self, _context: &ViewContext) -> BindingSet {
        let mut entries = self
            .commands
            .business
            .iter()
            .filter_map(|(key, label)| {
                key.map(|key| Binding {
                    key,
                    label: Some(label.clone()),
                })
            })
            .collect::<Vec<_>>();
        if let Ok(projection) = self.runtime.command_projection(&self.engine_context) {
            for command in projection
                .bindings
                .into_iter()
                .filter(|binding| binding.enabled)
            {
                if let Some(label) = command.label {
                    entries.push(Binding {
                        key: command.key,
                        label: Some(label),
                    });
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        BindingSet::new(
            entries
                .into_iter()
                .filter(|binding| seen.insert(binding.key.binding_identity())),
        )
    }

    fn publication(&self) -> Option<&ViewPublication> {
        self.publication.as_ref()
    }

    fn chrome(&self, context: &ViewContext) -> Result<crate::view::ViewChrome> {
        let model = self.runtime.render_model();
        self.renderer.validate_model(&model)?;
        let mut status = self.renderer.chrome(&model).status;
        if let Some(completion) = &self.completion {
            status = Some(format!(
                "{} / {} views",
                usize::from(!completion.candidates.is_empty()).saturating_add(completion.selected),
                completion.candidates.len()
            ));
        }
        Ok(crate::view::ViewChrome {
            status,
            error: self.diagnostic.clone(),
            bindings: Some(self.bindings(context)),
        })
    }

    fn command_snapshot(&self) -> ViewCommandSnapshot {
        ViewCommandSnapshot {
            engine_type: self.engine_context.view_identity().engine_type.clone(),
            parameters: self.parameters.values().clone(),
            raw_input: self.editor.raw.clone(),
            runtime: self.runtime_snapshot.clone(),
            publication: self.publication.clone(),
            revision: self.state_revision,
        }
    }

    fn event(&mut self, event: ViewEvent, context: &ViewContext) -> Result<ViewDecision> {
        anyhow::ensure!(
            context.instance == self.instance,
            "picker protocol context belongs to a different instance"
        );
        self.rebuild_context(context);
        let result = (|| match event {
            ViewEvent::Lifecycle(LifecycleEvent::Mounted) => Ok(ViewDecision::Stay),
            ViewEvent::Lifecycle(LifecycleEvent::Activated) => {
                self.active = true;
                let restoring = self.activated_once;
                let emission = if restoring {
                    self.runtime.restore_input(self.engine_context.clone())?
                } else {
                    self.activated_once = true;
                    self.runtime.activate(self.engine_context.clone())?
                };
                self.map_emission(context, emission)?;
                self.activate_ready_work(context)?;
                // Lifecycle transitions are transactional in Router and may
                // only return Stay or Invalidate. Runtime publications have
                // already been applied to the mutable ViewContext above.
                Ok(ViewDecision::Invalidate)
            }
            ViewEvent::Lifecycle(LifecycleEvent::Covered) => {
                self.active = false;
                self.runtime.suspend_auxiliary_work();
                self.task_registry.invalidate(TaskId(2));
                self.pending_command = None;
                self.defer_work_poll = false;
                self.task_completion_pending = false;
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Lifecycle(LifecycleEvent::Closing) => {
                self.active = false;
                self.task_registry.invalidate_all();
                self.pending_command = None;
                self.defer_work_poll = false;
                self.task_completion_pending = false;
                self.runtime.deactivate();
                self.starter.cancel_all();
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Lifecycle(LifecycleEvent::Closed) => {
                self.closed = true;
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Lifecycle(LifecycleEvent::TransitionCommitted { .. }) => {
                if self.route_transition_pending {
                    self.route_transition_pending = false;
                    self.editor.clear();
                    self.parse_editor(context)?;
                    Ok(ViewDecision::Invalidate)
                } else {
                    Ok(ViewDecision::Stay)
                }
            }
            ViewEvent::Lifecycle(LifecycleEvent::TransitionRejected { .. }) => {
                self.route_transition_pending = false;
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Command(crate::view::CommandResult::EditInput { value, cursor }) => {
                self.editor = self
                    .editor
                    .replaced_all(value, cursor)
                    .map_err(crate::view::operation_failure)?;
                self.edit_changed(context)
            }
            ViewEvent::Input(InputEvent::Key { key, raw: _ }) => {
                // Completion is a local input mode. Its accept/cancel keys own
                // the event before projected commands while the mode is open.
                if (key == Key::Tab || key == Key::Enter)
                    && self.completion.is_some()
                    && self.keymap.action(key).is_none()
                {
                    return self.completion_accept(context);
                }
                if key == Key::Escape
                    && self.completion.is_some()
                    && !self.disabled_keys.contains(&key.binding_identity())
                {
                    self.completion = None;
                    return Ok(ViewDecision::Invalidate);
                }
                if let Some(decision) = self.dispatch_command_key(context, key, true)? {
                    return Ok(decision);
                }
                if self.disabled_keys.contains(&key.binding_identity()) {
                    return Ok(ViewDecision::Stay);
                }
                if key == Key::Tab && self.keymap.action(key).is_none() {
                    self.open_completion();
                    return Ok(ViewDecision::Invalidate);
                }
                if key == Key::Enter
                    && self.completion.is_none()
                    && self.keymap.action(key).is_none()
                    && let Some(decision) = self.route_submission()?
                {
                    return Ok(decision);
                }
                if let Some(decision) = self.map_action(context, key)? {
                    return Ok(decision);
                }
                self.apply_key(context, key)
            }
            ViewEvent::Input(InputEvent::Paste {
                text: Some(text), ..
            }) => {
                self.editor.insert_text(&text);
                self.edit_changed(context)
            }
            ViewEvent::Input(InputEvent::Paste { text: None, .. })
            | ViewEvent::Input(InputEvent::Bytes(_)) => {
                if self.completion.take().is_some() {
                    Ok(ViewDecision::Invalidate)
                } else {
                    Ok(ViewDecision::Stay)
                }
            }
            ViewEvent::Input(InputEvent::Eof) => Ok(ViewDecision::Exit),
            ViewEvent::Task(task) => {
                if !self.task_registry.accepts(&task) {
                    return Ok(ViewDecision::Stay);
                }
                if task.task == TaskId(2) {
                    self.task_registry.invalidate(task.task);
                    if self.active {
                        self.start_prepared_work();
                    }
                    return Ok(ViewDecision::Invalidate);
                }
                if self.active {
                    if let Some(emission) = self.runtime.poll_work()? {
                        self.task_registry.invalidate(task.task);
                        let decision = self.map_emission(context, emission)?;
                        self.combine_pending_command(context, decision)
                    } else {
                        Ok(ViewDecision::Stay)
                    }
                } else if let Some(outcome) = self.runtime.poll_background_work()? {
                    self.task_registry.invalidate(task.task);
                    Ok(self.map_background(context, outcome))
                } else {
                    Ok(ViewDecision::Stay)
                }
            }
            ViewEvent::Tick if self.active => {
                if self.defer_work_poll {
                    self.defer_work_poll = false;
                    return Ok(ViewDecision::Invalidate);
                }
                if self.task_completion_pending {
                    self.task_completion_pending = false;
                    if let Some(emission) = self.runtime.poll_work()? {
                        let decision = self.map_emission(context, emission)?;
                        return self.combine_pending_command(context, decision);
                    }
                }
                let emission = self.runtime.tick(EngineTick {
                    context: self.engine_context.clone(),
                    content_size: self.content_size,
                })?;
                let decision = self.map_emission(context, emission)?;
                self.start_prepared_work();
                self.combine_pending_command(context, decision)
            }
            ViewEvent::Tick => Ok(ViewDecision::Stay),
            ViewEvent::Resize(size) => {
                self.content_size = (size.width, size.height);
                Ok(ViewDecision::Invalidate)
            }
        })();
        self.sync_auxiliary_size();
        result
    }

    fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        context: &RenderContext,
    ) -> Result<RenderResult> {
        let layout = self.body_layout(area);
        let query_height = layout[0].height;
        let divider_height = layout[1].height;
        let completion_height = layout[2].height;
        let query = visible_editor_query(
            self.query_prefix.as_deref(),
            self.recognized_editor_prefix_end(),
            &self.editor.raw,
            self.editor.cursor,
            layout[0].width as usize,
        );
        if query_height > 0 {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    " ".repeat(layout[0].width as usize),
                    self.theme.picker.text,
                )),
                layout[0],
            );
            let mut spans = Vec::new();
            let mut offset = 0;
            if let Some(highlight) = query.highlight.clone() {
                if highlight.start > 0 {
                    spans.push(Span::styled(
                        query.text[..highlight.start].to_string(),
                        self.theme.picker.text,
                    ));
                }
                spans.push(Span::styled(
                    query.text[highlight.clone()].to_string(),
                    self.theme.picker.input_prefix,
                ));
                offset = highlight.end;
            }
            if offset < query.text.len() {
                spans.push(Span::styled(
                    query.text[offset..].to_string(),
                    self.theme.picker.text,
                ));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), layout[0]);
            render_pseudo_cursor(frame, layout[0], query.cursor, self.theme.picker.cursor);
        }
        if divider_height > 0 {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "─".repeat(layout[1].width as usize),
                    self.theme.chrome.divider,
                )),
                layout[1],
            );
        }
        if let Some(completion) = &self.completion {
            let first = completion
                .selected
                .saturating_add(1)
                .saturating_sub(completion_height as usize);
            let lines = if completion.candidates.is_empty() {
                vec![Line::styled(
                    "(no matching routes)",
                    self.theme.picker.muted,
                )]
            } else {
                completion
                    .candidates
                    .iter()
                    .enumerate()
                    .skip(first)
                    .take(completion_height as usize)
                    .map(|(index, candidate)| {
                        let selected = index == completion.selected;
                        let marker = if selected { "> " } else { "  " };
                        let label = if candidate.label == candidate.target.reference {
                            candidate.label.clone()
                        } else {
                            format!("{} ({})", candidate.label, candidate.target.reference)
                        };
                        let style = if selected {
                            self.theme.picker.selected.add_modifier(Modifier::BOLD)
                        } else {
                            self.theme.picker.text
                        };
                        Line::from(vec![
                            Span::styled(marker, self.theme.picker.marker),
                            Span::styled(label, style),
                        ])
                    })
                    .collect()
            };
            frame.render_widget(Paragraph::new(lines), layout[2]);
        }
        let model = self.runtime.render_model();
        self.renderer.validate_model(&model)?;
        self.renderer.render(
            &model,
            &crate::engine::RenderContext {
                theme: self.theme.clone(),
                image_picker: context.image_picker,
            },
            frame,
            layout[3],
        );
        Ok(RenderResult {
            cursor: None,
            metadata: crate::view::ViewMetadata {
                status: self.renderer.chrome(&model).status,
                error: self.diagnostic.clone(),
                bindings: Some(self.bindings(&ViewContext::new(self.instance, "picker"))),
            },
        })
    }
}

fn passive_decision(decision: &ViewDecision) -> bool {
    match decision {
        ViewDecision::Stay | ViewDecision::Invalidate => true,
        ViewDecision::Batch(decisions) => decisions.iter().all(passive_decision),
        _ => false,
    }
}

fn cycle_completion_selection(selected: usize, length: usize, direction: isize) -> usize {
    if length == 0 {
        return 0;
    }
    if direction.is_negative() {
        if selected == 0 {
            length - 1
        } else {
            selected - 1
        }
    } else {
        (selected + 1) % length
    }
}

fn explicitly_disabled_keys(
    defaults: Option<&Value>,
    patch: Option<&Value>,
) -> HashSet<crate::input::BindingKey> {
    let mut disabled = HashSet::new();
    if let Some(bindings) = defaults.and_then(Value::as_object) {
        for (action, keys) in bindings {
            if !keys.as_array().is_some_and(Vec::is_empty) {
                continue;
            }
            for (key, candidate_action) in
                <super::keymap::PickerAction as KeymapAction>::default_bindings()
            {
                if candidate_action.name() == action {
                    disabled.insert(key.binding_identity());
                }
            }
        }
    }
    if let Some(patch) = patch.and_then(Value::as_object) {
        for (source, value) in patch {
            if value.as_bool() == Some(false)
                && let Ok(key) = Key::parse_binding(source)
            {
                disabled.insert(key.binding_identity());
            }
        }
    }
    disabled
}

const PSEUDO_CURSOR_SYMBOL: &str = "█";

fn render_pseudo_cursor(frame: &mut Frame, area: Rect, column: u16, style: ratatui::style::Style) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let target_x = area
        .x
        .saturating_add(column.min(area.width.saturating_sub(1)));
    let target_y = area.y;
    let buffer = frame.buffer_mut();
    let Some(target) = buffer.cell((target_x, target_y)) else {
        return;
    };
    let wide_cursor_start = (area.x..target_x).rev().find(|&x| {
        buffer.cell((x, target_y)).is_some_and(|cell| {
            let width = UnicodeWidthStr::width(cell.symbol());
            width > 1 && x.saturating_add(width as u16) > target_x
        })
    });
    let cursor_x = wide_cursor_start.unwrap_or_else(|| {
        if target.symbol().is_empty() {
            let mut x = target_x;
            while x > area.x
                && buffer
                    .cell((x, target_y))
                    .is_some_and(|cell| cell.symbol().is_empty())
            {
                x = x.saturating_sub(1);
            }
            x
        } else {
            target_x
        }
    });
    let Some(cell) = buffer.cell((cursor_x, target_y)) else {
        return;
    };
    let symbol = cell.symbol().to_string();
    let symbol_width = UnicodeWidthStr::width(symbol.as_str());
    let blank = symbol.trim().is_empty();

    if let Some(cell) = buffer.cell_mut((cursor_x, target_y)) {
        cell.set_style(style);
        if blank {
            cell.set_symbol(PSEUDO_CURSOR_SYMBOL);
        }
    }
    for offset in 1..symbol_width {
        if let Some(cell) = buffer.cell_mut((cursor_x.saturating_add(offset as u16), target_y)) {
            cell.set_style(style);
        }
    }
}

struct VisibleEditorQuery {
    text: String,
    cursor: u16,
    highlight: Option<Range<usize>>,
}

fn visible_editor_query(
    route_prefix: Option<&str>,
    recognized_input_prefix_end: Option<usize>,
    raw: &str,
    cursor: usize,
    width: usize,
) -> VisibleEditorQuery {
    let cursor = crate::input::previous_char_boundary(raw, cursor);
    let route_prefix = route_prefix.filter(|prefix| !prefix.is_empty());
    let prefix = route_prefix
        .map(|prefix| format!("{prefix} "))
        .unwrap_or_default();
    let prefix_width = UnicodeWidthStr::width(prefix.as_str());
    if width <= prefix_width {
        let text = crate::ui::chrome::clip(&prefix, width);
        let highlight = route_prefix
            .filter(|prefix| prefix.len() <= text.len())
            .map(|prefix| 0..prefix.len());
        return VisibleEditorQuery {
            text,
            cursor: width.saturating_sub(1) as u16,
            highlight,
        };
    }

    let available = width - prefix_width;
    let input_width = UnicodeWidthStr::width(raw);
    let cursor_width = UnicodeWidthStr::width(&raw[..cursor]);
    if input_width <= available {
        let text = format!("{prefix}{raw}");
        return VisibleEditorQuery {
            highlight: editor_prefix_highlight(
                route_prefix,
                recognized_input_prefix_end,
                prefix.len(),
                0,
                raw.len(),
                text.len(),
            ),
            text,
            cursor: (prefix_width + cursor_width) as u16,
        };
    }

    let needs_left_clip = cursor_width > available;
    let marker = if needs_left_clip && available >= 4 {
        "..."
    } else {
        ""
    };
    let marker_width = UnicodeWidthStr::width(marker);
    let budget = available.saturating_sub(marker_width);
    let start_width = if needs_left_clip {
        cursor_width.saturating_sub(budget.saturating_sub(1))
    } else {
        0
    };
    let mut start = 0;
    let mut used = 0;
    for (index, grapheme) in raw[..cursor].grapheme_indices(true) {
        if used >= start_width {
            start = index;
            break;
        }
        used += UnicodeWidthStr::width(grapheme);
        start = index + grapheme.len();
    }
    let mut visible = String::new();
    let mut visible_width = 0;
    for grapheme in raw[start..].graphemes(true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if visible_width + grapheme_width > budget {
            break;
        }
        visible.push_str(grapheme);
        visible_width += grapheme_width;
    }
    let local_cursor = UnicodeWidthStr::width(&raw[start..cursor]);
    let text = format!("{prefix}{marker}{visible}");
    VisibleEditorQuery {
        highlight: editor_prefix_highlight(
            route_prefix,
            recognized_input_prefix_end,
            prefix.len() + marker.len(),
            start,
            start.saturating_add(visible.len()),
            text.len(),
        ),
        text,
        cursor: (prefix_width + marker_width + local_cursor) as u16,
    }
}

fn editor_prefix_highlight(
    route_prefix: Option<&str>,
    recognized_input_prefix_end: Option<usize>,
    output_start: usize,
    input_start: usize,
    input_end: usize,
    output_end: usize,
) -> Option<Range<usize>> {
    if let Some(route_prefix) = route_prefix {
        return (route_prefix.len() <= output_end).then_some(0..route_prefix.len());
    }
    let end = recognized_input_prefix_end?;
    (input_start == 0 && end <= input_end).then_some(output_start..output_start + end)
}

impl Drop for PickerProtocolView {
    fn drop(&mut self) {
        self.task_registry.invalidate_all();
        if !self.closed {
            self.runtime.deactivate();
            self.starter.cancel_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ProjectedBindingConfig;
    use crate::view::{MapRouteCatalog, RouteCatalog, ViewContext};
    use ratatui::style::{Color, Style};
    use ratatui::{Terminal, backend::TestBackend};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    fn request(target: &str) -> NavigationRequest {
        NavigationRequest::new(
            target,
            ParsedQuery::new(target, "query", Value::String(String::new())),
        )
    }

    fn config(services: PickerViewServices) -> PickerProtocolConfig {
        config_with_tasks(services, TaskRuntime::new())
    }

    fn config_with_tasks(services: PickerViewServices, tasks: TaskRuntime) -> PickerProtocolConfig {
        let config = crate::workflow::config::load_test_fixture().unwrap();
        let parameter_binding = config.parameter_binding("core:default").unwrap();
        PickerProtocolConfig {
            commands: crate::protocol::ViewCommandBindings::new(
                &config,
                "core:default",
                crate::lifecycle::CancellationToken::new().observer(),
            )
            .unwrap(),
            identity: ViewIdentity::new("core:default", crate::workflow::config::ENGINE_PICKER),
            engine: ProjectedEngineConfig::default(),
            bindings: ProjectedBindingConfig::default(),
            services,
            parameter_bindings: BTreeMap::new(),
            parameter_binding,
            theme: ResolvedTheme::terminal(),
            route_entry: true,
            query_prefix: None,
            runtime_snapshot: serde_json::json!({"view": {}}),
            tasks,
        }
    }

    #[test]
    fn completion_candidates_are_read_only_and_replace_only_matching_revision() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("sy", "sys:main");
        assert_eq!(routes.complete("sy")[0].target.reference, "sys:main");
    }

    #[test]
    fn navigation_input_seed_is_separate_from_the_structured_query() {
        let query = ParsedQuery::new("picker", "query", Value::String("ok".into()));
        let request = NavigationRequest::new("picker", query.clone())
            .with_input("draft", 3)
            .unwrap();
        assert_eq!(request.query, query);
        assert_eq!(request.input.as_ref().unwrap().cursor, 3);
        let _ = ViewContext::new(ViewInstanceId(1), "picker");
    }

    #[test]
    fn pending_commands_can_follow_nested_passive_batches_only() {
        assert!(passive_decision(&ViewDecision::Batch(vec![
            ViewDecision::Invalidate,
            ViewDecision::Batch(vec![ViewDecision::Stay, ViewDecision::Invalidate]),
        ])));
        assert!(!passive_decision(&ViewDecision::Batch(vec![
            ViewDecision::Invalidate,
            ViewDecision::Exit,
        ])));
    }

    #[test]
    fn completion_selection_cycles_through_picker_matches() {
        assert_eq!(cycle_completion_selection(0, 3, -1), 2);
        assert_eq!(cycle_completion_selection(2, 3, 1), 0);
        assert_eq!(cycle_completion_selection(0, 0, 1), 0);
    }

    #[test]
    fn picker_task_registry_rejects_stale_events_and_consumes_the_current_completion() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("core:default", "core:default");
        let tasks = TaskRuntime::new();
        let fixture = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let services = crate::engine::picker::PickerRuntimeServices::new(
            fixture,
            MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(1))),
            "core:default",
        )
        .view_services();
        let mut view = create_protocol_view(
            config_with_tasks(services, tasks.clone()),
            &request("core:default"),
            ViewInstanceId(1),
            &routes,
        )
        .unwrap();
        let context = ViewContext::new(ViewInstanceId(1), "core:default");
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Mounted), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
            .unwrap();
        let correlation = (TaskId(1), 1);

        assert!(matches!(
            view.event(
                ViewEvent::Task(TaskEvent {
                    instance: ViewInstanceId(1),
                    task: correlation.0,
                    generation: correlation.1 + 1,
                    outcome: TaskOutcome::Failed("stale".to_string()),
                }),
                &context,
            )
            .unwrap(),
            ViewDecision::Stay
        ));
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Covered), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
            .unwrap();
        let event = wait_for_task_event(&tasks);
        assert_eq!((event.task, event.generation), correlation);
        view.event(ViewEvent::Task(event), &context).unwrap();
        assert!(matches!(
            view.event(
                ViewEvent::Task(TaskEvent {
                    instance: ViewInstanceId(1),
                    task: correlation.0,
                    generation: correlation.1,
                    outcome: TaskOutcome::Completed(Value::Null),
                }),
                &context,
            )
            .unwrap(),
            ViewDecision::Stay
        ));
    }

    fn wait_for_task_event(tasks: &TaskRuntime) -> TaskEvent {
        for _ in 0..200 {
            if let Some(event) = tasks.drain_events().into_iter().next() {
                return event;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("timed out waiting for task event");
    }

    struct ExitOnActionRuntime;

    impl EngineRuntime for ExitOnActionRuntime {
        fn action(&mut self, _input: EngineActionInput) -> Result<EngineEmission> {
            Ok(EngineEmission::decision(EngineDecision::Exit))
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("picker", ())
        }
    }

    struct StaleAwareTaskRuntime {
        task: Option<crate::task::TaskHandle<()>>,
        input_rejected: Arc<AtomicBool>,
        polls: Arc<AtomicUsize>,
    }

    impl EngineRuntime for StaleAwareTaskRuntime {
        fn input_rejected(
            &mut self,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<EngineEmission> {
            self.input_rejected.store(true, Ordering::Release);
            Ok(EngineEmission::decision(EngineDecision::Continue))
        }

        fn start_prepared_work(&mut self, starter: &MountTaskStarter) -> bool {
            if self.task.is_some() {
                return false;
            }
            self.task =
                Some(starter.spawn_latest_with_test_snapshot("items", Value::Null, |_| Ok(())));
            true
        }

        fn poll_work(&mut self) -> Result<Option<EngineEmission>> {
            let Some(mut task) = self.task.take() else {
                return Ok(None);
            };
            match task.try_recv() {
                Ok(crate::task::TaskCompletion::Completed(())) => {
                    self.polls.fetch_add(1, Ordering::AcqRel);
                    let emission = EngineEmission::decision(EngineDecision::Continue);
                    if self.input_rejected.load(Ordering::Acquire) {
                        // The Engine identifies this completion as stale and
                        // consumes it without a publication.
                        Ok(Some(emission))
                    } else {
                        Ok(Some(emission.with_publication(
                            crate::engine::ViewContextPublication::new(
                                serde_json::json!({"stale": true}),
                            ),
                        )))
                    }
                }
                Ok(crate::task::TaskCompletion::Failed(error)) => bail!(error),
                Ok(crate::task::TaskCompletion::Cancelled) => {
                    bail!("test task was unexpectedly cancelled")
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.task = Some(task);
                    Ok(None)
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    bail!("test task completion channel disconnected")
                }
            }
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("picker", ())
        }
    }

    fn invalid_integer_binding() -> ParameterBinding {
        let registry = Arc::new(
            crate::workflow::parameter::ParameterRegistry::compile(&serde_json::json!({
                "workflows": {"core": {"views": {"default": {"query": {
                    "type": "object",
                    "count": {"type": "integer", "default": 1}
                }}}}}
            }))
            .unwrap(),
        );
        registry.parameter_binding("core:default").unwrap()
    }

    fn view_with_stale_aware_runtime(
        runtime: Box<dyn EngineRuntime>,
        parameter_binding: ParameterBinding,
        tasks: &TaskRuntime,
    ) -> PickerProtocolView {
        let instance = ViewInstanceId(1);
        let identity = ViewIdentity::new("core:default", crate::workflow::config::ENGINE_PICKER);
        let editor = EditorBuffer::from_raw("1", 1);
        let parameters = ParameterSnapshot::from_parts(
            serde_json::json!({"count": 1}),
            editor.raw.clone(),
            InputSourceIdentity {
                frame: ViewMountId(instance.0),
                generation: editor.revision,
            },
            0,
        );
        let fixture = crate::workflow::config::load_test_fixture().unwrap();
        PickerProtocolView {
            runtime,
            renderer: create_renderer(RendererFactoryContext).unwrap(),
            options: PickerOptions::default(),
            keymap: PickerKeymap::from_values(None, None).unwrap(),
            bindings: Vec::new(),
            commands: crate::protocol::ViewCommandBindings::new(
                &fixture,
                "core:default",
                crate::lifecycle::CancellationToken::new().observer(),
            )
            .unwrap(),
            route_candidates: Vec::new(),
            recognized_route_selectors: HashSet::new(),
            route_schemas: BTreeMap::new(),
            route_resolutions: BTreeMap::new(),
            completion_prefixes: BTreeMap::new(),
            disabled_keys: HashSet::new(),
            parameter_bindings: BTreeMap::new(),
            route_entry: false,
            query_prefix: None,
            theme: ResolvedTheme::terminal(),
            parameter_binding,
            editor: editor.clone(),
            parameters: parameters.clone(),
            engine_context: engine_context(
                instance,
                &identity,
                &parameters,
                &Value::Null,
                0,
                None,
                editor.snapshot(),
            ),
            runtime_snapshot: Value::Null,
            publication: None,
            state_revision: 0,
            starter: MountTaskStarter::from_lease(tasks, MountTaskLease::new(ViewMountId(1))),
            instance,
            task_registry: ViewTaskRegistry::new(instance),
            task_generation: 0,
            active: false,
            activated_once: false,
            closed: false,
            completion: None,
            pending_command: None,
            route_transition_pending: false,
            defer_work_poll: false,
            task_completion_pending: false,
            publication_ready: false,
            diagnostic: None,
            content_size: (1, 1),
        }
    }

    #[test]
    fn preview_body_size_tracks_resize_completion_and_committed_starts() {
        struct SizedRuntime {
            size: (u16, u16),
            seen: Arc<std::sync::Mutex<Vec<(&'static str, (u16, u16))>>>,
        }
        impl EngineRuntime for SizedRuntime {
            fn set_auxiliary_content_size(&mut self, size: (u16, u16)) {
                self.size = size;
                self.seen.lock().unwrap().push(("size", size));
            }
            fn start_prepared_work(&mut self, _: &MountTaskStarter) -> bool {
                self.seen.lock().unwrap().push(("main", self.size));
                false
            }
            fn start_prepared_auxiliary_work(
                &mut self,
                _: &MountTaskStarter,
            ) -> Vec<(TaskId, u64)> {
                self.seen.lock().unwrap().push(("auxiliary", self.size));
                Vec::new()
            }
            fn action(&mut self, _: EngineActionInput) -> Result<EngineEmission> {
                self.seen.lock().unwrap().push(("action", self.size));
                Ok(EngineEmission::decision(EngineDecision::Continue))
            }
            fn render_model(&self) -> crate::engine::RenderModel {
                crate::engine::RenderModel::new("picker", ())
            }
        }
        let tasks = TaskRuntime::new();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let runtime = SizedRuntime {
            size: (99, 99),
            seen: seen.clone(),
        };
        let mut view =
            view_with_stale_aware_runtime(Box::new(runtime), invalid_integer_binding(), &tasks);
        view.options.show_input = true;
        view.options.show_divider = true;
        view.route_entry = true;
        let context = ViewContext::new(ViewInstanceId(1), "core:default");
        let resize = |height| ViewEvent::Resize(crate::view::TerminalSize { width: 40, height });
        view.event(resize(8), &context).unwrap();
        assert_eq!(seen.lock().unwrap().last(), Some(&("size", (40, 6))));
        view.start_prepared_work();
        assert_eq!(seen.lock().unwrap().last(), Some(&("auxiliary", (40, 6))));
        view.event(
            ViewEvent::Input(InputEvent::Key {
                key: Key::Tab,
                raw: Vec::new(),
            }),
            &context,
        )
        .unwrap();
        assert!(view.completion.is_some());
        assert_eq!(seen.lock().unwrap().last(), Some(&("size", (40, 0))));
        view.start_prepared_work();
        assert_eq!(seen.lock().unwrap().last(), Some(&("auxiliary", (40, 0))));
        view.event(resize(12), &context).unwrap();
        assert_eq!(seen.lock().unwrap().last(), Some(&("size", (40, 0))));
        view.event(
            ViewEvent::Input(InputEvent::Key {
                key: Key::Escape,
                raw: Vec::new(),
            }),
            &context,
        )
        .unwrap();
        assert_eq!(seen.lock().unwrap().last(), Some(&("size", (40, 10))));
        view.action(&context, "picker.toggle_preview").unwrap();
        assert_eq!(seen.lock().unwrap().last(), Some(&("action", (40, 10))));
        view.event(resize(1), &context).unwrap();
        view.start_prepared_work();
        assert_eq!(seen.lock().unwrap().last(), Some(&("auxiliary", (40, 0))));
        for height in 0..12 {
            let body = view.body_layout(Rect::new(0, 0, 40, height))[3];
            assert_eq!(body.height, height.saturating_sub(2));
        }
    }

    #[test]
    fn explicit_enter_action_precedes_route_submission() {
        let tasks = TaskRuntime::new();
        let binding = invalid_integer_binding();
        let mut view =
            view_with_stale_aware_runtime(Box::new(ExitOnActionRuntime), binding.clone(), &tasks);
        view.route_entry = true;
        view.editor = EditorBuffer::from_raw("other 1", 7);
        view.route_resolutions.insert(
            "other".to_string(),
            crate::view::RouteTarget {
                reference: "other".to_string(),
                label: None,
            },
        );
        view.route_schemas.insert(
            "other".to_string(),
            crate::view::QuerySchema {
                id: "query".to_string(),
            },
        );
        view.parameter_bindings.insert("other".to_string(), binding);
        view.keymap = PickerKeymap::from_values(
            Some(serde_json::json!({
                "exit": ["enter"]
            })),
            None,
        )
        .unwrap();

        let context = ViewContext::new(ViewInstanceId(1), "core:default");
        let decision = view
            .event(
                ViewEvent::Input(InputEvent::Key {
                    key: Key::Enter,
                    raw: b"\r".to_vec(),
                }),
                &context,
            )
            .unwrap();
        assert!(matches!(decision, ViewDecision::Exit));
        tasks.shutdown_and_wait();
    }

    #[test]
    fn invalid_input_keeps_the_active_task_registered_until_its_stale_completion_is_consumed() {
        let tasks = TaskRuntime::new();
        let input_rejected = Arc::new(AtomicBool::new(false));
        let polls = Arc::new(AtomicUsize::new(0));
        let runtime = StaleAwareTaskRuntime {
            task: None,
            input_rejected: Arc::clone(&input_rejected),
            polls: Arc::clone(&polls),
        };
        let mut view =
            view_with_stale_aware_runtime(Box::new(runtime), invalid_integer_binding(), &tasks);
        let context = ViewContext::new(ViewInstanceId(1), "core:default");

        view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
            .unwrap();
        view.event(
            ViewEvent::Input(InputEvent::Key {
                key: Key::Char('x'),
                raw: Vec::new(),
            }),
            &context,
        )
        .unwrap();
        assert!(input_rejected.load(Ordering::Acquire));

        let event = wait_for_task_event(&tasks);
        assert_eq!((event.task, event.generation), (TaskId(1), 1));
        view.event(ViewEvent::Task(event.clone()), &context)
            .unwrap();
        assert_eq!(polls.load(Ordering::Acquire), 1);
        assert!(view.publication().is_none());

        // Consumption invalidates the registry, so a duplicate completion
        // cannot be polled or published.
        view.event(ViewEvent::Task(event), &context).unwrap();
        assert_eq!(polls.load(Ordering::Acquire), 1);

        drop(view);
        tasks.shutdown_and_wait();
    }

    #[test]
    fn static_display_options_reach_picker_rendering_and_input() {
        let root = std::env::temp_dir().join(format!(
            "tlaunch-picker-display-options-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("workflows")).unwrap();
        std::fs::write(
            root.join("config.toml"),
            "default_view = \"core:default\"\n",
        )
        .unwrap();
        for (fields, show_input, show_divider) in [
            ("", true, true),
            ("show_input = true\nshow_divider = true", true, true),
            ("show_input = true\nshow_divider = false", true, false),
            ("show_input = false\nshow_divider = true", false, true),
            ("show_input = false\nshow_divider = false", false, false),
        ] {
            std::fs::write(
                root.join("workflows/core.toml"),
                format!(
                    "[workflow]\napi = 1\nname = \"Display options test\"\n[views.default.engine]\ntype = \"picker\"\n[views.default.engine.config]\nitems = []\n{fields}\n"
                ),
            )
            .unwrap();
            let fixture = Arc::new(
                crate::workflow::config::CompiledConfig::load_unvalidated(
                    &root.join("config.toml"),
                )
                .unwrap()
                .compile()
                .unwrap(),
            );
            let engines = crate::engine::EngineRegistry::new();
            fixture.validate_with_engines(&engines).unwrap();
            let tasks = TaskRuntime::new();
            let services = crate::engine::picker::PickerRuntimeServices::new(
                Arc::clone(&fixture),
                MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(1))),
                "core:default",
            )
            .view_services();
            let mut picker_config = config_with_tasks(services, tasks.clone());
            picker_config.parameter_binding = fixture.parameter_binding("core:default").unwrap();
            picker_config.engine = crate::engine::project_engine_config(
                &fixture,
                "core:default",
                &engines.definition(&fixture, "core:default").unwrap(),
                Value::Null,
            )
            .unwrap();
            let mut view = create_protocol_view(
                picker_config,
                &request("core:default").with_input("seed", 4).unwrap(),
                ViewInstanceId(1),
                &MapRouteCatalog::default(),
            )
            .unwrap();
            let context = ViewContext::new(ViewInstanceId(1), "core:default");
            let decision = view
                .event(
                    ViewEvent::Input(InputEvent::Key {
                        key: Key::Char('x'),
                        raw: Vec::new(),
                    }),
                    &context,
                )
                .unwrap();
            if !show_input {
                assert!(matches!(decision, ViewDecision::Stay));
            }
            let mut terminal = Terminal::new(TestBackend::new(20, 5)).unwrap();
            terminal
                .draw(|frame| {
                    let rendered = view
                        .render(
                            frame,
                            frame.area(),
                            &RenderContext::for_terminal(crate::view::TerminalSize {
                                width: 20,
                                height: 5,
                            }),
                        )
                        .unwrap();
                    assert!(rendered.cursor.is_none(), "{fields}");
                })
                .unwrap();
            let rows = terminal
                .backend()
                .buffer()
                .content
                .chunks(20)
                .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
                .collect::<Vec<_>>();
            assert_eq!(rows[0].starts_with("seedx"), show_input, "{fields}");
            assert_eq!(
                rows.iter().any(|row| row.chars().all(|ch| ch == '─')),
                show_input && show_divider,
                "{fields}"
            );
            if !show_input {
                assert!(matches!(
                    view.event(
                        ViewEvent::Input(InputEvent::Key {
                            key: Key::Backspace,
                            raw: Vec::new(),
                        }),
                        &context,
                    )
                    .unwrap(),
                    ViewDecision::Stay
                ));
                let mut context = context.clone();
                context.has_parent = true;
                assert!(matches!(
                    view.event(
                        ViewEvent::Input(InputEvent::Key {
                            key: Key::Backspace,
                            raw: Vec::new(),
                        }),
                        &context,
                    )
                    .unwrap(),
                    ViewDecision::Close
                ));
            }
            drop(view);
            tasks.shutdown_and_wait();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_query_backspace_returns_to_root_only_from_a_child() {
        let fixture = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let tasks = TaskRuntime::new();
        let services = crate::engine::picker::PickerRuntimeServices::new(
            fixture,
            MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(1))),
            "core:default",
        )
        .view_services();
        let mut picker_config = config_with_tasks(services, tasks.clone());
        picker_config.route_entry = false;
        picker_config.query_prefix = Some("app".to_string());
        let mut view = create_protocol_view(
            picker_config,
            &request("core:default"),
            ViewInstanceId(1),
            &MapRouteCatalog::default(),
        )
        .unwrap();
        let mut context = ViewContext::new(ViewInstanceId(1), "core:default");
        for (has_parent, expected) in [
            (true, ViewDecision::CloseToRoot),
            (false, ViewDecision::Invalidate),
        ] {
            context.has_parent = has_parent;
            assert_eq!(
                view.event(
                    ViewEvent::Input(InputEvent::Key {
                        key: Key::Backspace,
                        raw: Vec::new(),
                    }),
                    &context,
                )
                .unwrap(),
                expected,
            );
        }
        drop(view);
        tasks.shutdown_and_wait();
    }

    #[test]
    fn command_edit_updates_the_private_editor_and_rendered_cursor() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("core:default", "core:default");
        let fixture = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let services = crate::engine::picker::PickerRuntimeServices::new(
            Arc::clone(&fixture),
            MountTaskStarter::from_lease(&TaskRuntime::new(), MountTaskLease::new(ViewMountId(1))),
            "core:default",
        )
        .view_services();
        let mut picker_config = config(services);
        picker_config.query_prefix = Some("app".to_string());
        let mut view = create_protocol_view(
            picker_config,
            &request("core:default"),
            ViewInstanceId(1),
            &routes,
        )
        .unwrap();
        let mut context = ViewContext::new(ViewInstanceId(1), "core:default");
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Mounted), &mut context)
            .unwrap();
        view.event(
            ViewEvent::Lifecycle(LifecycleEvent::Activated),
            &mut context,
        )
        .unwrap();
        view.event(
            ViewEvent::Command(crate::view::CommandResult::EditInput {
                value: "rewritten".to_string(),
                cursor: 3,
            }),
            &mut context,
        )
        .unwrap();

        let mut terminal = Terminal::new(TestBackend::new(20, 5)).unwrap();
        terminal
            .draw(|frame| {
                let rendered = view
                    .render(
                        frame,
                        frame.area(),
                        &RenderContext::for_terminal(crate::view::TerminalSize {
                            width: 20,
                            height: 5,
                        }),
                    )
                    .unwrap();
                assert!(rendered.cursor.is_none());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let query = buffer.content[0..20]
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        let divider = buffer.content[20..40]
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(query.starts_with("app rewritten"), "query row: {query:?}");
        let cursor_cell = buffer.cell((7, 0)).unwrap();
        assert_eq!(cursor_cell.symbol(), "r");
        assert!(divider.chars().all(|character| character == '─'));
    }

    #[test]
    fn pseudo_cursor_styles_existing_and_trailing_cells() {
        let style = Style::default().fg(Color::Magenta).bg(Color::Green);
        let mut terminal = Terminal::new(TestBackend::new(8, 1)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new("ab界"), frame.area());
                render_pseudo_cursor(frame, frame.area(), 2, style);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((2, 0)).unwrap().symbol(), "界");
        assert_eq!(
            buffer.cell((2, 0)).unwrap().style().fg,
            Some(Color::Magenta)
        );
        assert_eq!(buffer.cell((2, 0)).unwrap().style().bg, Some(Color::Green));

        let mut terminal = Terminal::new(TestBackend::new(4, 1)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new("ab界"), frame.area());
                render_pseudo_cursor(frame, frame.area(), 3, style);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((2, 0)).unwrap().symbol(), "界");
        assert_eq!(buffer.cell((2, 0)).unwrap().style().bg, Some(Color::Green));
        assert_ne!(buffer.cell((3, 0)).unwrap().symbol(), PSEUDO_CURSOR_SYMBOL);

        let mut terminal = Terminal::new(TestBackend::new(8, 1)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new("abc"), frame.area());
                render_pseudo_cursor(frame, frame.area(), 3, style);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let cursor_cell = buffer.cell((3, 0)).unwrap();
        assert_eq!(cursor_cell.symbol(), PSEUDO_CURSOR_SYMBOL);
        assert_eq!(cursor_cell.style().fg, Some(Color::Magenta));
        assert_eq!(cursor_cell.style().bg, Some(Color::Green));
    }

    #[test]
    fn editor_cursor_uses_route_prefix_and_unicode_display_width() {
        let query = visible_editor_query(Some("app"), None, "a界bc", "a界".len(), 8);
        assert_eq!(query.text, "app a界b");
        assert_eq!(query.cursor, 7);
        assert_eq!(query.highlight, Some(0..3));

        let query = visible_editor_query(Some("app"), None, "界", "界".len(), 6);
        assert_eq!(query.text, "app 界");
        assert_eq!(query.cursor.min(5), 5);
        assert_eq!(query.highlight, Some(0..3));

        let query = visible_editor_query(None, None, "abc", 3, 3);
        assert_eq!(query.text, "abc");
        assert_eq!(query.cursor.min(2), 2);
        assert_eq!(query.highlight, None);

        let combining = "a\u{301}bc";
        let query = visible_editor_query(None, None, combining, combining.len(), 2);
        assert_eq!(query.text, "c");
        assert_eq!(query.cursor, 1);
        let emoji = "x👩‍💻yz";
        let query = visible_editor_query(None, None, emoji, "x👩‍💻".len(), 4);
        assert_eq!(query.text, "x👩‍💻y");
        assert_eq!(query.cursor, 3);
        let query = visible_editor_query(None, None, emoji, emoji.len(), 4);
        assert_eq!(query.text, "...");
        assert_eq!(query.cursor, 3);

        let query = visible_editor_query(Some("abcdef"), None, "x", 0, 3);
        assert_eq!(query.text, "abc");
        assert_eq!(query.cursor, 2);
        assert_eq!(query.highlight, None);

        let query = visible_editor_query(None, Some(3), "app needle", 10, 20);
        assert_eq!(query.highlight, Some(0..3));
    }

    #[test]
    fn completion_disabled_patch_is_respected() {
        let disabled = explicitly_disabled_keys(
            None,
            Some(&serde_json::json!({
                "tab": false,
                "escape": false,
            })),
        );
        assert!(disabled.contains(&Key::Tab.binding_identity()));
        assert!(disabled.contains(&Key::Escape.binding_identity()));
        let empty_override = explicitly_disabled_keys(Some(&serde_json::json!({"back": []})), None);
        assert!(empty_override.contains(&Key::Escape.binding_identity()));
    }

    #[test]
    fn route_query_contains_only_target_binding_values() {
        let config = crate::workflow::config::load_test_fixture().unwrap();
        let binding = config.parameter_binding("core:default").unwrap();
        let query = parsed_route_query(&binding, "core:default", "query", "needle").unwrap();
        assert_eq!(query.values, Value::String("needle".to_string()));
    }

    #[test]
    fn invalid_route_query_is_rejected_before_navigation() {
        let registry = crate::workflow::parameter::ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"target": {"views": {"detail": {"query": {
                "type": "object",
                "count": {"type": "integer"}
            }}}}}
        }))
        .unwrap();
        let binding = std::sync::Arc::new(registry)
            .parameter_binding("target:detail")
            .unwrap();
        assert!(parsed_route_query(&binding, "target:detail", "query", "bad").is_err());
    }

    #[test]
    fn picker_preview_declared_image_renders_after_decode_and_encoding() {
        let temp_dir = std::env::temp_dir().join(format!("test-picker-img-{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let image_path = temp_dir.join("test.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            64,
            64,
            image::Rgb([255, 0, 0]),
        ))
        .save(&image_path)
        .unwrap();

        let fixture = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let tasks = TaskRuntime::new();
        let starter = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(1)));
        let services =
            crate::engine::picker::PickerRuntimeServices::new(fixture, starter, "core:default")
                .view_services();
        let mut picker_config = config_with_tasks(services, tasks.clone());
        picker_config.engine.fields.insert(
            "preview".to_string(),
            serde_json::json!({
                "producer": "declared",
                "document": {"type": "image", "path": image_path.to_string_lossy()}
            }),
        );
        let mut view = create_protocol_view(
            picker_config,
            &request("core:default"),
            ViewInstanceId(1),
            &MapRouteCatalog::default(),
        )
        .unwrap();
        let context = ViewContext::new(ViewInstanceId(1), "core:default");
        let size = crate::view::TerminalSize {
            width: 80,
            height: 24,
        };
        view.event(ViewEvent::Resize(size), &context).unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
            .unwrap();
        view.event(
            ViewEvent::Input(InputEvent::Key {
                key: Key::Ctrl('p'),
                raw: vec![0x10],
            }),
            &context,
        )
        .unwrap();
        let render_context =
            RenderContext::new(size, Some(crate::terminal::ImagePicker::test_halfblocks()));
        let mut terminal = Terminal::new(TestBackend::new(size.width, size.height)).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            view.event(ViewEvent::Tick, &context).unwrap();
            for event in tasks.drain_events() {
                view.event(ViewEvent::Task(event), &context).unwrap();
            }
            terminal
                .draw(|frame| {
                    view.render(frame, frame.area(), &render_context).unwrap();
                })
                .unwrap();
            // The known red pixels must reach the preview pane through both async pools.
            if (40..size.width).any(|x| {
                terminal.backend().buffer()[(x, 2)].bg == ratatui::style::Color::Rgb(255, 0, 0)
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "declared image was never rendered: {:?}",
                terminal.backend().buffer()
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context)
            .unwrap();
        drop(view);
        tasks.shutdown_and_wait();
        std::fs::remove_dir_all(temp_dir).unwrap();
    }

    #[test]
    fn preview_events_have_their_own_registry_entry_and_render_after_items_complete() {
        let config = crate::workflow::config::CompiledConfig::load_unvalidated(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/preview/config.toml"),
        )
        .unwrap()
        .compile()
        .unwrap();
        let engines = crate::engine::EngineRegistry::new();
        let tasks = TaskRuntime::new();
        let instance = ViewInstanceId(901);
        let page = "browser:main";
        let services = super::super::mount_data(
            &config,
            &Value::Null,
            page,
            MountTaskLease::new(ViewMountId(instance.0)),
        )
        .unwrap();
        let definition = engines.definition(&config, page).unwrap();
        let protocol_config = PickerProtocolConfig {
            commands: crate::protocol::ViewCommandBindings::new(
                &config,
                page,
                crate::lifecycle::CancellationToken::new().observer(),
            )
            .unwrap(),
            identity: ViewIdentity::new(page, "picker"),
            engine: crate::engine::project_engine_config(&config, page, &definition, Value::Null)
                .unwrap(),
            bindings: crate::engine::project_binding_config(&config, page, &definition).unwrap(),
            services,
            parameter_bindings: BTreeMap::new(),
            parameter_binding: config.parameter_binding(page).unwrap(),
            theme: ResolvedTheme::terminal(),
            route_entry: false,
            query_prefix: None,
            runtime_snapshot: serde_json::json!({"view":{}}),
            tasks: tasks.clone(),
        };
        let request = NavigationRequest::new(
            page,
            ParsedQuery::new(
                page,
                "query",
                serde_json::json!({"search":"","owner":"browser"}),
            ),
        );
        let mut view = create_protocol_view(
            protocol_config,
            &request,
            instance,
            &crate::view::MapRouteCatalog::default(),
        )
        .unwrap();
        let context = ViewContext::new(instance, page);
        view.event(
            ViewEvent::Resize(crate::view::TerminalSize {
                width: 80,
                height: 24,
            }),
            &context,
        )
        .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
            .unwrap();
        view.event(
            ViewEvent::Input(InputEvent::Key {
                key: Key::Ctrl('p'),
                raw: vec![0x10],
            }),
            &context,
        )
        .unwrap();
        let mut events = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            view.event(ViewEvent::Tick, &context).unwrap();
            for event in tasks.drain_events() {
                events.push(event.clone());
                view.event(ViewEvent::Task(event), &context).unwrap();
            }
            if events.iter().any(|event| event.task == TaskId(2)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(events.iter().any(|event| event.task == TaskId(1)));
        let preview_event = events
            .iter()
            .find(|event| event.task == TaskId(2))
            .expect("preview event missing");
        assert_eq!(preview_event.instance, instance);
        // Duplicate and stale preview events cannot consume an item completion.
        assert!(matches!(
            view.event(ViewEvent::Task(preview_event.clone()), &context)
                .unwrap(),
            ViewDecision::Stay
        ));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| {
                view.render(
                    frame,
                    frame.area(),
                    &RenderContext::for_terminal(crate::view::TerminalSize {
                        width: 80,
                        height: 24,
                    }),
                )
                .unwrap();
            })
            .unwrap();
        let content = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(
            content.contains("Mixed preview")
                && content.contains("Details")
                && content.contains("library"),
            "{content}"
        );
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Covered), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context)
            .unwrap();
        drop(view);
        tasks.shutdown_and_wait();
    }
}
