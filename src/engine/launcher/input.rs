use super::LauncherView;
use super::keymap::LauncherAction;
use crate::input::Key;

pub(super) enum LauncherInputAction {
    Continue,
    Refresh,
    ClearError,
    Activate(Key),
    OpenCommandView,
    Back,
    Exit,
}

impl LauncherView {
    pub(super) fn handle_input(
        &mut self,
        key: Key,
        command_view: &str,
        command_available: bool,
        input: &mut String,
        nested: bool,
    ) -> LauncherInputAction {
        if let Some(action) = self.keymap.action(key) {
            return self.apply_launcher_action(action, command_view, input, nested);
        }
        if command_available && self.current().command_owner.is_none() {
            return LauncherInputAction::Activate(key);
        }
        match key {
            Key::Char(character) if !character.is_control() => {
                input.push(character);
                LauncherInputAction::Refresh
            }
            _ => LauncherInputAction::Continue,
        }
    }

    fn apply_launcher_action(
        &mut self,
        action: LauncherAction,
        command_view: &str,
        input: &mut String,
        nested: bool,
    ) -> LauncherInputAction {
        match action {
            LauncherAction::Exit => LauncherInputAction::Exit,
            LauncherAction::OpenCommands => {
                if self.current().view != command_view {
                    LauncherInputAction::OpenCommandView
                } else {
                    LauncherInputAction::Continue
                }
            }
            LauncherAction::Back => {
                if nested {
                    LauncherInputAction::Back
                } else if !input.is_empty() {
                    input.clear();
                    LauncherInputAction::Refresh
                } else {
                    LauncherInputAction::Back
                }
            }
            LauncherAction::SelectPrevious => {
                if !self.current().items.is_empty() {
                    self.current_mut().selected = self.current().selected.saturating_sub(1);
                }
                LauncherInputAction::ClearError
            }
            LauncherAction::SelectNext => {
                if !self.current().items.is_empty() {
                    let last = self.current().items.len() - 1;
                    self.current_mut().selected = (self.current().selected + 1).min(last);
                }
                LauncherInputAction::ClearError
            }
            LauncherAction::DeleteBackward => {
                if input.pop().is_some() {
                    LauncherInputAction::Refresh
                } else {
                    LauncherInputAction::Continue
                }
            }
            LauncherAction::ClearInput => {
                if input.is_empty() {
                    LauncherInputAction::Continue
                } else {
                    input.clear();
                    LauncherInputAction::Refresh
                }
            }
            LauncherAction::DeleteWord => {
                let previous_length = input.len();
                while input.chars().last().is_some_and(char::is_whitespace) {
                    input.pop();
                }
                while !input.chars().last().is_some_and(char::is_whitespace) && !input.is_empty() {
                    input.pop();
                }
                if input.len() != previous_length {
                    LauncherInputAction::Refresh
                } else {
                    LauncherInputAction::Continue
                }
            }
            LauncherAction::Activate => LauncherInputAction::Activate(Key::Enter),
        }
    }
}
