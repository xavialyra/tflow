use crate::input::Key;
use crate::input::bindings::{ActionBindings, BindingAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum CaptureAction {
    Copy,
    Back,
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
}

impl CaptureAction {
    const ALL: [Self; 6] = [
        Self::Copy,
        Self::Back,
        Self::ScrollUp,
        Self::ScrollDown,
        Self::PageUp,
        Self::PageDown,
    ];
}

impl BindingAction for CaptureAction {
    const LABEL: &'static str = "capture";

    fn name(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Back => "back",
            Self::ScrollUp => "scroll_up",
            Self::ScrollDown => "scroll_down",
            Self::PageUp => "page_up",
            Self::PageDown => "page_down",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.name() == name)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Copy => "Copy",
            Self::Back => "Back",
            Self::ScrollUp => "Scroll Up",
            Self::ScrollDown => "Scroll Down",
            Self::PageUp => "Page Up",
            Self::PageDown => "Page Down",
        }
    }

    fn default_bindings() -> &'static [(Key, Self)] {
        &[
            (Key::Enter, Self::Copy),
            (Key::Escape, Self::Back),
            (Key::Up, Self::ScrollUp),
            (Key::Down, Self::ScrollDown),
            (Key::Char('k'), Self::ScrollUp),
            (Key::Char('j'), Self::ScrollDown),
            (Key::Ctrl('u'), Self::PageUp),
            (Key::Ctrl('d'), Self::PageDown),
        ]
    }
}

pub(super) type CaptureBindings = ActionBindings<CaptureAction>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_copy_on_enter_and_go_back_on_escape() {
        let bindings = CaptureBindings::from_defaults(None).unwrap();
        assert_eq!(bindings.action(Key::Enter), Some(CaptureAction::Copy));
        assert_eq!(bindings.action(Key::Escape), Some(CaptureAction::Back));
        assert_eq!(bindings.action(Key::Up), Some(CaptureAction::ScrollUp));
        assert_eq!(bindings.action(Key::Down), Some(CaptureAction::ScrollDown));
        assert_eq!(bindings.action(Key::Ctrl('u')), Some(CaptureAction::PageUp));
        assert_eq!(
            bindings.action(Key::Ctrl('d')),
            Some(CaptureAction::PageDown)
        );
        assert_eq!(
            bindings.action(Key::Char('k')),
            Some(CaptureAction::ScrollUp)
        );
        assert_eq!(
            bindings.action(Key::Char('j')),
            Some(CaptureAction::ScrollDown)
        );
        assert_eq!(bindings.action(Key::Char('x')), None);
    }

    #[test]
    fn bindings_matches_uppercase_printable_input() {
        let bindings = CaptureBindings::from_defaults(Some(json!({"a": "copy"}))).unwrap();
        assert_eq!(bindings.action(Key::Char('A')), Some(CaptureAction::Copy));
    }

    #[test]
    fn conflicting_bindings_are_rejected() {
        let error = CaptureBindings::from_defaults(Some(json!({
            "escape": false,
            "esc": "copy"
        })))
        .expect_err("one key cannot be both disabled and rebound");
        assert!(error.to_string().contains("both disabled and rebound"));
    }
}
