use super::PickerView;
use super::keymap::PickerAction;
use crate::chrome::ShellInput;
use crate::input::Key;
use crate::router::ViewCandidate;

#[derive(Clone)]
pub(crate) struct ViewCompletion {
    pub(crate) candidates: Vec<ViewCandidate>,
    pub(crate) selected: usize,
    pub(crate) selector_start: usize,
    pub(crate) selector_end: usize,
}

pub(super) enum PickerInputAction {
    Continue,
    Refresh,
    ClearError,
    OpenCompletion,
    CycleCompletion(isize),
    AcceptCompletion,
    CloseCompletion,
    Activate(Key),
    OpenCommandView,
    Back,
    Exit,
}

impl PickerView {
    pub(super) fn handle_input(
        &mut self,
        key: Key,
        command_view: &str,
        command_available: bool,
        input: &mut ShellInput,
        nested: bool,
    ) -> PickerInputAction {
        if self.completion_active() {
            match key {
                Key::Escape => return PickerInputAction::CloseCompletion,
                Key::Enter => return PickerInputAction::AcceptCompletion,
                Key::Tab => return PickerInputAction::CycleCompletion(1),
                Key::BackTab | Key::Up => return PickerInputAction::CycleCompletion(-1),
                Key::Down => return PickerInputAction::CycleCompletion(1),
                _ => self.close_completion(),
            }
        }

        if key == Key::Tab {
            return PickerInputAction::OpenCompletion;
        }
        if key == Key::BackTab {
            return PickerInputAction::OpenCompletion;
        }
        if let Some(action) = self.keymap.action(key) {
            return self.apply_picker_action(action, command_view, input, nested);
        }
        match key {
            Key::Left => {
                input.move_left();
                return PickerInputAction::Continue;
            }
            Key::Right => {
                input.move_right();
                return PickerInputAction::Continue;
            }
            Key::Home => {
                input.move_home();
                return PickerInputAction::Continue;
            }
            Key::End => {
                input.move_end();
                return PickerInputAction::Continue;
            }
            Key::Delete => {
                return if input.delete_forward() {
                    PickerInputAction::Refresh
                } else {
                    PickerInputAction::Continue
                };
            }
            _ => {}
        }
        if command_available && self.current().command_owner.is_none() {
            return PickerInputAction::Activate(key);
        }
        match key {
            Key::Char(character) if !character.is_control() => {
                input.insert(character);
                PickerInputAction::Refresh
            }
            _ => PickerInputAction::Continue,
        }
    }

    fn apply_picker_action(
        &mut self,
        action: PickerAction,
        command_view: &str,
        input: &mut ShellInput,
        nested: bool,
    ) -> PickerInputAction {
        match action {
            PickerAction::Exit => PickerInputAction::Exit,
            PickerAction::OpenCommands => {
                if self.current().view != command_view {
                    PickerInputAction::OpenCommandView
                } else {
                    PickerInputAction::Continue
                }
            }
            PickerAction::Back => {
                if nested {
                    PickerInputAction::Back
                } else if !input.raw.is_empty() {
                    input.clear();
                    PickerInputAction::Refresh
                } else {
                    PickerInputAction::Back
                }
            }
            PickerAction::SelectPrevious => {
                if !self.current().items.is_empty() {
                    self.current_mut().selected = self.current().selected.saturating_sub(1);
                }
                PickerInputAction::ClearError
            }
            PickerAction::SelectNext => {
                if !self.current().items.is_empty() {
                    let last = self.current().items.len() - 1;
                    self.current_mut().selected = (self.current().selected + 1).min(last);
                }
                PickerInputAction::ClearError
            }
            PickerAction::DeleteBackward => {
                if input.delete_backward() {
                    PickerInputAction::Refresh
                } else {
                    PickerInputAction::Continue
                }
            }
            PickerAction::ClearInput => {
                if input.clear() {
                    PickerInputAction::Refresh
                } else {
                    PickerInputAction::Continue
                }
            }
            PickerAction::DeleteWord => {
                if input.delete_word() {
                    PickerInputAction::Refresh
                } else {
                    PickerInputAction::Continue
                }
            }
            PickerAction::Activate => PickerInputAction::Activate(Key::Enter),
        }
    }
}
