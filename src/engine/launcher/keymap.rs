use crate::expression::Template;
use crate::input::Key;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum LauncherAction {
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

impl LauncherAction {
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
pub(super) struct LauncherKeymap {
    bindings: HashMap<Key, LauncherAction>,
}

impl LauncherKeymap {
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
            bail!("launcher bindings must be an object or complete expression");
        }
        let bindings = value
            .as_object()
            .context("launcher bindings must be an object")?;
        let mut dynamic = false;
        for (name, values) in bindings {
            LauncherAction::parse(name)
                .with_context(|| format!("unsupported launcher binding action {:?}", name))?;
            let values = values
                .as_array()
                .with_context(|| format!("launcher binding {:?} must be an array", name))?;
            for value in values {
                let source = value.as_str().with_context(|| {
                    format!("launcher binding {:?} entries must be strings", name)
                })?;
                if source.contains("{{") {
                    Template::parse(source)?;
                    dynamic = true;
                } else {
                    Key::parse_binding(source)
                        .with_context(|| format!("launcher binding action {:?}", name))?;
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
            .context("launcher bindings must evaluate to an object")?;
        let mut actions = HashSet::new();
        for name in overrides.keys() {
            let action = LauncherAction::parse(name)
                .with_context(|| format!("unsupported launcher binding action {:?}", name))?;
            actions.insert(action);
        }
        self.bindings.retain(|_, action| !actions.contains(action));

        for (name, values) in overrides {
            let action =
                LauncherAction::parse(name).expect("launcher binding action was validated above");
            let values = values
                .as_array()
                .with_context(|| format!("launcher binding {:?} must be an array", name))?;
            for value in values {
                let source = value.as_str().with_context(|| {
                    format!("launcher binding {:?} entries must be strings", name)
                })?;
                let key = Key::parse_binding(source)
                    .with_context(|| format!("launcher binding action {:?}", name))?;
                if let Some(existing) = self.bindings.insert(key, action) {
                    bail!(
                        "launcher key {:?} is assigned to both {:?} and {:?}",
                        source,
                        existing.name(),
                        action.name()
                    );
                }
            }
        }
        Ok(())
    }

    pub(super) fn action(&self, key: Key) -> Option<LauncherAction> {
        self.bindings.get(&key).copied()
    }
}

fn default_bindings() -> HashMap<Key, LauncherAction> {
    [
        (Key::Ctrl('c'), LauncherAction::Exit),
        (Key::Ctrl('d'), LauncherAction::Exit),
        (Key::Ctrl('k'), LauncherAction::OpenCommands),
        (Key::Escape, LauncherAction::Back),
        (Key::Up, LauncherAction::SelectPrevious),
        (Key::Down, LauncherAction::SelectNext),
        (Key::Backspace, LauncherAction::DeleteBackward),
        (Key::Ctrl('u'), LauncherAction::ClearInput),
        (Key::Ctrl('w'), LauncherAction::DeleteWord),
        (Key::Enter, LauncherAction::Activate),
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
        let keymap = LauncherKeymap::from_value(Some(json!({
            "open_commands": ["ctrl+p"]
        })))
        .unwrap();
        assert_eq!(
            keymap.action(Key::Ctrl('p')),
            Some(LauncherAction::OpenCommands)
        );
        assert_eq!(keymap.action(Key::Ctrl('k')), None);
        assert_eq!(keymap.action(Key::Ctrl('c')), Some(LauncherAction::Exit));
    }

    #[test]
    fn view_overrides_are_applied_after_root_defaults() {
        let keymap = LauncherKeymap::from_values(
            Some(json!({"open_commands": ["ctrl+p"]})),
            Some(json!({"open_commands": ["ctrl+o"]})),
        )
        .unwrap();
        assert_eq!(
            keymap.action(Key::Ctrl('o')),
            Some(LauncherAction::OpenCommands)
        );
        assert_eq!(keymap.action(Key::Ctrl('p')), None);
        assert_eq!(keymap.action(Key::Ctrl('k')), None);
    }

    #[test]
    fn empty_override_disables_an_action() {
        let keymap = LauncherKeymap::from_value(Some(json!({"exit": []}))).unwrap();
        assert_eq!(keymap.action(Key::Ctrl('c')), None);
        assert_eq!(keymap.action(Key::Ctrl('d')), None);
    }

    #[test]
    fn conflicting_bindings_are_rejected() {
        let error = LauncherKeymap::from_value(Some(json!({
            "exit": ["ctrl+k"]
        })))
        .expect_err("default command binding should conflict");
        assert!(error.to_string().contains("both"));
    }

    #[test]
    fn ctrl_aliases_use_the_terminal_key_identity() {
        let keymap = LauncherKeymap::from_value(Some(json!({
            "activate": [],
            "exit": ["ctrl+j"]
        })))
        .unwrap();
        assert_eq!(keymap.action(Key::Enter), Some(LauncherAction::Exit));
        assert_eq!(keymap.action(Key::Ctrl('j')), None);
    }

    #[test]
    fn validation_accepts_binding_expressions() {
        LauncherKeymap::validate_value(Some(&json!("{{ config:keymaps.launcher }}"))).unwrap();
        LauncherKeymap::validate_value(Some(&json!({
            "exit": ["{{ config:keymaps.exit }}"]
        })))
        .unwrap();
    }
}
