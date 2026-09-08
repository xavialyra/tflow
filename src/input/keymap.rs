use crate::input::{BindingKey, Key};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

pub(crate) fn validate_patch<A, F>(
    value: Option<&Value>,
    label: &str,
    parse_action: F,
) -> Result<()>
where
    A: Copy,
    F: Fn(&str) -> Option<A> + Copy,
{
    let Some(value) = value else {
        return Ok(());
    };
    let patch = value
        .as_object()
        .with_context(|| format!("{label} keymap must be an object"))?;
    for (source, value) in patch {
        Key::parse_binding(source)
            .with_context(|| format!("{label} keymap binding {:?}", source))?;
        if value.as_bool() == Some(false) {
            continue;
        }
        if value.is_boolean() {
            bail!(
                "{label} keymap binding {:?} must be false or an action",
                source
            );
        }
        let action = value.as_str().with_context(|| {
            format!(
                "{label} keymap binding {:?} must be false or an action",
                source
            )
        })?;
        parse_action(action)
            .with_context(|| format!("unsupported {label} keymap action {:?}", action))?;
    }
    Ok(())
}

pub(crate) fn static_bindings(value: Option<&Value>) -> Option<Value> {
    let Some(Value::Object(bindings)) = value else {
        return None;
    };
    let mut static_bindings = serde_json::Map::new();
    for (action, values) in bindings {
        let values = values.as_array()?;
        let values = values.iter().cloned().collect();
        static_bindings.insert(action.clone(), Value::Array(values));
    }
    Some(Value::Object(static_bindings))
}

pub(crate) fn static_patch(value: Option<&Value>) -> Option<Value> {
    let Some(Value::Object(patch)) = value else {
        return None;
    };
    let patch = patch
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Some(Value::Object(patch))
}

pub(crate) fn apply_patch<A, F>(
    bindings: &mut HashMap<BindingKey, A>,
    value: Value,
    label: &str,
    parse_action: F,
) -> Result<()>
where
    A: Copy + Eq + Hash,
    F: Fn(&str) -> Option<A> + Copy,
{
    let patch = value
        .as_object()
        .with_context(|| format!("{label} keymap must be an object"))?;
    let mut tombstones = HashSet::new();
    let mut rebound = HashSet::new();

    for (source, value) in patch {
        let key = Key::parse_binding(source)
            .with_context(|| format!("{label} keymap binding {:?}", source))?
            .binding_identity();
        if value.as_bool() == Some(false) {
            if rebound.contains(&key) {
                bail!(
                    "{label} keymap key {:?} cannot be both disabled and rebound",
                    source
                );
            }
            if !tombstones.insert(key) {
                bail!(
                    "{label} keymap contains duplicate physical key {:?}",
                    source
                );
            }
            continue;
        }
        if value.is_boolean() {
            bail!(
                "{label} keymap binding {:?} must be false or an action",
                source
            );
        }
        if tombstones.contains(&key) {
            bail!(
                "{label} keymap key {:?} cannot be both disabled and rebound",
                source
            );
        }
        if !rebound.insert(key) {
            bail!(
                "{label} keymap contains duplicate physical key {:?}",
                source
            );
        }
        let action_name = value.as_str().with_context(|| {
            format!(
                "{label} keymap binding {:?} must be false or an action",
                source
            )
        })?;
        let action = parse_action(action_name)
            .with_context(|| format!("unsupported {label} keymap action {:?}", action_name))?;
        bindings.insert(key, action);
    }

    for key in tombstones {
        bindings.remove(&key);
    }
    Ok(())
}

pub(crate) trait KeymapAction: Copy + Eq + Hash {
    const LABEL: &'static str;

    fn name(self) -> &'static str;
    fn parse(name: &str) -> Option<Self>;
    fn default_bindings() -> &'static [(Key, Self)];
}

#[derive(Debug, Clone)]
pub(crate) struct ActionBindings<A> {
    bindings: HashMap<BindingKey, A>,
}

