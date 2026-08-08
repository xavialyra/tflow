use super::PickerView;
use super::keymap::PickerAction;
use crate::input::Key;

pub(super) enum PickerInputAction {
    Continue,
    Refresh,
    ClearError,
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
        input: &mut String,
        nested: bool,
    ) -> PickerInputAction {
        if let Some(action) = self.keymap.action(key) {
            return self.apply_picker_action(action, command_view, input, nested);
        }
        if command_available && self.current().command_owner.is_none() {
            return PickerInputAction::Activate(key);
        }
        match key {
            Key::Char(character) if !character.is_control() => {
                input.push(character);
                PickerInputAction::Refresh
            }
            _ => PickerInputAction::Continue,
        }
    }

    fn apply_picker_action(
        &mut self,
        action: PickerAction,
        command_view: &str,
        input: &mut String,
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
                } else if !input.is_empty() {
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
                if input.pop().is_some() {
                    PickerInputAction::Refresh
                } else {
                    PickerInputAction::Continue
                }
            }
            PickerAction::ClearInput => {
                if input.is_empty() {
                    PickerInputAction::Continue
                } else {
                    input.clear();
                    PickerInputAction::Refresh
                }
            }
            PickerAction::DeleteWord => {
                let previous_length = input.len();
                while input.chars().last().is_some_and(char::is_whitespace) {
                    input.pop();
                }
                while !input.chars().last().is_some_and(char::is_whitespace) && !input.is_empty() {
                    input.pop();
                }
                if input.len() != previous_length {
                    PickerInputAction::Refresh
                } else {
                    PickerInputAction::Continue
                }
            }
            PickerAction::Activate => PickerInputAction::Activate(Key::Enter),
        }
    }
}
