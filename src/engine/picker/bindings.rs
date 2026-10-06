use crate::input::Key;
use crate::input::bindings::{ActionBindings, BindingAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum PickerAction {
    Exit,
    Back,
    SelectPrevious,
    SelectNext,
    DeleteBackward,
    ClearInput,
    DeleteWord,
}

impl PickerAction {
    const ALL: [Self; 7] = [
        Self::Exit,
        Self::Back,
        Self::SelectPrevious,
        Self::SelectNext,
        Self::DeleteBackward,
        Self::ClearInput,
        Self::DeleteWord,
    ];
}

impl BindingAction for PickerAction {
    const LABEL: &'static str = "picker";

    fn name(self) -> &'static str {
        match self {
            Self::Exit => "exit",
            Self::Back => "back",
            Self::SelectPrevious => "select_previous",
            Self::SelectNext => "select_next",
            Self::DeleteBackward => "delete_backward",
            Self::ClearInput => "clear_input",
            Self::DeleteWord => "delete_word",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.name() == name)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Exit => "Exit",
            Self::Back => "Back",
            Self::SelectPrevious => "Select Previous",
            Self::SelectNext => "Select Next",
            Self::DeleteBackward => "Delete",
            Self::ClearInput => "Clear Input",
            Self::DeleteWord => "Delete Word",
        }
    }

    fn default_bindings() -> &'static [(Key, Self)] {
        &[
            (Key::Ctrl('c'), Self::Exit),
            (Key::Ctrl('d'), Self::Exit),
            (Key::Escape, Self::Back),
            (Key::Up, Self::SelectPrevious),
            (Key::Down, Self::SelectNext),
            (Key::Backspace, Self::DeleteBackward),
            (Key::Ctrl('u'), Self::ClearInput),
            (Key::Ctrl('w'), Self::DeleteWord),
        ]
    }
}

pub(super) type PickerBindings = ActionBindings<PickerAction>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_back_on_escape_and_clear_input_on_ctrl_u() {
        let bindings = PickerBindings::from_defaults(None).unwrap();
        assert_eq!(bindings.action(Key::Escape), Some(PickerAction::Back));
        assert_eq!(
            bindings.action(Key::Ctrl('u')),
            Some(PickerAction::ClearInput)
        );
        assert_eq!(bindings.action(Key::Ctrl('p')), None);
    }

    /// One action may be reached by several keys. Both must survive entry
    /// construction, or a shortcut silently disappears from the footer hints and
    /// the command palette.
    #[test]
    fn both_exit_keys_drive_the_same_action_in_a_stable_order() {
        let bindings = PickerBindings::from_defaults(None).unwrap();
        let exits: Vec<String> = bindings
            .bindings()
            .filter(|(_, action)| *action == PickerAction::Exit)
            .map(|(key, _)| key.binding_name().unwrap())
            .collect();
        assert_eq!(exits, ["ctrl+c", "ctrl+d"]);
    }

    #[test]
    fn key_centric_defaults_add_bindings_and_keep_builtin_keys() {
        let bindings = PickerBindings::from_defaults(Some(json!({
            "ctrl+n": "select_next"
        })))
        .unwrap();
        assert_eq!(
            bindings.action(Key::Ctrl('n')),
            Some(PickerAction::SelectNext)
        );
        assert_eq!(bindings.action(Key::Down), Some(PickerAction::SelectNext));
        assert_eq!(bindings.action(Key::Ctrl('c')), Some(PickerAction::Exit));
    }

    #[test]
    fn bindings_matches_uppercase_printable_input() {
        let bindings = PickerBindings::from_defaults(Some(json!({"a": "select_next"}))).unwrap();
        assert_eq!(
            bindings.action(Key::Char('A')),
            Some(PickerAction::SelectNext)
        );
    }

    #[test]
    fn empty_override_disables_an_action() {
        let bindings = PickerBindings::from_defaults(Some(json!({
            "ctrl+c": false,
            "ctrl+d": false
        })))
        .unwrap();
        assert_eq!(bindings.action(Key::Ctrl('c')), None);
        assert_eq!(bindings.action(Key::Ctrl('d')), None);
    }

    #[test]
    fn conflicting_bindings_are_rejected() {
        let error = PickerBindings::from_defaults(Some(json!({
            "escape": false,
            "esc": "back"
        })))
        .expect_err("one key cannot be both disabled and rebound");
        assert!(error.to_string().contains("both disabled and rebound"));
    }

    #[test]
    fn enter_is_not_an_implicit_picker_action() {
        let bindings = PickerBindings::from_defaults(Some(json!({
            "ctrl+o": "exit"
        })))
        .unwrap();
        assert_eq!(bindings.action(Key::Ctrl('o')), Some(PickerAction::Exit));
        assert_eq!(bindings.action(Key::Enter), None);
    }

    #[test]
    fn removed_activate_action_is_rejected() {
        let value = json!({
            "enter": "activate"
        });
        let error = PickerBindings::validate_defaults(Some(&value)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported picker binding action")
        );
    }
}
