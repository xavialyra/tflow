//! Native form View. Query parameters are immutable inputs to content production;
//! editable drafts are published separately for ordinary workflow commands.
mod content;
mod render;
#[cfg(test)]
mod tests;

use self::content::{Draft, FieldType, parse_content};
use crate::engine::{
    EngineDefinition, EngineValidationContext, FactoryFieldPlan, ProjectedBindingConfig,
    ProjectedEngineConfig,
};
use crate::input::keymap::{ActionBindings, KeymapAction};
use crate::input::{InputEvent, Key, ViewMountId};
use crate::protocol::contracts::{TaskId, ViewInstanceId};
use crate::task::{
    MountTaskLease, MountTaskStarter, TaskCompletion, TaskHandle, TaskRuntime, TaskTags,
};
use crate::ui::theme::ResolvedTheme;
use crate::view::{
    FallbackInputReceiver, LifecycleEvent, NavigationRequest, View, ViewChrome,
    ViewCommandSnapshot, ViewContext, ViewDecision, ViewEvent, ViewPublication, ViewTaskRegistry,
};
use crate::workflow::config::{
    Defaults, ProducerKind, ResolvedScriptSource, parse_producer_script_handler, toml_to_json,
};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum FormAction {
    FocusNext,
    FocusPrev,
    Cancel,
    Exit,
}

impl FormAction {
    const ALL: [Self; 4] = [Self::FocusNext, Self::FocusPrev, Self::Cancel, Self::Exit];
}

impl KeymapAction for FormAction {
    const LABEL: &'static str = "form";

    fn name(self) -> &'static str {
        match self {
            Self::FocusNext => "focus_next",
            Self::FocusPrev => "focus_prev",
            Self::Cancel => "cancel",
            Self::Exit => "exit",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.name() == name)
    }

    fn default_bindings() -> &'static [(Key, Self)] {
        &[
            (Key::Tab, Self::FocusNext),
            (Key::Down, Self::FocusNext),
            (Key::BackTab, Self::FocusPrev),
            (Key::Up, Self::FocusPrev),
            (Key::Escape, Self::Cancel),
            (Key::Ctrl('c'), Self::Exit),
            (Key::Ctrl('d'), Self::Exit),
        ]
    }
}

pub(super) type FormKeymap = ActionBindings<FormAction>;

