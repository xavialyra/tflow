use crate::engine::keymap::{apply_patch, static_bindings, static_patch, validate_patch};
use crate::expression::{Template, is_dynamic_string};
use crate::input::{BindingKey, Key};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

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
}

#[derive(Debug, Clone)]
pub(super) struct PickerKeymap {
    bindings: HashMap<BindingKey, PickerAction>,
}

impl PickerKeymap {
    #[cfg(test)]
    pub(super) fn validate_value(value: Option<&Value>) -> Result<()> {
        Self::validate_shape(value)?;
        Self::from_values(static_bindings(value), None)?;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn validate_keymap_value(value: Option<&Value>) -> Result<()> {
        validate_patch(value, "picker", PickerAction::parse)
    }

    pub(super) fn validate_values(defaults: Option<&Value>, view: Option<&Value>) -> Result<()> {
        Self::validate_shape(defaults)?;
        validate_patch(view, "picker", PickerAction::parse)?;
        Self::from_values(static_bindings(defaults), static_patch(view))?;
        Ok(())
    }

    fn validate_shape(value: Option<&Value>) -> Result<()> {
        let Some(value) = value else {
            return Ok(());
        };
        if let Some(source) = value.as_str() {
            if Template::parse(source)?.is_complete_path() {
                return Ok(());
            }
            bail!("picker bindings must be an object or complete dynamic path");
        }
        let bindings = value
            .as_object()
            .context("picker bindings must be an object")?;
        for (name, values) in bindings {
            PickerAction::parse(name)
                .with_context(|| format!("unsupported picker binding action {:?}", name))?;
            let values = values
                .as_array()
                .with_context(|| format!("picker binding {:?} must be an array", name))?;
            for value in values {
                let source = value.as_str().with_context(|| {
                    format!("picker binding {:?} entries must be strings", name)
                })?;
                if is_dynamic_string(source) {
                    Template::parse(source)?;
                } else {
                    Key::parse_binding(source)
                        .with_context(|| format!("picker binding action {:?}", name))?;
                }
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn from_value(value: Option<Value>) -> Result<Self> {
        Self::from_values(value, None)
    }

    pub(super) fn from_values(defaults: Option<Value>, view: Option<Value>) -> Result<Self> {
        let mut keymap = Self {
            bindings: default_bindings(),
        };
        if let Some(defaults) = defaults {
            keymap.apply(defaults)?;
        }
        if let Some(view) = view {
            keymap.apply_patch(view)?;
        }
        Ok(keymap)
    }

    fn apply(&mut self, value: Value) -> Result<()> {
        let overrides = value
            .as_object()
            .context("picker bindings must evaluate to an object")?;
        let mut actions = HashSet::new();
        for name in overrides.keys() {
            let action = PickerAction::parse(name)
                .with_context(|| format!("unsupported picker binding action {:?}", name))?;
            actions.insert(action);
        }
        self.bindings.retain(|_, action| !actions.contains(action));

        for (name, values) in overrides {
            let action =
                PickerAction::parse(name).expect("picker binding action was validated above");
            let values = values
                .as_array()
                .with_context(|| format!("picker binding {:?} must be an array", name))?;
            for value in values {
                let source = value.as_str().with_context(|| {
                    format!("picker binding {:?} entries must be strings", name)
                })?;
                let key = Key::parse_binding(source)
                    .with_context(|| format!("picker binding action {:?}", name))?
                    .binding_identity();
                if let Some(existing) = self.bindings.insert(key, action) {
                    bail!(
                        "picker key {:?} is assigned to both {:?} and {:?}",
                        source,
                        existing.name(),
                        action.name()
                    );
                }
            }
        }
        Ok(())
    }

    fn apply_patch(&mut self, value: Value) -> Result<()> {
        apply_patch(&mut self.bindings, value, "picker", PickerAction::parse)
    }

    pub(super) fn action(&self, key: Key) -> Option<PickerAction> {
        self.bindings.get(&key.binding_identity()).copied()
    }
}

fn default_bindings() -> HashMap<BindingKey, PickerAction> {
    [
        (Key::Ctrl('c'), PickerAction::Exit),
        (Key::Ctrl('d'), PickerAction::Exit),
        (Key::Escape, PickerAction::Back),
        (Key::Up, PickerAction::SelectPrevious),
        (Key::Down, PickerAction::SelectNext),
        (Key::Backspace, PickerAction::DeleteBackward),
        (Key::Ctrl('u'), PickerAction::ClearInput),
        (Key::Ctrl('w'), PickerAction::DeleteWord),
        (Key::Enter, PickerAction::Activate),
    ]
    .into_iter()
    .map(|(key, action)| (key.binding_identity(), action))
    .collect()
}

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
