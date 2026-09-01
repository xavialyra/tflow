use super::AppSession;
use super::completion::{SelectionRenderModel, render_completion};
use super::state::ChromeHint;
use crate::config::CommandBindingVisibility;
use crate::engine::{InputFocus, RenderContext};
use crate::input::Key;
use crate::input::keymap::BindingState;
use crate::terminal::Terminal;
use crate::theme::Theme;
use anyhow::{Context, Result};
use ratatui::Frame;
use ratatui::layout::Rect;

impl AppSession<'_> {
    fn current_binding_hints(&self) -> (Vec<ChromeHint>, Option<ChromeHint>) {
        let Some(context) = self.active_input_context().ok() else {
            return (Vec::new(), None);
        };
        let snapshot = self.input_router.snapshot(context.id);
        debug_assert_eq!(snapshot.context(), context.id);
        let mut commands = Vec::new();
        let mut overflow = None;
        for (key, id) in snapshot.bindings() {
            let Some(record) = self.input_router.record(id) else {
                continue;
            };
            if record.state == BindingState::Disabled {
                continue;
            }
            let Some(hint) = &record.target.hint else {
                continue;
            };
            let Some(key) = key.binding_name() else {
                continue;
            };
            let value = (key, hint.label.clone());
            match hint.visibility {
                CommandBindingVisibility::Always => commands.push(value),
                CommandBindingVisibility::Overflow => overflow = Some(value),
                CommandBindingVisibility::Hidden => {}
            }
        }
        commands.sort_by(|left, right| crate::command::compare_bindings(&left.0, &right.0));
        (commands, overflow)
    }

    #[cfg(test)]
    pub(super) fn current_chrome(&mut self, width: usize) -> Result<crate::chrome::ChromeFrame> {
        self.refresh_active_bindings()?;
        let model = {
            let entry = self.views.last().context("session has no active view")?;
            let model = entry.runtime.render_model();
            entry.renderer.validate_model(&model)?;
            model
        };
        self.current_chrome_for_model(width, &model)
    }

    fn current_chrome_for_model(
        &self,
        width: usize,
        model: &crate::engine::RenderModel,
    ) -> Result<crate::chrome::ChromeFrame> {
        let (binding_commands, binding_overflow) = self.current_binding_hints();
        let route_completion_available = self.route_input_available();
        let route_completion_active = self.route_completion.is_some();
        let route_completion_opens_on_tab =
            route_completion_available && self.resolve_key_binding(Key::Tab).is_none();
        let entry = self.views.last().context("session has no active view")?;
        entry.renderer.validate_model(model)?;
        let route = (self.config.default_view.as_deref() != Some(entry.view_ref.as_str()))
            .then(|| self.router.display(&entry.view_ref));
        let error = self
            .active_error
            .as_ref()
            .map(|record| record.label.clone());
        let input_focus = entry.input_focus();
        let input_text = match input_focus {
            InputFocus::Focused => entry.input.raw.clone(),
            InputFocus::Unfocused => entry.committed_input().to_string(),
        };
        let input_cursor = match input_focus {
            InputFocus::Focused => entry.input.cursor,
            InputFocus::Unfocused => input_text.len(),
        };
        let mut engine_chrome = entry.renderer.chrome(model);
        engine_chrome.commands = binding_commands;
        engine_chrome.overflow_command = binding_overflow;
        if route_completion_available
            && let Some(end) = self
                .router
                .recognized_prefix_end(&entry.view_ref, &entry.input.raw)
        {
            engine_chrome.presentation =
                engine_chrome.presentation.with_recognized_input_prefix(end);
        }
        if let Some(completion) = &self.route_completion {
            let model = completion.render_model();
            engine_chrome.status = Some(format!(
                "{} / {} views",
                usize::from(!model.candidates.is_empty()).saturating_add(model.selected),
                model.candidates.len()
            ));
        }
        if route_completion_active {
            engine_chrome.overflow_command = None;
        } else if route_completion_opens_on_tab {
            engine_chrome.commands.retain(|(key, _)| key != "tab");
        }
        if input_focus == InputFocus::Unfocused {
            engine_chrome.presentation = engine_chrome.presentation.with_unfocused_input();
        }
        Ok(crate::chrome::ChromeFrame::compose_with_cursor(
            width,
            route.as_ref(),
            &input_text,
            input_cursor,
            engine_chrome,
            error.as_deref(),
        ))
    }

    pub(super) fn active_content_size(&self, terminal_size: (u16, u16)) -> Result<(u16, u16)> {
        let entry = self.views.last().context("session has no active view")?;
        let route = (self.config.default_view.as_deref() != Some(entry.view_ref.as_str()))
            .then(|| self.router.display(&entry.view_ref));
        let model = entry.runtime.render_model();
        entry.renderer.validate_model(&model)?;
        let chrome = entry.renderer.chrome(&model);
        let input = match entry.input_focus() {
            InputFocus::Focused => entry.input.raw.as_str(),
            InputFocus::Unfocused => entry.committed_input(),
        };
        let frame = crate::chrome::ChromeFrame::compose_with_cursor(
            terminal_size.0 as usize,
            route.as_ref(),
            input,
            input.len(),
            chrome,
            None,
        );
        let area = frame.content_area(Rect::new(0, 0, terminal_size.0, terminal_size.1));
        Ok((area.width, area.height))
    }

    pub(super) fn render(&mut self, terminal: &mut Terminal) -> Result<()> {
        self.refresh_active_bindings()?;
        let model = {
            let entry = self.views.last().context("session has no active view")?;
            let model = entry.runtime.render_model();
            entry.renderer.validate_model(&model)?;
            model
        };
        let chrome = self.current_chrome_for_model(terminal.size().0 as usize, &model)?;
        let input_focus = self
            .views
            .last()
            .context("session has no active view")?
            .input_focus();
        let route_completion = self
            .route_completion
            .as_ref()
            .map(|completion| completion.render_model());
        let render_context = RenderContext {
            theme: self.theme,
            image_picker: terminal.image_picker(),
        };
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        terminal.draw(|frame| {
            let content_area = chrome.render_chrome(frame, &self.theme);
            if let Some(completion) = route_completion.as_ref() {
                render_route_completion(frame, content_area, completion, &self.theme);
            } else {
                entry
                    .renderer
                    .render(&model, &render_context, frame, content_area);
            }
            if input_focus == InputFocus::Focused {
                chrome.set_input_cursor(frame);
            }
        })
    }
}

pub(super) fn render_route_completion(
    frame: &mut Frame,
    area: Rect,
    completion: &SelectionRenderModel,
    theme: &Theme,
) {
    render_completion(frame, area, completion, theme);
}
