use crate::expression::Template;
use crate::input::Key;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum PickerAction {
    Exit,
    OpenCommands,
    Back,
    SelectPrevious,
    SelectNext,
    DeleteBackward,
    ClearInput,
    DeleteWord,
    Activate,
}

impl PickerAction {
    const ALL: [Self; 9] = [
        Self::Exit,
        Self::OpenCommands,
        Self::Back,
        Self::SelectPrevious,
        Self::SelectNext,
        Self::DeleteBackward,
        Self::ClearInput,
        Self::DeleteWord,
        Self::Activate,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Exit => "exit",
            Self::OpenCommands => "open_commands",
            Self::Back => "back",
            Self::SelectPrevious => "select_previous",
            Self::SelectNext => "select_next",
            Self::DeleteBackward => "delete_backward",
            Self::ClearInput => "clear_input",
            Self::DeleteWord => "delete_word",
            Self::Activate => "activate",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.name() == name)
    }
}

#[derive(Debug, Clone)]
pub(super) struct PickerKeymap {
    bindings: HashMap<Key, PickerAction>,
}

impl PickerKeymap {
    #[cfg(test)]
    pub(super) fn validate_value(value: Option<&Value>) -> Result<()> {
        Self::validate_values(None, value)
    }

    pub(super) fn validate_values(defaults: Option<&Value>, view: Option<&Value>) -> Result<()> {
        let dynamic = Self::validate_shape(defaults)? | Self::validate_shape(view)?;
        if !dynamic {
            Self::from_values(defaults.cloned(), view.cloned())?;
        }
        Ok(())
    }

    fn validate_shape(value: Option<&Value>) -> Result<bool> {
        let Some(value) = value else {
            return Ok(false);
        };
        if let Some(source) = value.as_str() {
            if Template::parse(source)?.is_complete_expression() {
                return Ok(true);
            }
            bail!("picker bindings must be an object or complete expression");
        }
        let bindings = value
            .as_object()
            .context("picker bindings must be an object")?;
        let mut dynamic = false;
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
                if source.contains("{{") {
                    Template::parse(source)?;
                    dynamic = true;
                } else {
                    Key::parse_binding(source)
                        .with_context(|| format!("picker binding action {:?}", name))?;
                }
            }
        }
        Ok(dynamic)
    }

    #[cfg(test)]
    pub(super) fn from_value(value: Option<Value>) -> Result<Self> {
        Self::from_values(None, value)
    }

    pub(super) fn from_values(defaults: Option<Value>, view: Option<Value>) -> Result<Self> {
        let mut keymap = Self {
            bindings: default_bindings(),
        };
        if let Some(defaults) = defaults {
            keymap.apply(defaults)?;
        }
        if let Some(view) = view {
            keymap.apply(view)?;
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
                    .with_context(|| format!("picker binding action {:?}", name))?;
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

    pub(super) fn action(&self, key: Key) -> Option<PickerAction> {
        self.bindings.get(&key).copied()
    }
}

fn default_bindings() -> HashMap<Key, PickerAction> {
    [
        (Key::Ctrl('c'), PickerAction::Exit),
        (Key::Ctrl('d'), PickerAction::Exit),
        (Key::Ctrl('k'), PickerAction::OpenCommands),
        (Key::Escape, PickerAction::Back),
        (Key::Up, PickerAction::SelectPrevious),
        (Key::Down, PickerAction::SelectNext),
        (Key::Backspace, PickerAction::DeleteBackward),
        (Key::Ctrl('u'), PickerAction::ClearInput),
        (Key::Ctrl('w'), PickerAction::DeleteWord),
        (Key::Enter, PickerAction::Activate),
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn partial_overrides_replace_one_action_and_keep_other_defaults() {
        let keymap = PickerKeymap::from_value(Some(json!({
            "open_commands": ["ctrl+p"]
        })))
        .unwrap();
        assert_eq!(
            keymap.action(Key::Ctrl('p')),
            Some(PickerAction::OpenCommands)
        );
        assert_eq!(keymap.action(Key::Ctrl('k')), None);
        assert_eq!(keymap.action(Key::Ctrl('c')), Some(PickerAction::Exit));
    }

    #[test]
    fn view_overrides_are_applied_after_root_defaults() {
        let keymap = PickerKeymap::from_values(
            Some(json!({"open_commands": ["ctrl+p"]})),
            Some(json!({"open_commands": ["ctrl+o"]})),
        )
        .unwrap();
        assert_eq!(
            keymap.action(Key::Ctrl('o')),
            Some(PickerAction::OpenCommands)
        );
        assert_eq!(keymap.action(Key::Ctrl('p')), None);
        assert_eq!(keymap.action(Key::Ctrl('k')), None);
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
            "exit": ["ctrl+k"]
        })))
        .expect_err("default command binding should conflict");
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
    fn validation_accepts_binding_expressions() {
        PickerKeymap::validate_value(Some(&json!("{{ config:keymaps.picker }}"))).unwrap();
        PickerKeymap::validate_value(Some(&json!({
            "exit": ["{{ config:keymaps.exit }}"]
        })))
        .unwrap();
    }
}
