use super::LauncherView;
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
    pub(super) fn handle_input(&mut self, key: Key, command_view: &str) -> LauncherInputAction {
        match key {
            Key::CtrlC | Key::CtrlD => LauncherInputAction::Exit,
            Key::CtrlK => {
                if self.current().view != command_view {
                    LauncherInputAction::OpenCommandView
                } else {
                    LauncherInputAction::Continue
                }
            }
            Key::Escape => {
                if !self.current().input.is_empty() {
                    self.current_mut().input.clear();
                    LauncherInputAction::Refresh
                } else {
                    LauncherInputAction::Back
                }
            }
            Key::Enter | Key::Alt(_) => {
                if self.current().command_owner.is_some() && !matches!(key, Key::Enter) {
                    LauncherInputAction::Continue
                } else {
                    LauncherInputAction::Activate(key)
                }
            }
            Key::Up => {
                if !self.current().items.is_empty() {
                    self.current_mut().selected = self.current().selected.saturating_sub(1);
                }
                LauncherInputAction::ClearError
            }
            Key::Down => {
                if !self.current().items.is_empty() {
                    let last = self.current().items.len() - 1;
                    self.current_mut().selected = (self.current().selected + 1).min(last);
                }
                LauncherInputAction::ClearError
            }
            Key::Backspace => {
                if self.current_mut().input.pop().is_some() {
                    LauncherInputAction::Refresh
                } else {
                    LauncherInputAction::Continue
                }
            }
            Key::CtrlU => {
                if self.current().input.is_empty() {
                    LauncherInputAction::Continue
                } else {
                    self.current_mut().input.clear();
                    LauncherInputAction::Refresh
                }
            }
            Key::CtrlW => {
                let input = &mut self.current_mut().input;
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
            Key::Char(character) if !character.is_control() => {
                self.current_mut().input.push(character);
                LauncherInputAction::Refresh
            }
            Key::Char(_) => LauncherInputAction::Continue,
        }
    }
}
