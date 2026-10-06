//! Protocol-native adapter for the Picker engine.
//!
//! The Host owns editing and input presentation. Picker retains a read-only
//! input snapshot and owns item loading, selection, and parameter parsing.

use super::{
    PickerBindings, PickerOptions, PickerView, PickerViewServices, PrefixBackspace, create_renderer,
};
use crate::engine::{
    ActionId, BackgroundOutcome, EngineActionInput, EngineDecision, EngineEmission,
    EngineNavigationRequest, EngineRuntime, EngineRuntimeSnapshot, EngineTick,
    ProjectedBindingConfig, ProjectedEngineConfig, RendererFactoryContext,
    ViewContext as EngineContext, ViewIdentity,
};
use crate::input::bindings::BindingAction;
use crate::input::{
    EditorBuffer, EditorSnapshot, InputEvent, InputSourceIdentity, Key, ViewMountId,
};
#[cfg(test)]
use crate::protocol::contracts::{TaskEvent, TaskOutcome};
use crate::protocol::contracts::{TaskId, ViewInstanceId};
use crate::task::{MountTaskLease, MountTaskStarter, TaskRuntime};
use crate::ui::theme::ResolvedTheme;
use crate::view::{
    EffectRequest, FallbackInputReceiver, InputEdit, LifecycleEvent, NavigationRequest,
    ParsedQuery, RenderContext, RenderResult, View, ViewCommandSnapshot, ViewContext, ViewDecision,
    ViewEvent, ViewPublication, ViewTaskRegistry,
};
use crate::workflow::parameter::{ParameterBinding, ParameterSnapshot};
use anyhow::Result;
use ratatui::{Frame, layout::Rect};
use serde_json::Value;

pub(super) const CMD_EXIT: &str = "picker.exit";
pub(super) const CMD_BACK: &str = "picker.back";
pub(super) const CMD_SELECT_PREVIOUS: &str = "picker.select_previous";
pub(super) const CMD_SELECT_NEXT: &str = "picker.select_next";
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
    pub(crate) theme: ResolvedTheme,
    /// Marker rendered at the start of a non-root View's input line. Purely
    /// presentational; it never affects key handling.
    pub(crate) left_prefix: Option<String>,
    /// Backspace behavior on an empty, prefixed input line. `None` leaves
    /// Backspace inert.
    pub(crate) prefix_backspace: Option<super::PrefixBackspace>,
    pub(crate) runtime_snapshot: Value,
    pub(crate) tasks: TaskRuntime,
}

