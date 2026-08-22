use crate::engine::keymap::{apply_patch, static_bindings, static_patch, validate_patch};
use crate::expression::{Template, is_dynamic_string};
use crate::input::{BindingKey, Key};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum CaptureAction {
    Copy,
    Back,
}

impl CaptureAction {
    const ALL: [Self; 2] = [Self::Copy, Self::Back];

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Back => "back",
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Copy => "Copy",
            Self::Back => "Back",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.name() == name)
    }
}

#[derive(Debug, Clone)]
pub(super) struct CaptureKeymap {
    bindings: HashMap<BindingKey, CaptureAction>,
}

impl CaptureKeymap {
    #[cfg(test)]
    pub(super) fn validate_value(value: Option<&Value>) -> Result<()> {
        Self::validate_shape(value)?;
        Self::from_values(static_bindings(value), None)?;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn validate_keymap_value(value: Option<&Value>) -> Result<()> {
        validate_patch(value, "capture", CaptureAction::parse)
    }

    pub(super) fn validate_values(defaults: Option<&Value>, view: Option<&Value>) -> Result<()> {
        Self::validate_shape(defaults)?;
        validate_patch(view, "capture", CaptureAction::parse)?;
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
            bail!("capture bindings must be an object or complete dynamic path");
        }
        let bindings = value
            .as_object()
            .context("capture bindings must be an object")?;
        for (name, values) in bindings {
            CaptureAction::parse(name)
                .with_context(|| format!("unsupported capture binding action {:?}", name))?;
            let values = values
                .as_array()
                .with_context(|| format!("capture binding {:?} must be an array", name))?;
            for value in values {
                let source = value.as_str().with_context(|| {
                    format!("capture binding {:?} entries must be strings", name)
                })?;
                if is_dynamic_string(source) {
                    Template::parse(source)?;
                } else {
                    Key::parse_binding(source)
                        .with_context(|| format!("capture binding action {:?}", name))?;
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
            .context("capture bindings must evaluate to an object")?;
        let mut actions = HashSet::new();
        for name in overrides.keys() {
            let action = CaptureAction::parse(name)
                .with_context(|| format!("unsupported capture binding action {:?}", name))?;
            actions.insert(action);
        }
        self.bindings.retain(|_, action| !actions.contains(action));

        for (name, values) in overrides {
            let action =
                CaptureAction::parse(name).expect("capture binding action was validated above");
            let values = values
                .as_array()
                .with_context(|| format!("capture binding {:?} must be an array", name))?;
            for value in values {
                let source = value.as_str().with_context(|| {
                    format!("capture binding {:?} entries must be strings", name)
                })?;
                let key = Key::parse_binding(source)
                    .with_context(|| format!("capture binding action {:?}", name))?
                    .binding_identity();
                if let Some(existing) = self.bindings.insert(key, action) {
                    bail!(
                        "capture key {:?} is assigned to both {:?} and {:?}",
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
        apply_patch(&mut self.bindings, value, "capture", CaptureAction::parse)
    }

    pub(super) fn action(&self, key: Key) -> Option<CaptureAction> {
        self.bindings.get(&key.binding_identity()).copied()
    }

    pub(super) fn bindings(&self) -> impl Iterator<Item = (Key, CaptureAction)> + '_ {
        self.bindings
            .iter()
            .map(|(key, action)| (key.key(), *action))
    }
}

fn default_bindings() -> HashMap<BindingKey, CaptureAction> {
    [
        (Key::Enter, CaptureAction::Copy),
        (Key::Escape, CaptureAction::Back),
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
