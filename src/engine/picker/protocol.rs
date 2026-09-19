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
    EffectRequest, FallbackInputReceiver, LifecycleEvent, NavigationRequest, ParsedQuery,
    RenderContext, RenderResult, RouteCandidate, RouteCatalog, TransitionRequest, View,
    ViewCommandSnapshot, ViewContext, ViewDecision, ViewEvent, ViewPublication, ViewTaskRegistry,
};
use crate::workflow::parameter::{ParameterBinding, ParameterSnapshot};
use anyhow::{Context, Result};
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

pub(super) const CMD_EXIT: &str = "picker.exit";
pub(super) const CMD_BACK: &str = "picker.back";
pub(super) const CMD_SELECT_PREVIOUS: &str = "picker.select_previous";
pub(super) const CMD_SELECT_NEXT: &str = "picker.select_next";
pub(super) const CMD_TOGGLE_PREVIEW: &str = "picker.toggle_preview";
pub(super) const CMD_PREVIEW_SCROLL_UP: &str = "picker.preview_scroll_up";
pub(super) const CMD_PREVIEW_SCROLL_DOWN: &str = "picker.preview_scroll_down";
pub(super) const CMD_CLEAR_INPUT: &str = "picker.clear_input";
pub(super) const CMD_DELETE_WORD: &str = "picker.delete_word";
pub(super) const CMD_DELETE_BACKWARD: &str = "picker.delete_backward";

#[derive(Clone)]
pub(crate) struct PickerProtocolConfig {
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
    if let Some(focus) = &request.focus {
        runtime.set_initial_focus(Some(focus.clone()));
    }
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

struct PickerProtocolView {
    runtime: Box<dyn EngineRuntime>,
    renderer: Box<dyn crate::engine::ViewRenderer>,
    options: PickerOptions,
    keymap: PickerKeymap,
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
            EngineDecision::Close => Ok(ViewDecision::Close),
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
            if self.completion.take().is_some() {
                self.state_revision = self.state_revision.wrapping_add(1);
            }
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
        self.state_revision = self.state_revision.wrapping_add(1);
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
        self.state_revision = self.state_revision.wrapping_add(1);
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
}

impl View for PickerProtocolView {
    fn preferred_top_inset(&self) -> u16 {
        1
    }

    fn publication(&self) -> Option<&ViewPublication> {
        self.publication.as_ref()
    }

