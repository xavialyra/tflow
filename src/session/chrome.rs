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
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

const DEFAULT_POPUP_WIDTH: u16 = 72;
const DEFAULT_POPUP_HEIGHT: u16 = 16;

impl AppSession<'_> {
    fn binding_hints_for(
        &self,
        context: crate::input::InputContext,
    ) -> (Vec<ChromeHint>, Option<ChromeHint>) {
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

    fn base_view_index(&self) -> Result<usize> {
        let active = self
            .views
            .len()
            .checked_sub(1)
            .context("session has no active view")?;
        Ok(
            if active > 0
                && self.views[active].presentation.mode
                    == crate::config::ViewPresentationMode::Popup
            {
                active - 1
            } else {
                active
            },
        )
    }

    #[cfg(test)]
    pub(super) fn current_chrome(&mut self, width: usize) -> Result<crate::chrome::ChromeFrame> {
        self.refresh_active_bindings()?;
        let index = self.base_view_index()?;
        let model = {
            let entry = &self.views[index];
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
        let index = self.base_view_index()?;
        let entry = &self.views[index];
        let route_completion_available = self.route_input_available();
        let route_completion_active = self.route_completion.is_some();
        let context = if route_completion_active {
            self.active_input_context()?
        } else {
            entry.input_context()
        };
        let (binding_commands, binding_overflow) = self.binding_hints_for(context);
        let route_completion_opens_on_tab =
            route_completion_available && self.resolve_key_binding(Key::Tab).is_none();
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
        let active_index = self
            .views
            .len()
            .checked_sub(1)
            .context("session has no active view")?;
        let base_index = self.base_view_index()?;
        let chrome_entry = &self.views[base_index];
        let entry = &self.views[active_index];
        let route = (self.config.default_view.as_deref() != Some(chrome_entry.view_ref.as_str()))
            .then(|| self.router.display(&chrome_entry.view_ref));
        let model = chrome_entry.runtime.render_model();
        chrome_entry.renderer.validate_model(&model)?;
        let chrome = chrome_entry.renderer.chrome(&model);
        let input = match chrome_entry.input_focus() {
            InputFocus::Focused => chrome_entry.input.raw.as_str(),
            InputFocus::Unfocused => chrome_entry.committed_input(),
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
        let area = if active_index != base_index {
            render_area_for_presentation(area, &entry.presentation, entry.input_focus())
        } else {
            area
        };
        Ok((area.width, area.height))
    }

    pub(super) fn render(&mut self, terminal: &mut Terminal) -> Result<()> {
        self.refresh_active_bindings()?;
        let active_index = self
            .views
            .len()
            .checked_sub(1)
            .context("session has no active view")?;
        let (model, renderer, presentation) = {
            let entry = &self.views[active_index];
            let model = entry.runtime.render_model();
            entry.renderer.validate_model(&model)?;
            (model, &*entry.renderer, entry.presentation.clone())
        };
        let base_index = self.base_view_index()?;
        let base_model = {
            let entry = &self.views[base_index];
            let model = entry.runtime.render_model();
            entry.renderer.validate_model(&model)?;
            model
        };
        let chrome = self.current_chrome_for_model(terminal.size().0 as usize, &base_model)?;
        let input_focus = self.views[active_index].input_focus();
        let route_completion = self
            .route_completion
            .as_ref()
            .map(|completion| completion.render_model());
        let parent = (active_index != base_index).then(|| {
            let entry = &self.views[base_index];
            (base_model.clone(), &*entry.renderer)
        });
        let render_context = RenderContext {
            theme: self.theme,
            image_picker: terminal.image_picker(),
        };
        terminal.draw(|frame| {
            let content_area = chrome.render_chrome(frame, &self.theme);
            if let Some(completion) = route_completion.as_ref() {
                render_route_completion(frame, content_area, completion, &self.theme);
            } else if let Some((parent_model, parent_renderer)) = parent.as_ref() {
                parent_renderer.render(parent_model, &render_context, frame, content_area);
                render_popup(
                    frame,
                    content_area,
                    &presentation,
                    renderer,
                    &model,
                    &self.views[active_index].input,
                    input_focus,
                    &render_context,
                    &self.theme,
                );
            } else {
                renderer.render(&model, &render_context, frame, content_area);
            }
            if active_index == base_index && input_focus == InputFocus::Focused {
                chrome.set_input_cursor(frame);
            }
        })
    }
}

fn render_area_for_presentation(
    area: Rect,
    presentation: &crate::config::ViewPresentation,
    input_focus: InputFocus,
) -> Rect {
    if presentation.mode != crate::config::ViewPresentationMode::Popup {
        return area;
    }
    popup_body_area(popup_area(area, presentation), input_focus)
}

fn popup_area(area: Rect, presentation: &crate::config::ViewPresentation) -> Rect {
    let width = presentation
        .width
        .unwrap_or(DEFAULT_POPUP_WIDTH)
        .min(area.width);
    let height = presentation
        .height
        .unwrap_or(DEFAULT_POPUP_HEIGHT)
        .min(area.height);
    Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    )
}

fn popup_inner_area(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

fn popup_body_area(area: Rect, input_focus: InputFocus) -> Rect {
    let inner = popup_inner_area(area);
    if input_focus == InputFocus::Focused && inner.height > 0 {
        Rect::new(
            inner.x,
            inner.y.saturating_add(1),
            inner.width,
            inner.height.saturating_sub(1),
        )
    } else {
        inner
    }
}

fn render_popup(
    frame: &mut Frame,
    area: Rect,
    presentation: &crate::config::ViewPresentation,
    renderer: &dyn crate::engine::ViewRenderer,
    model: &crate::engine::RenderModel,
    input: &crate::input::EditorBuffer,
    input_focus: InputFocus,
    context: &RenderContext,
    theme: &Theme,
) {
    let popup = popup_area(area, presentation);
    frame.render_widget(Clear, popup);
    frame.render_widget(Block::bordered().border_style(theme.preview.border), popup);
    let inner = popup_inner_area(popup);
    if input_focus == InputFocus::Focused && inner.height > 0 {
        let input_area = Rect::new(inner.x, inner.y, inner.width, 1);
        let input_text = format!("> {}", input.raw);
        frame.render_widget(Paragraph::new(input_text).style(theme.text), input_area);
    }
    let body = popup_body_area(popup, input_focus);
    renderer.render(model, context, frame, body);
    if input_focus == InputFocus::Focused && inner.height > 0 {
        let cursor = input
            .raw
            .get(..input.cursor)
            .map(UnicodeWidthStr::width)
            .unwrap_or(0)
            .saturating_add(2)
            .min(inner.width.saturating_sub(1) as usize);
        frame.set_cursor_position((inner.x.saturating_add(cursor as u16), inner.y));
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

#[cfg(test)]
mod tests {
    use super::{popup_area, popup_inner_area};
    use crate::config::{ViewPresentation, ViewPresentationMode};
    use ratatui::layout::Rect;

    #[test]
    fn popup_area_is_centered_and_clamped() {
        let presentation = ViewPresentation {
            mode: ViewPresentationMode::Popup,
            width: Some(20),
            height: Some(8),
        };
        assert_eq!(
            popup_area(Rect::new(10, 5, 80, 30), &presentation),
            Rect::new(40, 16, 20, 8)
        );
        assert_eq!(
            popup_area(Rect::new(0, 0, 12, 4), &presentation),
            Rect::new(0, 0, 12, 4)
        );
    }

    #[test]
    fn popup_inner_area_reserves_a_border() {
        assert_eq!(
            popup_inner_area(Rect::new(10, 5, 20, 8)),
            Rect::new(11, 6, 18, 6)
        );
    }
}
