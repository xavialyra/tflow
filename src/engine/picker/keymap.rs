use crate::engine::keymap::{ActionBindings, KeymapAction};
use crate::input::Key;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum PickerAction {
    Exit,
    Back,
    SelectPrevious,
    SelectNext,
    DeleteBackward,
    ClearInput,
    DeleteWord,
    Activate,
    TogglePreview,
}

impl PickerAction {
    const ALL: [Self; 9] = [
        Self::Exit,
        Self::Back,
        Self::SelectPrevious,
        Self::SelectNext,
        Self::DeleteBackward,
        Self::ClearInput,
        Self::DeleteWord,
        Self::Activate,
        Self::TogglePreview,
    ];
}

impl KeymapAction for PickerAction {
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
            Self::Activate => "activate",
            Self::TogglePreview => "toggle_preview",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.name() == name)
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
            (Key::Enter, Self::Activate),
        ]
    }
}

pub(super) type PickerKeymap = ActionBindings<PickerAction>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn partial_overrides_replace_one_action_and_keep_other_defaults() {
        let keymap = PickerKeymap::from_value(Some(json!({
            "select_next": ["ctrl+n"]
        })))
        .unwrap();
        assert_eq!(
            keymap.action(Key::Ctrl('n')),
            Some(PickerAction::SelectNext)
        );
        assert_eq!(keymap.action(Key::Down), None);
        assert_eq!(keymap.action(Key::Ctrl('c')), Some(PickerAction::Exit));
    }

    #[test]
    fn view_keymap_patch_is_applied_after_root_defaults() {
        let keymap = PickerKeymap::from_values(
            Some(json!({"select_next": ["ctrl+n"]})),
            Some(json!({
                "ctrl+n": false,
                "ctrl+o": "select_next"
            })),
        )
        .unwrap();
        assert_eq!(
            keymap.action(Key::Ctrl('o')),
            Some(PickerAction::SelectNext)
        );
        assert_eq!(keymap.action(Key::Ctrl('n')), None);
    }

    #[test]
    fn keymap_patch_disables_an_inherited_action_with_a_tombstone() {
        let keymap = PickerKeymap::from_values(None, Some(json!({"enter": false}))).unwrap();
        assert_eq!(keymap.action(Key::Enter), None);
    }

    #[test]
    fn keymap_patch_replaces_an_inherited_action_by_key() {
        let keymap =
            PickerKeymap::from_values(None, Some(json!({"enter": "select_next"}))).unwrap();
        assert_eq!(keymap.action(Key::Enter), Some(PickerAction::SelectNext));
    }

    #[test]
    fn keymap_matches_uppercase_printable_input() {
        let keymap = PickerKeymap::from_values(None, Some(json!({"a": "select_next"}))).unwrap();
        assert_eq!(
            keymap.action(Key::Char('A')),
            Some(PickerAction::SelectNext)
        );
    }

    #[test]
    fn toggle_preview_binding_is_recognized() {
        let keymap = PickerKeymap::from_value(Some(json!({"toggle_preview": ["ctrl+p"]}))).unwrap();
        assert_eq!(
            keymap.action(Key::Ctrl('p')),
            Some(PickerAction::TogglePreview)
        );
    }

    #[test]
    fn empty_override_disables_an_action() {
        let keymap = PickerKeymap::from_value(Some(json!({"exit": []}))).unwrap();
        assert_eq!(keymap.action(Key::Ctrl('c')), None);
        assert_eq!(keymap.action(Key::Ctrl('d')), None);
    }

    #[test]
    fn conflicting_bindings_are_rejected() {
        let error = PickerKeymap::from_value(Some(json!({
            "exit": ["ctrl+j"]
        })))
        .expect_err("default activate binding should conflict");
        assert!(error.to_string().contains("both"));
    }

    #[test]
    fn ctrl_aliases_use_the_terminal_key_identity() {
        let keymap = PickerKeymap::from_value(Some(json!({
            "activate": [],
            "exit": ["ctrl+j"]
        })))
        .unwrap();
        assert_eq!(keymap.action(Key::Enter), Some(PickerAction::Exit));
        assert_eq!(keymap.action(Key::Ctrl('j')), None);
    }

    #[test]
    fn validation_accepts_dynamic_binding_paths() {
        PickerKeymap::validate_value(Some(&json!("{{ view.input }}"))).unwrap();
        PickerKeymap::validate_value(Some(&json!({
            "exit": ["{{ view.input }}"]
        })))
        .unwrap();
        PickerKeymap::validate_keymap_value(Some(&json!({
            "escape": false,
            "ctrl+y": "{{ view.input }}"
        })))
        .unwrap();
    }
}
