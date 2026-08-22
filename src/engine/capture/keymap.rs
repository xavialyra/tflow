use crate::engine::keymap::{ActionBindings, KeymapAction};
use crate::input::Key;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum CaptureAction {
    Copy,
    Back,
}

impl CaptureAction {
    const ALL: [Self; 2] = [Self::Copy, Self::Back];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Copy => "Copy",
            Self::Back => "Back",
        }
    }
}

impl KeymapAction for CaptureAction {
    const LABEL: &'static str = "capture";

    fn name(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Back => "back",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.name() == name)
    }

    fn default_bindings() -> &'static [(Key, Self)] {
        &[(Key::Enter, Self::Copy), (Key::Escape, Self::Back)]
    }
}

pub(super) type CaptureKeymap = ActionBindings<CaptureAction>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_copy_on_enter_and_go_back_on_escape() {
        let keymap = CaptureKeymap::from_value(None).unwrap();
        assert_eq!(keymap.action(Key::Enter), Some(CaptureAction::Copy));
        assert_eq!(keymap.action(Key::Escape), Some(CaptureAction::Back));
        assert_eq!(keymap.action(Key::Char('x')), None);
    }

    #[test]
    fn view_keymap_patch_is_applied_after_root_defaults() {
        let keymap = CaptureKeymap::from_values(
            Some(json!({"copy": ["ctrl+y"]})),
            Some(json!({
                "ctrl+y": false,
                "alt+c": "copy"
            })),
        )
        .unwrap();
        assert_eq!(keymap.action(Key::Alt('c')), Some(CaptureAction::Copy));
        assert_eq!(keymap.action(Key::Ctrl('y')), None);
        assert_eq!(keymap.action(Key::Escape), Some(CaptureAction::Back));
    }

    #[test]
    fn keymap_patch_disables_an_inherited_action_with_a_tombstone() {
        let keymap = CaptureKeymap::from_values(None, Some(json!({"enter": false}))).unwrap();
        assert_eq!(keymap.action(Key::Enter), None);
        assert_eq!(keymap.action(Key::Escape), Some(CaptureAction::Back));
    }

    #[test]
    fn keymap_matches_uppercase_printable_input() {
        let keymap = CaptureKeymap::from_values(None, Some(json!({"a": "copy"}))).unwrap();
        assert_eq!(keymap.action(Key::Char('A')), Some(CaptureAction::Copy));
    }

    #[test]
    fn conflicting_bindings_are_rejected() {
        let error = CaptureKeymap::from_value(Some(json!({"copy": ["escape"]})))
            .expect_err("default back binding should conflict");
        assert!(error.to_string().contains("both"));
    }

    #[test]
    fn validation_accepts_dynamic_binding_paths() {
        CaptureKeymap::validate_value(Some(&json!("{{ view.input }}"))).unwrap();
        CaptureKeymap::validate_value(Some(&json!({
            "copy": ["{{ view.input }}"]
        })))
        .unwrap();
        CaptureKeymap::validate_keymap_value(Some(&json!({
            "escape": false,
            "ctrl+y": "{{ view.input }}"
        })))
        .unwrap();
    }
}
