use super::LauncherDriver;
use crate::engine::{EngineKeyAction, Key};

impl LauncherDriver {
    pub(crate) fn handle_input(&mut self, key: Key, command_view: &str) -> EngineKeyAction {
        match key {
            Key::CtrlC | Key::CtrlD => EngineKeyAction::Exit,
            Key::CtrlK => {
                if self.current().view != command_view {
                    EngineKeyAction::OpenCommandView
                } else {
                    EngineKeyAction::Continue
                }
            }
            Key::Escape => {
                if !self.current().input.is_empty() {
                    self.current_mut().input.clear();
                    EngineKeyAction::Refresh
                } else {
                    EngineKeyAction::PopView
                }
            }
            Key::Enter | Key::Alt(_) => {
                if self.current().command_owner.is_some() && !matches!(key, Key::Enter) {
                    EngineKeyAction::Continue
                } else {
                    EngineKeyAction::Command(key)
                }
            }
            Key::Up => {
                if !self.current().items.is_empty() {
                    self.current_mut().selected = self.current().selected.saturating_sub(1);
                }
                EngineKeyAction::ClearError
            }
            Key::Down => {
                if !self.current().items.is_empty() {
                    let last = self.current().items.len() - 1;
                    self.current_mut().selected = (self.current().selected + 1).min(last);
                }
                EngineKeyAction::ClearError
            }
            Key::Backspace => {
                if self.current_mut().input.pop().is_some() {
                    EngineKeyAction::Refresh
                } else {
                    EngineKeyAction::Continue
                }
            }
            Key::CtrlU => {
                if self.current().input.is_empty() {
                    EngineKeyAction::Continue
                } else {
                    self.current_mut().input.clear();
                    EngineKeyAction::Refresh
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
                    EngineKeyAction::Refresh
                } else {
                    EngineKeyAction::Continue
                }
            }
            Key::Char(character) if !character.is_control() => {
                self.current_mut().input.push(character);
                EngineKeyAction::Refresh
            }
            Key::Char(_) => EngineKeyAction::Continue,
        }
    }
}