    fn chrome(&self, _context: &ViewContext) -> Result<crate::view::ViewChrome> {
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
            bindings: None,
            overflow_command: None,
            has_unbound: false,
        })
    }

    fn engine_commands(&self, _context: &ViewContext) -> Vec<crate::command::CommandEntry> {
        let mut entries = Vec::new();
        for (key, action) in self.keymap.bindings() {
            let (id, label) = match action {
                super::keymap::PickerAction::Exit => (CMD_EXIT, "Exit"),
                super::keymap::PickerAction::Back => (CMD_BACK, "Back"),
                super::keymap::PickerAction::SelectPrevious => {
                    (CMD_SELECT_PREVIOUS, "Select Previous")
                }
                super::keymap::PickerAction::SelectNext => (CMD_SELECT_NEXT, "Select Next"),
                super::keymap::PickerAction::TogglePreview => {
                    (CMD_TOGGLE_PREVIEW, "Toggle Preview")
                }
                super::keymap::PickerAction::PreviewScrollUp => {
                    (CMD_PREVIEW_SCROLL_UP, "Scroll Preview Up")
                }
                super::keymap::PickerAction::PreviewScrollDown => {
                    (CMD_PREVIEW_SCROLL_DOWN, "Scroll Preview Down")
                }
                super::keymap::PickerAction::ClearInput => (CMD_CLEAR_INPUT, "Clear Input"),
                super::keymap::PickerAction::DeleteWord => (CMD_DELETE_WORD, "Delete Word"),
                super::keymap::PickerAction::DeleteBackward => (CMD_DELETE_BACKWARD, "Delete"),
            };

            entries.push(crate::command::CommandEntry::for_event(
                id,
                Some(label.to_string()),
                Some(key),
                crate::command::CommandScope::Engine,
            ));
        }

        entries
    }

    fn fallback_receiver(&mut self) -> Option<&mut dyn FallbackInputReceiver> {
        Some(self)
    }

    fn on_command(&mut self, id: &str, context: &ViewContext) -> Result<ViewDecision> {
        self.rebuild_context(context);
        let result = match id {
            CMD_EXIT => Ok(ViewDecision::Exit),
            CMD_BACK => {
                if self.completion.is_some() {
                    self.completion = None;
                    self.state_revision = self.state_revision.wrapping_add(1);
                    Ok(ViewDecision::Invalidate)
                } else {
                    self.action(context, "picker.back")
                }
            }
            CMD_SELECT_PREVIOUS => {
                if self.completion_move(-1) {
                    Ok(ViewDecision::Invalidate)
                } else {
                    self.action(context, "picker.select_previous")
                }
            }
            CMD_SELECT_NEXT => {
                if self.completion_move(1) {
                    Ok(ViewDecision::Invalidate)
                } else {
                    self.action(context, "picker.select_next")
                }
            }
            CMD_TOGGLE_PREVIEW => self.action(context, "picker.toggle_preview"),
            CMD_PREVIEW_SCROLL_UP => self.action(context, "picker.preview_scroll_up"),
            CMD_PREVIEW_SCROLL_DOWN => self.action(context, "picker.preview_scroll_down"),
            CMD_CLEAR_INPUT => {
                self.editor.clear();
                self.edit_changed(context)
            }
            CMD_DELETE_WORD => {
                self.editor.delete_word();
                self.edit_changed(context)
            }
            CMD_DELETE_BACKWARD => self.apply_key(context, Key::Backspace),
            _ => Ok(ViewDecision::Stay),
        };
        self.sync_auxiliary_size();
        result
    }

    fn command_snapshot(&self) -> ViewCommandSnapshot {
        let in_completion = self.completion.is_some();
        ViewCommandSnapshot {
            engine_type: self.engine_context.view_identity().engine_type.clone(),
            parameters: self.parameters.values().clone(),
            raw_input: self.editor.raw.clone(),
            runtime: self.runtime_snapshot.clone(),
            publication: if in_completion {
                None
            } else {
                self.publication.clone()
            },
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
                self.defer_work_poll = false;
                self.task_completion_pending = false;
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Lifecycle(LifecycleEvent::Closing) => {
                self.active = false;
                self.task_registry.invalidate_all();
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
            ViewEvent::Input(InputEvent::Key { key, raw }) => {
                self.dispatch_key_event(key, &raw, context)
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
                    self.state_revision = self.state_revision.wrapping_add(1);
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
                        Ok(decision)
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
                        return Ok(decision);
                    }
                }
                let emission = self.runtime.tick(EngineTick {
                    context: self.engine_context.clone(),
                    content_size: self.content_size,
                })?;
                let decision = self.map_emission(context, emission)?;
                self.start_prepared_work();
                Ok(decision)
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
                bindings: None,
            },
        })
    }
}

impl FallbackInputReceiver for PickerProtocolView {
    fn on_unbound_key(
        &mut self,
        key: Key,
        _raw: &[u8],
        context: &ViewContext,
    ) -> Result<ViewDecision> {
        self.rebuild_context(context);
        let result = (|| {
            if self.disabled_keys.contains(&key.binding_identity()) {
                return Ok(ViewDecision::Stay);
            }
            if (key == Key::Tab || key == Key::Enter) && self.completion.is_some() {
                return self.completion_accept(context);
            }
            if key == Key::Tab {
                self.open_completion();
                return Ok(ViewDecision::Invalidate);
            }
            if key == Key::Enter
                && self.completion.is_none()
                && let Some(decision) = self.route_submission()?
            {
                return Ok(decision);
            }
            self.apply_key(context, key)
        })();
        self.sync_auxiliary_size();
        result
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
mod tests;
