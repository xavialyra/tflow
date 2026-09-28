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
        .with_context(|| format!("{label} bindings must be an object"))?;
    for (source, value) in patch {
        Key::parse_binding(source)
            .with_context(|| format!("{label} bindings entry {:?}", source))?;
        if value.as_bool() == Some(false) {
            continue;
        }
        if value.is_boolean() {
            bail!(
                "{label} bindings entry {:?} must be false or an action",
                source
            );
        }
        let action = value.as_str().with_context(|| {
            format!(
                "{label} bindings entry {:?} must be false or an action",
                source
            )
        })?;
        parse_action(action)
            .with_context(|| format!("unsupported {label} binding action {:?}", action))?;
    }
    Ok(())
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
        .with_context(|| format!("{label} bindings must be an object"))?;
    let mut tombstones = HashSet::new();
    let mut rebound = HashSet::new();

    for (source, value) in patch {
        if source.is_empty() {
            continue;
        }
        let key = Key::parse_binding(source)
            .with_context(|| format!("{label} bindings entry {:?}", source))?
            .binding_identity();
        if value.as_bool() == Some(false) {
            if rebound.contains(&key) {
                bail!(
                    "{label} binding key {:?} cannot be both disabled and rebound",
                    source
                );
            }
            if !tombstones.insert(key) {
                bail!(
                    "{label} bindings contain duplicate physical key {:?}",
                    source
                );
            }
            continue;
        }
        if value.is_boolean() {
            bail!(
                "{label} bindings entry {:?} must be false or an action",
                source
            );
        }
        if tombstones.contains(&key) {
            bail!(
                "{label} binding key {:?} cannot be both disabled and rebound",
                source
            );
        }
        if !rebound.insert(key) {
            bail!(
                "{label} bindings contain duplicate physical key {:?}",
                source
            );
        }
        let action_name = value.as_str().with_context(|| {
            format!(
                "{label} bindings entry {:?} must be false or an action",
                source
            )
        })?;
        if let Some(action) = parse_action(action_name) {
            bindings.insert(key, action);
        } else {
            bindings.remove(&key);
        }
    }

    for key in tombstones {
        bindings.remove(&key);
    }
    Ok(())
}

pub(crate) trait BindingAction: Copy + Eq + Hash {
    const LABEL: &'static str;

    fn name(self) -> &'static str;
    /// Human label for the action, shown in the command palette and footer.
    fn label(self) -> &'static str;
    fn parse(name: &str) -> Option<Self>;
    fn default_bindings() -> &'static [(Key, Self)];
}

/// Resolves a bare engine action name into its fully-qualified command id
/// (`<engine>.<action>`) and human label, so the command index can address an
/// engine action exactly like any other command.
pub(crate) fn binding_action_spec<A: BindingAction>(name: &str) -> Option<(String, &'static str)> {
    A::parse(name).map(|action| (format!("{}.{}", A::LABEL, action.name()), action.label()))
}

#[derive(Debug, Clone)]
pub(crate) struct ActionBindings<A> {
    bindings: HashMap<BindingKey, A>,
}

impl<A: BindingAction + 'static> ActionBindings<A> {
    pub(crate) fn validate_defaults(defaults: Option<&Value>) -> Result<()> {
        validate_patch(defaults, A::LABEL, A::parse)?;
        Self::from_defaults(defaults.cloned())?;
        Ok(())
    }

    pub(crate) fn from_defaults(defaults: Option<Value>) -> Result<Self> {
        let mut bindings = Self {
            bindings: A::default_bindings()
                .iter()
                .map(|(key, action)| (key.binding_identity(), *action))
                .collect(),
        };
        if let Some(defaults) = defaults {
            apply_patch(&mut bindings.bindings, defaults, A::LABEL, A::parse)?;
        }
        Ok(bindings)
    }

    #[cfg(test)]
    pub(crate) fn action(&self, key: Key) -> Option<A> {
        self.bindings.get(&key.binding_identity()).copied()
    }

    /// The configured bindings in a stable order.
    ///
    /// The table is a `HashMap`, so its iteration order is arbitrary. Entries
    /// built from it (registry commands, footer hints, the command palette)
    /// would otherwise change order between runs, and a positional
    /// change-detection would report a change that did not happen. Keys are
    /// ordered by their canonical spelling, which is the order they read in.
    pub(crate) fn bindings(&self) -> impl Iterator<Item = (Key, A)> + '_ {
        let mut entries = self
            .bindings
            .iter()
            .map(|(key, action)| (key.key(), *action))
            .collect::<Vec<_>>();
        entries.sort_by_cached_key(|(key, _)| {
            (
                key.binding_name().unwrap_or_default(),
                // Keys without a canonical spelling (only reachable by direct
                // construction) still need a deterministic tiebreaker.
                format!("{key:?}"),
            )
        });
        entries.into_iter()
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

    impl BindingAction for Action {
        const LABEL: &'static str = "test";
        fn name(self) -> &'static str {
            match self {
                Action::Back => "back",
                Action::Copy => "copy",
            }
        }
        fn label(self) -> &'static str {
            match self {
                Action::Back => "Back",
                Action::Copy => "Copy",
            }
        }
        fn parse(name: &str) -> Option<Self> {
            parse_action(name)
        }
        fn default_bindings() -> &'static [(Key, Self)] {
            &[(Key::Escape, Action::Back)]
        }
    }

    fn parse_action(name: &str) -> Option<Action> {
        match name {
            "back" => Some(Action::Back),
            "copy" => Some(Action::Copy),
            _ => None,
        }
    }

    #[test]
    fn bindings_iterate_in_a_stable_canonical_order() {
        let bindings = ActionBindings::<Action>::from_defaults(Some(json!({
            "ctrl+d": "copy",
            "escape": "copy",
            "alt+x": "back",
            "ctrl+c": "back",
        })))
        .unwrap();

        let keys: Vec<String> = bindings
            .bindings()
            .map(|(key, _)| key.binding_name().unwrap())
            .collect();
        assert_eq!(keys, ["alt+x", "ctrl+c", "ctrl+d", "escape"]);

        // Two keys may drive one action, and both are kept: the table is a
        // binding list, not a one-key-per-action list.
        assert_eq!(
            bindings
                .bindings()
                .filter(|(_, action)| *action == Action::Back)
                .count(),
            2
        );
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
    #[test]
    fn key_centric_bindings_are_accepted_in_defaults() {
        let bindings = ActionBindings::<Action>::from_defaults(Some(json!({
            "ctrl+x": "copy",
            "escape": false,
        })))
        .unwrap();
        assert_eq!(bindings.action(Key::Ctrl('x')), Some(Action::Copy));
        assert_eq!(bindings.action(Key::Escape), None);
    }
}