pub(super) fn definition() -> EngineDefinition {
    EngineDefinition::new().with_factory_fields(FactoryFieldPlan {
        runtime: &["content"],
        binding: &[],
        binding_defaults: Some(&["defaults", "form", "bindings"]),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Producer {
    producer: ProducerKind,
    handler: Value,
}

enum PreparedContent {
    Declared(Vec<Draft>),
    Script(ResolvedScriptSource),
}

fn prepare(value: Value, root: Option<&std::path::Path>) -> Result<PreparedContent> {
    let producer: Producer =
        serde_json::from_value(value).context("form content must define producer and handler")?;
    match producer.producer {
        ProducerKind::Declared => Ok(PreparedContent::Declared(parse_content(producer.handler)?)),
        ProducerKind::Script => {
            let handler = toml::Value::try_from(producer.handler)?;
            Ok(PreparedContent::Script(parse_producer_script_handler(
                &handler, root,
            )?))
        }
    }
}

pub(super) fn validate_defaults(defaults: &Defaults) -> Result<()> {
    let bindings = defaults
        .form
        .bindings
        .as_ref()
        .map(toml_to_json)
        .transpose()?;
    FormKeymap::validate_values(bindings.as_ref(), None).context("form bindings")
}

pub(super) fn validate_config(context: EngineValidationContext<'_>) -> Result<()> {
    let view = context.view;
    ensure!(
        view.selected_items().is_none(),
        "form views cannot provide picker items"
    );
    crate::engine::validate_fields(context.view_ref, view, &["content"])?;
    crate::engine::require_field(context.view_ref, view, "content")?;
    prepare(
        toml_to_json(view.engine_field("content").unwrap())?,
        context.script_root,
    )?;
    Ok(())
}

pub(crate) struct FormProtocolConfig {
    pub(crate) engine: ProjectedEngineConfig,
    pub(crate) bindings: ProjectedBindingConfig,
    pub(crate) runtime_snapshot: Value,
    pub(crate) raw_input: String,
    pub(crate) theme: ResolvedTheme,
    pub(crate) tasks: TaskRuntime,
}

pub(crate) fn create_protocol_view(
    config: FormProtocolConfig,
    request: &NavigationRequest,
    instance: ViewInstanceId,
) -> Result<Box<dyn View>> {
    Ok(Box::new(FormView::new(config, request, instance)?))
}

struct FormView {
    instance: ViewInstanceId,
    target: String,
    keymap: FormKeymap,
    parameters: Value,
    raw_input: String,
    runtime_snapshot: Value,
    theme: ResolvedTheme,
    fields: Vec<Draft>,
    focus: usize,
    content_size: (u16, u16),
    publication: ViewPublication,
    revision: u64,
    script: Option<ResolvedScriptSource>,
    workflow_root: Option<std::path::PathBuf>,
    launch_input: Value,
    starter: MountTaskStarter,
    task: Option<TaskHandle<Value>>,
    task_generation: u64,
    registry: ViewTaskRegistry,
    error: Option<String>,
    active: bool,
    closed: bool,
}

impl FormView {
    fn new(
        config: FormProtocolConfig,
        request: &NavigationRequest,
        instance: ViewInstanceId,
    ) -> Result<Self> {
        let prepared = prepare(
            config
                .engine
                .field("content")
                .context("form requires content")?
                .clone(),
            config.engine.workflow_root.as_deref(),
        )?;
        let (fields, script) = match prepared {
            PreparedContent::Declared(fields) => (fields, None),
            PreparedContent::Script(source) => (Vec::new(), Some(source)),
        };
        let ready = script.is_none();
        let publication = ViewPublication::new(content::state(&fields, 0, ready), ready);
        let keymap =
            FormKeymap::from_values(config.bindings.defaults, config.bindings.view_keymap)?;
        Ok(Self {
            instance,
            target: request.target.clone(),
            keymap,
            parameters: request.query.values.clone(),
            raw_input: config.raw_input,
            runtime_snapshot: config.runtime_snapshot,
            theme: config.theme,
            fields,
            focus: 0,
            content_size: (0, 0),
            publication,
            revision: 0,
            script,
            workflow_root: config.engine.workflow_root,
            launch_input: config.engine.launch_input,
            starter: MountTaskStarter::from_lease(
                &config.tasks,
                MountTaskLease::new(ViewMountId(instance.0)),
            ),
            task: None,
            task_generation: 0,
            registry: ViewTaskRegistry::new(instance),
            error: None,
            active: false,
            closed: false,
        })
    }

    fn status(&self) -> String {
        let base = if self.error.is_some() {
            "Form unavailable"
        } else if !self.publication.ready {
            "Loading form…"
        } else if self.publication.current["valid"] == false {
            "Invalid"
        } else if self.publication.current["dirty"] == true {
            "Modified"
        } else {
            "Ready"
        };
        if self.fields.is_empty() {
            base.to_string()
        } else {
            format!("{base} · {} of {}", self.focus + 1, self.fields.len())
        }
    }

    fn publish(&mut self) {
        self.publication.current = content::state(&self.fields, self.focus, self.publication.ready);
        self.revision = self.revision.wrapping_add(1);
    }

    fn start(&mut self) {
        if self.task.is_some() {
            return;
        }
        let Some(source) = self.script.clone() else {
            return;
        };
        let owner = self.target.clone();
        let root = self.workflow_root.clone();
        let request = crate::protocol::form_request(
            &self.parameters,
            &self.launch_input,
            &self.publication.current,
        );
        self.task_generation = self.task_generation.wrapping_add(1).max(1);
        self.registry.register(TaskId(1), self.task_generation);
        self.task = Some(
            self.starter
                .for_task(TaskId(1), self.task_generation)
                .spawn_latest_tagged(
                    "form-content",
                    TaskTags::new("form", "content"),
                    move |context| {
                        let response = crate::protocol::run_script_form_response(
                            &owner,
                            root.as_deref(),
                            &source,
                            &request,
                            &context.cancellation.observer(),
                        );
                        if response.managed_child_reaped {
                            context.mark_process_reaped();
                        }
                        response.result.map_err(|error| format!("{error:#}"))
                    },
                ),
        );
    }

    fn poll(&mut self) -> ViewDecision {
        let Some(task) = self.task.as_mut() else {
            return ViewDecision::Stay;
        };
        let result = match task.try_recv() {
            Ok(TaskCompletion::Completed(value)) => parse_content(value),
            Ok(TaskCompletion::Failed(error)) => Err(anyhow::anyhow!(error)),
            Ok(TaskCompletion::Cancelled) => {
                Err(anyhow::anyhow!("form content loading was cancelled"))
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => return ViewDecision::Stay,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(anyhow::anyhow!(
                "form content task stopped without a result"
            )),
        };
        self.task = None;
        self.script = None;
        self.registry.invalidate_all();
        match result {
            Ok(fields) => {
                self.fields = fields;
                self.publication.ready = true;
            }
            Err(error) => {
                self.error = Some(format!("{error:#}"));
            }
        }
        self.publish();
        ViewDecision::Invalidate
    }

    fn edit(&mut self, key: Key) -> ViewDecision {
        if self.fields.is_empty() {
            return ViewDecision::Stay;
        }
        let draft = &mut self.fields[self.focus];
        let buffer = &mut draft.buffer;
        match key {
            Key::Char(' ') | Key::Enter | Key::Right if draft.field.kind == FieldType::Boolean => {
                let text = if buffer.raw.trim() == "true" {
                    "false"
                } else {
                    "true"
                };
                buffer.replace_all(text.to_string(), text.len());
            }
            Key::Left if draft.field.kind == FieldType::Boolean => {
                let text = if buffer.raw.trim() == "false" {
                    "true"
                } else {
                    "false"
                };
                buffer.replace_all(text.to_string(), text.len());
            }
            Key::Home | Key::Ctrl('a') if draft.field.kind == FieldType::Boolean => {
                buffer.replace_all("true".to_string(), 4);
            }
            Key::End | Key::Ctrl('e') if draft.field.kind == FieldType::Boolean => {
                buffer.replace_all("false".to_string(), 5);
            }
            Key::Char('t') | Key::Char('T') if draft.field.kind == FieldType::Boolean => {
                buffer.replace_all("true".to_string(), 4);
            }
            Key::Char('f') | Key::Char('F') if draft.field.kind == FieldType::Boolean => {
                buffer.replace_all("false".to_string(), 5);
            }
            Key::Char(' ') | Key::Right if draft.field.kind == FieldType::Enum => {
                draft.prefix_len = 0;
                if let Some(options) = &draft.field.options
                    && !options.is_empty()
                {
                    let next = match options.iter().position(|opt| opt == buffer.raw.trim()) {
                        Some(idx) => &options[(idx + 1) % options.len()],
                        None => &options[0],
                    };
                    buffer.replace_all(next.clone(), next.len());
                }
            }
            Key::Left if draft.field.kind == FieldType::Enum => {
                draft.prefix_len = 0;
                if let Some(options) = &draft.field.options
                    && !options.is_empty()
                {
                    let prev = match options.iter().position(|opt| opt == buffer.raw.trim()) {
                        Some(idx) => &options[(idx + options.len() - 1) % options.len()],
                        None => options.last().unwrap(),
                    };
                    buffer.replace_all(prev.clone(), prev.len());
                }
            }
            Key::Home | Key::Ctrl('a') if draft.field.kind == FieldType::Enum => {
                draft.prefix_len = 0;
                if let Some(options) = &draft.field.options
                    && let Some(first) = options.first()
                {
                    buffer.replace_all(first.clone(), first.len());
                }
            }
            Key::End | Key::Ctrl('e') if draft.field.kind == FieldType::Enum => {
                draft.prefix_len = 0;
                if let Some(options) = &draft.field.options
                    && let Some(last) = options.last()
                {
                    buffer.replace_all(last.clone(), last.len());
                }
            }
            Key::Char(c) if draft.field.kind == FieldType::Enum && !c.is_control() => {
                if let Some(options) = &draft.field.options {
                    let c_lower = c.to_lowercase().to_string();
                    let current_val = buffer.raw.trim();
                    let opt_match = options.iter().find(|opt| opt.as_str() == current_val);

                    let matched_continuation = if let Some(opt) = opt_match {
                        if draft.prefix_len < opt.len() {
                            let next_char = opt[draft.prefix_len..]
                                .chars()
                                .next()
                                .map(|ch| ch.to_lowercase().to_string());
                            next_char == Some(c_lower.clone())
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    if matched_continuation {
                        draft.prefix_len += c.len_utf8();
                    } else {
                        let current_pos = options.iter().position(|opt| opt == current_val);
                        let start_idx = current_pos.map(|p| p + 1).unwrap_or(0);
                        let matched = (0..options.len())
                            .map(|i| (start_idx + i) % options.len())
                            .find(|&idx| {
                                let opt = &options[idx];
                                opt.chars()
                                    .next()
                                    .is_some_and(|ch| ch.to_lowercase().to_string() == c_lower)
                            });
                        if let Some(idx) = matched {
                            let chosen = &options[idx];
                            buffer.replace_all(chosen.clone(), chosen.len());
                            draft.prefix_len = c.len_utf8();
                        }
                    }
                }
            }
            Key::Backspace | Key::Delete | Key::Ctrl('u') | Key::Ctrl('w')
                if draft.field.kind == FieldType::Enum
                    || draft.field.kind == FieldType::Boolean =>
            {
                draft.prefix_len = 0;
                buffer.clear();
            }
            Key::Char(_) | Key::Home | Key::End | Key::Ctrl('a') | Key::Ctrl('e')
                if draft.field.kind == FieldType::Boolean =>
            {
                return ViewDecision::Stay;
            }
            Key::Char(c) if !c.is_control() => buffer.insert(c),
            Key::Left => buffer.move_left(),
            Key::Right => buffer.move_right(),
            Key::Home | Key::Ctrl('a') => buffer.move_home(),
            Key::End | Key::Ctrl('e') => buffer.move_end(),
            Key::Backspace => {
                buffer.delete_backward();
            }
            Key::Delete => {
                buffer.delete_forward();
            }
            Key::Ctrl('u') => {
                buffer.clear();
            }
            Key::Ctrl('w') => {
                buffer.delete_word();
            }
            _ => return ViewDecision::Stay,
        }
        self.publish();
        ViewDecision::Invalidate
    }
}

impl FallbackInputReceiver for FormView {
    fn on_unbound_key(
        &mut self,
        key: Key,
        _raw: &[u8],
        _context: &ViewContext,
    ) -> Result<ViewDecision> {
        if self.active && self.publication.ready {
            Ok(self.edit(key))
        } else {
            Ok(ViewDecision::Stay)
        }
    }
}

pub(super) const CMD_FOCUS_NEXT: &str = "form.focus_next";
pub(super) const CMD_FOCUS_PREV: &str = "form.focus_prev";
pub(super) const CMD_CANCEL: &str = "form.cancel";
pub(super) const CMD_EXIT: &str = "form.exit";

impl View for FormView {
    fn engine_commands(&self, _context: &ViewContext) -> Vec<crate::command::CommandEntry> {
        let mut entries = Vec::new();
        for (key, action) in self.keymap.bindings() {
            let (id, label) = match action {
                FormAction::FocusNext => (CMD_FOCUS_NEXT, "Next Field"),
                FormAction::FocusPrev => (CMD_FOCUS_PREV, "Previous Field"),
                FormAction::Cancel => (CMD_CANCEL, "Cancel"),
                FormAction::Exit => (CMD_EXIT, "Exit"),
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

    fn on_command(&mut self, id: &str, _context: &ViewContext) -> Result<ViewDecision> {
        match id {
            CMD_FOCUS_NEXT => {
                if !self.fields.is_empty() {
                    self.focus = (self.focus + 1) % self.fields.len();
                    self.publish();
                    Ok(ViewDecision::Invalidate)
                } else {
                    Ok(ViewDecision::Stay)
                }
            }
            CMD_FOCUS_PREV => {
                if !self.fields.is_empty() {
                    self.focus = (self.focus + self.fields.len() - 1) % self.fields.len();
                    self.publish();
                    Ok(ViewDecision::Invalidate)
                } else {
                    Ok(ViewDecision::Stay)
                }
            }
            CMD_CANCEL => Ok(ViewDecision::Close),
            CMD_EXIT => Ok(ViewDecision::Exit),
            _ => Ok(ViewDecision::Stay),
        }
    }

    fn fallback_receiver(&mut self) -> Option<&mut dyn FallbackInputReceiver> {
        Some(self)
    }

    fn publication(&self) -> Option<&ViewPublication> {
        Some(&self.publication)
    }

    fn command_snapshot(&self) -> ViewCommandSnapshot {
        ViewCommandSnapshot {
            engine_type: "form".into(),
            parameters: self.parameters.clone(),
            raw_input: self.raw_input.clone(),
            runtime: self.runtime_snapshot.clone(),
            publication: Some(self.publication.clone()),
            revision: self.revision,
        }
    }

    fn chrome(&self, _context: &ViewContext) -> Result<ViewChrome> {
        Ok(ViewChrome {
            status: Some(self.status()),
            error: self.error.clone(),
            bindings: None,
            overflow_command: None,
            has_unbound: false,
        })
    }

    fn event(&mut self, event: ViewEvent, context: &ViewContext) -> Result<ViewDecision> {
        ensure!(
            context.instance == self.instance,
            "form context belongs to another instance"
        );
        if self.closed {
            return Ok(ViewDecision::Stay);
        }
        Ok(match event {
            ViewEvent::Lifecycle(LifecycleEvent::Activated) => {
                self.active = true;
                self.start();
                ViewDecision::Invalidate
            }
            ViewEvent::Lifecycle(LifecycleEvent::Covered) => {
                self.active = false;
                ViewDecision::Stay
            }
            ViewEvent::Lifecycle(
                lifecycle @ (LifecycleEvent::Closing | LifecycleEvent::Closed),
            ) => {
                self.active = false;
                // Closing can be rolled back by Router. Retain completed drafts
                // and the producer source for an interrupted initial load.
                self.closed = lifecycle == LifecycleEvent::Closed;
                self.starter.cancel_all();
                self.task = None;
                if self.closed {
                    self.script = None;
                }
                self.registry.invalidate_all();
                ViewDecision::Stay
            }
            ViewEvent::Task(task) if self.registry.accepts(&task) => self.poll(),
            ViewEvent::Input(InputEvent::Eof) => ViewDecision::Exit,
            ViewEvent::Input(InputEvent::Key { key, raw }) if self.active => {
                self.dispatch_key_event(key, &raw, context)?
            }
            ViewEvent::Input(InputEvent::Paste {
                text: Some(text), ..
            }) if self.active && self.publication.ready => {
                if let Some(field) = self.fields.get_mut(self.focus) {
                    if field.field.kind == FieldType::Enum {
                        field.prefix_len = 0;
                        let trimmed = text.trim();
                        field.buffer.replace_all(trimmed.to_string(), trimmed.len());
                    } else if field.field.kind == FieldType::Boolean {
                        let trimmed = text.trim();
                        let normalized = match trimmed.to_lowercase().as_str() {
                            "true" | "1" | "yes" | "y" | "t" => "true",
                            "false" | "0" | "no" | "n" | "f" => "false",
                            _ => trimmed,
                        };
                        field
                            .buffer
                            .replace_all(normalized.to_string(), normalized.len());
                    } else {
                        field.buffer.insert_text(&text);
                    }
                    self.publish();
                    ViewDecision::Invalidate
                } else {
                    ViewDecision::Stay
                }
            }
            ViewEvent::Resize(size) => {
                self.content_size = (size.width, size.height);
                ViewDecision::Invalidate
            }
            _ => ViewDecision::Stay,
        })
    }

    fn render(
        &self,
        frame: &mut ratatui::Frame,
        area: ratatui::layout::Rect,
        _: &crate::view::RenderContext,
    ) -> Result<crate::view::RenderResult> {
        Ok(self.render_form(frame, area))
    }
}

impl Drop for FormView {
    fn drop(&mut self) {
        self.starter.cancel_all();
    }
}
