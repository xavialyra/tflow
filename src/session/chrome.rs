use super::AppSession;
use super::state::{ChromeHint, RouteCompletion};
use crate::config::CommandBindingVisibility;
use crate::engine::{EngineHost, InputFocus};
use crate::input::Key;
use crate::input::keymap::BindingState;
use crate::terminal::Terminal;
use crate::theme::Theme;
use anyhow::{Context, Result};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

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

    pub(super) fn current_chrome(&mut self, width: usize) -> Result<crate::chrome::ChromeFrame> {
        self.refresh_active_bindings()?;
        let (binding_commands, binding_overflow) = self.current_binding_hints();
        let route_completion_available = self.route_input_available();
        let route_completion_active = self.route_completion.is_some();
        let route_completion_opens_on_tab =
            route_completion_available && self.resolve_key_binding(Key::Tab).is_none();
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        let route = (self.config.default_view.as_deref() != Some(entry.view_ref.as_str()))
            .then(|| self.router.display(&entry.view_ref));
        let error = self
            .active_error
            .as_ref()
            .map(|record| record.label.clone());
        let input_focus = entry.instance.input_focus();
        let input_text = match input_focus {
            InputFocus::Focused => entry.input.raw.clone(),
            InputFocus::Unfocused => entry.input.params.clone(),
        };
        let input_cursor = match input_focus {
            InputFocus::Focused => entry.input.cursor,
            InputFocus::Unfocused => input_text.len(),
        };
        let host = EngineHost {
            config: self.config,
            theme: self.theme,
            input: &mut entry.input,
            state: &mut entry.state,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        let mut engine_chrome = entry.instance.chrome(&host);
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
            engine_chrome.status = Some(format!(
                "{} / {} views",
                usize::from(!completion.candidates.is_empty()).saturating_add(completion.selected),
                completion.candidates.len()
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

    pub(super) fn render(&mut self, terminal: &mut Terminal) -> Result<()> {
        let chrome = self.current_chrome(terminal.size().0 as usize)?;
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        let host = EngineHost {
            config: self.config,
            theme: self.theme,
            input: &mut entry.input,
            state: &mut entry.state,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        let route_completion = self.route_completion.clone();
        terminal.draw(|frame| {
            let content_area = chrome.render_chrome(frame, &host.theme);
            if let Some(completion) = &route_completion {
                render_route_completion(frame, content_area, completion, &host.theme);
            } else {
                entry.instance.render(&host, frame, content_area);
            }
            if entry.instance.input_focus() == InputFocus::Focused {
                chrome.set_input_cursor(frame);
            }
        })
    }
}

pub(super) fn render_route_completion(
    frame: &mut Frame,
    area: Rect,
    completion: &RouteCompletion,
    theme: &Theme,
) {
    let height = area.height as usize;
    let width = area.width as usize;
    if height == 0 || width == 0 {
        return;
    }
    if completion.candidates.is_empty() {
        frame.render_widget(
            Paragraph::new("(no matching views)").style(theme.picker.muted),
            area,
        );
        return;
    }

    let start = if completion.selected >= height {
        completion.selected + 1 - height
    } else {
        0
    };
    let lines = completion
        .candidates
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(index, candidate)| {
            let reference = candidate
                .alias
                .as_ref()
                .map(|_| format!("  {}", candidate.view_ref))
                .unwrap_or_default();
            let metadata = format!("  [{} / {}]", candidate.plugin_name, candidate.engine_type);
            let label = format!("  {}", candidate.primary_label());
            let text = crate::chrome::clip(&format!("{label}{reference}{metadata}"), width);
            if index == completion.selected {
                Line::from(vec![
                    Span::styled("▌", theme.picker.marker),
                    Span::styled(
                        text.strip_prefix(' ').unwrap_or(&text).to_string(),
                        theme.picker.selected,
                    ),
                ])
            } else {
                let label_end = label.len().min(text.len());
                Line::from(vec![
                    Span::styled(text[..label_end].to_string(), theme.picker.text),
                    Span::styled(text[label_end..].to_string(), theme.picker.muted),
                ])
            }
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).style(theme.picker.text), area);
}