pub(crate) fn create_protocol_view(
    config: PickerProtocolConfig,
    request: &NavigationRequest,
    instance: ViewInstanceId,
) -> Result<Box<dyn View>> {
    anyhow::ensure!(
        request.query.target == config.identity.view_ref,
        "picker request target {:?} does not match configured View {:?}",
        request.query.target,
        config.identity.view_ref
    );
    let mount_id = ViewMountId(instance.0);
    let typed_picker = config.engine.as_picker();
    let options = PickerOptions {
        show_input: typed_picker.and_then(|p| p.show_input).unwrap_or(true),
        show_divider: typed_picker.and_then(|p| p.show_divider).unwrap_or(true),
        show_left_prefix: typed_picker
            .and_then(|p| p.show_left_prefix)
            .unwrap_or(true),
        prefix_backspace: config.prefix_backspace,
        // An empty hint is treated as no hint so it never renders a blank row.
        input_placeholder: typed_picker
            .and_then(|p| p.input_placeholder.as_deref())
            .filter(|placeholder| !placeholder.is_empty())
            .map(str::to_string),
    };
    let mut runtime_services = config.services.clone();
    runtime_services.launch_input = config.engine.launch_input.clone();
    let mut runtime = PickerView::new(&config.identity.view_ref, runtime_services);
    if let Some(focus) = &request.focus {
        runtime.set_initial_focus(Some(focus.clone()));
    }
    let runtime: Box<dyn EngineRuntime> = Box::new(runtime);
    let resolved = PickerBindings::from_defaults(config.bindings.defaults.clone())?;
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
        editor.clone(),
    );
    let starter = MountTaskStarter::from_lease(&config.tasks, MountTaskLease::new(mount_id))
        .with_execution_class(request.execution_class);
    Ok(Box::new(PickerProtocolView {
        runtime,
        renderer,
        bindings: resolved,
        left_prefix: config.left_prefix,
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
        defer_work_poll: false,
        publication_ready: false,
        diagnostic: None,
        content_size: (1, 1),
        has_parent: false,
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
) -> Result<EditorSnapshot> {
    if let Some(seed) = input {
        return Ok(EditorBuffer::from_raw(seed.text.clone(), seed.cursor).snapshot());
    }
    let state = binding.state_from_snapshot(snapshot, false)?;
    let rendered = binding.render_input(&state)?;
    Ok(EditorBuffer::from_raw(rendered.clone(), rendered.len()).snapshot())
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

struct PickerProtocolView {
    runtime: Box<dyn EngineRuntime>,
    renderer: Box<dyn crate::engine::ViewRenderer>,
    options: PickerOptions,
    bindings: PickerBindings,
    left_prefix: Option<String>,
    theme: ResolvedTheme,
    parameter_binding: ParameterBinding,
    editor: EditorSnapshot,
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
    defer_work_poll: bool,
    publication_ready: bool,
    diagnostic: Option<String>,
    content_size: (u16, u16),
    /// Whether this View was pushed on top of a parent, cached from the mount
    /// context so the render path can show a parent hint without a ViewContext.
    has_parent: bool,
}

impl PickerProtocolView {
    fn rebuild_context(&mut self, context: &ViewContext) {
        debug_assert_eq!(context.instance, self.instance);
        self.has_parent = context.has_parent;
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
            self.editor.clone(),
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

    fn start_prepared_work(&mut self) {
        let task = TaskId(1);
        let generation = self.task_generation.wrapping_add(1).max(1);
        let starter = self.starter.for_task(task, generation);
        if self.runtime.start_prepared_work(&starter) {
            self.task_generation = generation;
            self.task_registry.register(task, generation);
        }
    }

    fn rendered_left_prefix(&self) -> Option<&str> {
        self.left_prefix
            .as_deref()
            .filter(|_| self.options.show_left_prefix && self.has_parent)
    }

    fn map_emission(
        &mut self,
        context: &ViewContext,
        emission: EngineEmission,
    ) -> Result<ViewDecision> {
        if let Some(publication) = emission.publication() {
            self.publication_ready = publication.ready;
            let current = publication.current().clone();
            let publication_changed = self.publication.as_ref().is_none_or(|snapshot| {
                snapshot.current != current || snapshot.ready != publication.ready
            });
            if publication_changed {
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
            EngineDecision::Navigate(EngineNavigationRequest { target, .. }) => {
                // The Picker runtime never emits engine navigation; route jumps
                // are workflow commands. Keep a defensive error for other hosts.
                anyhow::bail!(
                    "Picker engine navigation to {:?} is unsupported; bind a navigate command instead",
                    target
                )
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

    fn apply_key(&mut self, context: &ViewContext, key: Key) -> Result<ViewDecision> {
        if !self.options.show_input {
            return match key {
                Key::Backspace if context.has_parent => self.action(context, "picker.back"),
                _ => Ok(ViewDecision::Stay),
            };
        }
        if key == Key::Backspace && self.editor.raw.is_empty() {
            return Ok(if self.rendered_left_prefix().is_some() {
                match self.options.prefix_backspace {
                    Some(PrefixBackspace::Parent) => ViewDecision::Close,
                    Some(PrefixBackspace::Root) => ViewDecision::CloseToRoot,
                    None => ViewDecision::Invalidate,
                }
            } else {
                ViewDecision::Invalidate
            });
        }
        Ok(ViewDecision::EditInput(InputEdit::Key(key)))
    }
}

impl View for PickerProtocolView {
    fn unhandled_input_behavior(&self) -> crate::view::UnhandledInputBehavior {
        crate::view::UnhandledInputBehavior::ForwardToOmnibar
    }

    fn input_mode(&self) -> crate::ui::chrome::InputPresentationMode {
        if self.options.show_input {
            crate::ui::chrome::InputPresentationMode::Omnibar { show_cursor: true }
        } else {
            crate::ui::chrome::InputPresentationMode::Hidden
        }
    }

    fn input_placeholder(&self) -> Option<String> {
        self.options.input_placeholder.clone()
    }

    fn input_left_prefix(&self) -> Option<String> {
        self.rendered_left_prefix().map(str::to_string)
    }

    fn initial_input(&self) -> EditorSnapshot {
        self.editor.clone()
    }

    fn input_divider(&self) -> bool {
        self.options.show_divider
    }

    fn follows_companion_data(&self) -> bool {
        true
    }

    fn on_companion_data_changed(
        &mut self,
        data: &crate::view::companion::CompanionData,
        context: &ViewContext,
    ) -> Result<ViewDecision> {
        let text = match &data.input {
            Value::String(text) => text.clone(),
            Value::Null => match &data.parameters {
                Value::String(text) => text.clone(),
                Value::Null => String::new(),
                value => value.to_string(),
            },
            value => value.to_string(),
        };
        let input = crate::input::EditorBuffer::from_raw(text.clone(), text.len()).snapshot();
        self.on_host_input_changed(&input, context)
    }

    fn on_host_input_changed(
        &mut self,
        input: &EditorSnapshot,
        context: &ViewContext,
    ) -> Result<ViewDecision> {
        let text_changed = self.editor.raw != input.raw;
        self.editor = input.clone();
        self.rebuild_context(context);
        if text_changed {
            self.edit_changed(context)
        } else {
            Ok(ViewDecision::Invalidate)
        }
    }

    fn on_input_changed(&mut self, text: &str, context: &ViewContext) -> Result<ViewDecision> {
        let input = EditorSnapshot {
            raw: text.to_string(),
            cursor: text.len(),
            revision: self.editor.revision.wrapping_add(1),
        };
        self.on_host_input_changed(&input, context)
    }

    fn preferred_top_inset(&self) -> u16 {
        1
    }

    fn retained_content_area(&self, area: Rect) -> Option<Rect> {
        Some(area)
    }

    fn publication(&self) -> Option<&ViewPublication> {
        self.publication.as_ref()
    }

    fn chrome(&self, _context: &ViewContext) -> Result<crate::view::ViewChrome> {
        let model = self.runtime.render_model();
        self.renderer.validate_model(&model)?;
        let status = self.renderer.chrome(&model).status;
        Ok(crate::view::ViewChrome {
            status,
            error: self.diagnostic.clone(),
            bindings: None,
            has_unbound: false,
        })
    }

    fn engine_commands(&self, _context: &ViewContext) -> Vec<crate::command::CommandEntry> {
        let mut entries = Vec::new();
        for (key, action) in self.bindings.bindings() {
            let id = match action {
                super::bindings::PickerAction::Exit => CMD_EXIT,
                super::bindings::PickerAction::Back => CMD_BACK,
                super::bindings::PickerAction::SelectPrevious => CMD_SELECT_PREVIOUS,
                super::bindings::PickerAction::SelectNext => CMD_SELECT_NEXT,
                super::bindings::PickerAction::ClearInput => CMD_CLEAR_INPUT,
                super::bindings::PickerAction::DeleteWord => CMD_DELETE_WORD,
                super::bindings::PickerAction::DeleteBackward => CMD_DELETE_BACKWARD,
            };

            entries.push(crate::command::CommandEntry::for_event(
                id,
                Some(action.label().to_string()),
                Some(key),
                crate::command::BindingLayer::Engine,
            ));
        }

        entries
    }

    fn fallback_receiver(&mut self) -> Option<&mut dyn FallbackInputReceiver> {
        Some(self)
    }

    fn on_command(&mut self, id: &str, context: &ViewContext) -> Result<ViewDecision> {
        self.rebuild_context(context);
        match id {
            CMD_EXIT => Ok(ViewDecision::Exit),
            CMD_BACK => {
                if !context.has_parent && !self.editor.raw.is_empty() {
                    Ok(ViewDecision::EditInput(InputEdit::Clear))
                } else {
                    self.action(context, "picker.back")
                }
            }
            CMD_SELECT_PREVIOUS => self.action(context, "picker.select_previous"),
            CMD_SELECT_NEXT => self.action(context, "picker.select_next"),
            CMD_CLEAR_INPUT => Ok(ViewDecision::EditInput(InputEdit::Clear)),
            CMD_DELETE_WORD => Ok(ViewDecision::EditInput(InputEdit::DeleteWord)),
            CMD_DELETE_BACKWARD => self.apply_key(context, Key::Backspace),
            _ => Ok(ViewDecision::Stay),
        }
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
        match event {
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
                self.defer_work_poll = false;
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Lifecycle(LifecycleEvent::Closing) => {
                self.active = false;
                self.task_registry.invalidate_all();
                self.defer_work_poll = false;
                self.runtime.deactivate();
                self.starter.cancel_all();
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Lifecycle(LifecycleEvent::Closed) => {
                self.closed = true;
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Lifecycle(LifecycleEvent::TransitionCommitted { .. })
            | ViewEvent::Lifecycle(LifecycleEvent::TransitionRejected { .. }) => {
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Input(InputEvent::Key { .. }) => {
                anyhow::bail!("key input must be resolved by the command registry")
            }
            ViewEvent::Input(InputEvent::Paste {
                text: Some(text), ..
            }) => Ok(ViewDecision::EditInput(InputEdit::Paste(text))),
            ViewEvent::Input(InputEvent::Paste { text: None, .. })
            | ViewEvent::Input(InputEvent::Bytes(_)) => Ok(ViewDecision::Stay),
            ViewEvent::Input(InputEvent::Eof) => Ok(ViewDecision::Exit),
            ViewEvent::Task(task) => {
                if !self.task_registry.accepts(&task) {
                    return Ok(ViewDecision::Stay);
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
        }
    }

    fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        context: &RenderContext,
    ) -> Result<RenderResult> {
        let model = self.runtime.render_model();
        self.renderer.validate_model(&model)?;
        self.renderer.render(
            &model,
            &crate::engine::RenderContext {
                theme: self.theme.clone(),
                image_picker: context.image_picker,
            },
            frame,
            area,
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
        self.apply_key(context, key)
    }
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
