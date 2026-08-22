use crate::expression::{Template, is_dynamic_string};
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
    if let Some(source) = value.as_str() {
        if Template::parse(source)?.is_complete_path() {
            return Ok(());
        }
        bail!("{label} keymap must be an object or complete dynamic path");
    }
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
        if is_dynamic_string(action) {
            Template::parse(action)?;
        } else {
            parse_action(action)
                .with_context(|| format!("unsupported {label} keymap action {:?}", action))?;
        }
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
        let values = values
            .iter()
            .filter(|value| !value.as_str().is_some_and(is_dynamic_string))
            .cloned()
            .collect();
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
        .filter(|(_, value)| !value.as_str().is_some_and(is_dynamic_string))
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
        .with_context(|| format!("{label} keymap must evaluate to an object"))?;
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
