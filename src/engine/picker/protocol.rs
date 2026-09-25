//! Protocol-native adapter for the Picker engine.
//!
//! The adapter owns the interactive editor. The existing PickerView remains
//! responsible for item loading, selection, and preview

use super::{
    PickerKeymap, PickerOptions, PickerView, PickerViewServices, PrefixBackspace,
    create_input_bindings, create_renderer,
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
    RelativeCursor, RenderContext, RenderResult, View, ViewCommandSnapshot, ViewContext,
    ViewDecision, ViewEvent, ViewPublication, ViewTaskRegistry,
};
use crate::workflow::parameter::{ParameterBinding, ParameterSnapshot};
use anyhow::Result;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::Paragraph,
};
use serde_json::Value;
use std::{collections::HashSet, ops::Range};

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
        show_left_prefix: config
            .engine
            .field("show_left_prefix")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        prefix_backspace: config.prefix_backspace,
        // An empty hint is treated as no hint so it never renders a blank row.
        input_placeholder: config
            .engine
            .field("input_placeholder")
            .and_then(Value::as_str)
            .filter(|placeholder| !placeholder.is_empty())
            .map(str::to_string),
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
        disabled_keys,
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
        task_completion_pending: false,
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
) -> Result<EditorBuffer> {
    if let Some(seed) = input {
        return Ok(EditorBuffer::from_raw(seed.text.clone(), seed.cursor));
    }
    let state = binding.state_from_snapshot(snapshot, false)?;
    let rendered = binding.render_input(&state)?;
    Ok(EditorBuffer::from_raw(rendered.clone(), rendered.len()))
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
    keymap: PickerKeymap,
    disabled_keys: HashSet<crate::input::BindingKey>,
    left_prefix: Option<String>,
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
    defer_work_poll: bool,
    task_completion_pending: bool,
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

    fn body_layout(&self, area: Rect) -> [Rect; 3] {
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
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(query_height),
                Constraint::Length(divider_height),
                Constraint::Min(0),
            ])
            .split(area);

        std::array::from_fn(|i| layout[i])
    }

    fn sync_auxiliary_size(&mut self) {
        let body = self.body_layout(Rect::new(0, 0, self.content_size.0, self.content_size.1))[2];
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
        match key {
            Key::Char(character) => {
                self.editor.insert(character);
                self.edit_changed(context)
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
                } else if self.editor.raw.is_empty() && self.rendered_left_prefix().is_some() {
                    // The only editable thing left is the rendered left prefix.
                    // Both opt-in behaviors consume it by leaving this View;
                    // unset (or a View without a prefix) leaves Backspace inert.
                    match self.options.prefix_backspace {
                        Some(PrefixBackspace::Parent) => Ok(ViewDecision::Close),
                        Some(PrefixBackspace::Root) => Ok(ViewDecision::CloseToRoot),
                        None => Ok(ViewDecision::Invalidate),
                    }
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

    fn retained_content_area(&self, area: Rect) -> Option<Rect> {
        // The freshly mounted input line and divider belong to this instance.
        // Only the item list and preview may briefly show pixels retained from
        // the View this Picker replaced.
        Some(self.body_layout(area)[2])
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
                if !context.has_parent && !self.editor.raw.is_empty() {
                    self.editor.clear();
                    self.edit_changed(context)
                } else {
                    self.action(context, "picker.back")
                }
            }
            CMD_SELECT_PREVIOUS => self.action(context, "picker.select_previous"),
            CMD_SELECT_NEXT => self.action(context, "picker.select_next"),
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

    fn clear_input(&mut self, context: &ViewContext) -> Result<()> {
        if self.editor.raw.is_empty() {
            return Ok(());
        }
        self.editor.clear();
        self.parse_editor(context)?;
        self.sync_auxiliary_size();
        Ok(())
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
            ViewEvent::Lifecycle(LifecycleEvent::TransitionCommitted { .. })
            | ViewEvent::Lifecycle(LifecycleEvent::TransitionRejected { .. }) => {
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
            | ViewEvent::Input(InputEvent::Bytes(_)) => Ok(ViewDecision::Stay),
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
        let left_prefix = self.rendered_left_prefix();
        let query = visible_editor_query(
            left_prefix,
            self.options.input_placeholder.as_deref(),
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
            if let Some(placeholder) = query.placeholder.clone() {
                if placeholder.start > offset {
                    spans.push(Span::styled(
                        query.text[offset..placeholder.start].to_string(),
                        self.theme.picker.text,
                    ));
                }
                spans.push(Span::styled(
                    query.text[placeholder.clone()].to_string(),
                    self.theme.picker.placeholder,
                ));
                offset = placeholder.end;
            }
            if offset < query.text.len() {
                spans.push(Span::styled(
                    query.text[offset..].to_string(),
                    self.theme.picker.text,
                ));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), layout[0]);
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
        let model = self.runtime.render_model();
        self.renderer.validate_model(&model)?;
        self.renderer.render(
            &model,
            &crate::engine::RenderContext {
                theme: self.theme.clone(),
                image_picker: context.image_picker,
            },
            frame,
            layout[2],
        );
        let cursor =
            (self.active && self.options.show_input && query_height > 0).then(|| RelativeCursor {
                x: query.cursor,
                y: 0,
                visible: true,
            });

        Ok(RenderResult {
            cursor,
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
            self.apply_key(context, key)
        })();
        self.sync_auxiliary_size();
        result
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

struct VisibleEditorQuery {
    text: String,
    cursor: u16,
    highlight: Option<Range<usize>>,
    /// Byte range of the rendered input placeholder, if any. Never overlaps
    /// `highlight`: it only exists while the raw input is empty.
    placeholder: Option<Range<usize>>,
}

fn visible_editor_query(
    left_prefix: Option<&str>,
    placeholder: Option<&str>,
    raw: &str,
    cursor: usize,
    width: usize,
) -> VisibleEditorQuery {
    let cursor = crate::input::previous_char_boundary(raw, cursor);
    let left_prefix = left_prefix.filter(|prefix| !prefix.is_empty());
    let prefix = left_prefix
        .map(|prefix| format!("{prefix} "))
        .unwrap_or_default();
    let prefix_width = UnicodeWidthStr::width(prefix.as_str());
    let highlight = |output_end: usize| {
        left_prefix
            .filter(|prefix| prefix.len() <= output_end)
            .map(|prefix| 0..prefix.len())
    };
    if width <= prefix_width {
        let text = crate::ui::chrome::clip(&prefix, width);
        return VisibleEditorQuery {
            highlight: highlight(text.len()),
            text,
            cursor: width.saturating_sub(1) as u16,
            placeholder: None,
        };
    }

    let available = width - prefix_width;
    let input_width = UnicodeWidthStr::width(raw);
    let cursor_width = UnicodeWidthStr::width(&raw[..cursor]);
    if input_width <= available {
        if raw.is_empty()
            && let Some(placeholder) = placeholder.filter(|placeholder| !placeholder.is_empty())
        {
            let clipped = crate::ui::chrome::clip(placeholder, available);
            let text = format!("{prefix}{clipped}");
            let placeholder = (!clipped.is_empty()).then(|| prefix.len()..text.len());
            return VisibleEditorQuery {
                highlight: highlight(prefix.len()),
                placeholder,
                text,
                cursor: prefix_width as u16,
            };
        }
        let text = format!("{prefix}{raw}");
        return VisibleEditorQuery {
            highlight: highlight(text.len()),
            text,
            cursor: (prefix_width + cursor_width) as u16,
            placeholder: None,
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
        highlight: highlight(text.len()),
        text,
        cursor: (prefix_width + marker_width + local_cursor) as u16,
        placeholder: None,
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