impl<A: KeymapAction + 'static> ActionBindings<A> {
    #[cfg(test)]
    pub(crate) fn validate_value(value: Option<&Value>) -> Result<()> {
        Self::validate_bindings(value)?;
        Self::from_values(static_bindings(value), None)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn validate_keymap_value(value: Option<&Value>) -> Result<()> {
        validate_patch(value, A::LABEL, A::parse)
    }

    pub(crate) fn validate_values(defaults: Option<&Value>, view: Option<&Value>) -> Result<()> {
        Self::validate_bindings(defaults)?;
        validate_patch(view, A::LABEL, A::parse)?;
        Self::from_values(static_bindings(defaults), static_patch(view))?;
        Ok(())
    }

    fn validate_bindings(value: Option<&Value>) -> Result<()> {
        let Some(value) = value else {
            return Ok(());
        };
        let bindings = value
            .as_object()
            .with_context(|| format!("{} bindings must be an object", A::LABEL))?;
        for (name, values) in bindings {
            A::parse(name)
                .with_context(|| format!("unsupported {} binding action {:?}", A::LABEL, name))?;
            let values = values
                .as_array()
                .with_context(|| format!("{} binding {:?} must be an array", A::LABEL, name))?;
            for value in values {
                let source = value.as_str().with_context(|| {
                    format!("{} binding {:?} entries must be strings", A::LABEL, name)
                })?;
                Key::parse_binding(source)
                    .with_context(|| format!("{} binding action {:?}", A::LABEL, name))?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn from_value(value: Option<Value>) -> Result<Self> {
        Self::from_values(value, None)
    }

    pub(crate) fn from_values(defaults: Option<Value>, view: Option<Value>) -> Result<Self> {
        let mut keymap = Self {
            bindings: A::default_bindings()
                .iter()
                .map(|(key, action)| (key.binding_identity(), *action))
                .collect(),
        };
        if let Some(defaults) = defaults {
            keymap.apply_bindings(defaults)?;
        }
        if let Some(view) = view {
            apply_patch(&mut keymap.bindings, view, A::LABEL, A::parse)?;
        }
        Ok(keymap)
    }

    fn apply_bindings(&mut self, value: Value) -> Result<()> {
        let overrides = value
            .as_object()
            .with_context(|| format!("{} bindings must be an object", A::LABEL))?;
        let mut actions = HashSet::new();
        for name in overrides.keys() {
            let action = A::parse(name)
                .with_context(|| format!("unsupported {} binding action {:?}", A::LABEL, name))?;
            actions.insert(action);
        }
        self.bindings.retain(|_, action| !actions.contains(action));

        for (name, values) in overrides {
            let action = A::parse(name).expect("keymap binding action was validated above");
            let values = values
                .as_array()
                .with_context(|| format!("{} binding {:?} must be an array", A::LABEL, name))?;
            for value in values {
                let source = value.as_str().with_context(|| {
                    format!("{} binding {:?} entries must be strings", A::LABEL, name)
                })?;
                let key = Key::parse_binding(source)
                    .with_context(|| format!("{} binding action {:?}", A::LABEL, name))?
                    .binding_identity();
                if let Some(existing) = self.bindings.insert(key, action) {
                    bail!(
                        "{} key {:?} is assigned to both {:?} and {:?}",
                        A::LABEL,
                        source,
                        existing.name(),
                        action.name()
                    );
                }
            }
        }
        Ok(())
    }

    pub(crate) fn action(&self, key: Key) -> Option<A> {
        self.bindings.get(&key.binding_identity()).copied()
    }

    pub(crate) fn bindings(&self) -> impl Iterator<Item = (Key, A)> + '_ {
        self.bindings
            .iter()
            .map(|(key, action)| (key.key(), *action))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum Action {
        Back,
        Copy,
    }

    fn parse_action(name: &str) -> Option<Action> {
        match name {
            "back" => Some(Action::Back),
            "copy" => Some(Action::Copy),
            _ => None,
        }
    }

    #[test]
    fn patch_rebinds_by_physical_key_without_knowing_the_previous_action() {
        let mut bindings = HashMap::from([(Key::Enter.binding_identity(), Action::Copy)]);
        apply_patch(
            &mut bindings,
            json!({"enter": "back", "ctrl+y": "copy"}),
            "test",
            parse_action,
        )
        .unwrap();
        assert_eq!(
            bindings.get(&Key::Enter.binding_identity()),
            Some(&Action::Back)
        );
        assert_eq!(
            bindings.get(&Key::Ctrl('y').binding_identity()),
            Some(&Action::Copy)
        );
    }

    #[test]
    fn patch_disables_an_inherited_key_with_a_tombstone() {
        let mut bindings = HashMap::from([(Key::Enter.binding_identity(), Action::Copy)]);
        apply_patch(&mut bindings, json!({"enter": false}), "test", parse_action).unwrap();
        assert!(!bindings.contains_key(&Key::Enter.binding_identity()));
    }

    #[test]
    fn patch_uses_canonical_identity_for_uppercase_input() {
        let mut bindings = HashMap::new();
        apply_patch(&mut bindings, json!({"a": "copy"}), "test", parse_action).unwrap();
        assert_eq!(
            bindings.get(&Key::Char('A').binding_identity()),
            Some(&Action::Copy)
        );
    }

    #[test]
    fn patch_rejects_a_tombstone_and_rebind_of_the_same_key() {
        let mut bindings = HashMap::new();
        let error = apply_patch(
            &mut bindings,
            json!({"enter": false, "ctrl+j": "copy"}),
            "test",
            parse_action,
        )
        .expect_err("one key cannot be both disabled and rebound");
        assert!(error.to_string().contains("both disabled and rebound"));
    }

    #[test]
    fn patch_rejects_duplicate_tombstones_by_physical_key() {
        let mut bindings = HashMap::new();
        let error = apply_patch(
            &mut bindings,
            json!({"esc": false, "escape": false}),
            "test",
            parse_action,
        )
        .expect_err("one physical key cannot have duplicate tombstones");
        assert!(error.to_string().contains("duplicate physical key"));
    }
}
