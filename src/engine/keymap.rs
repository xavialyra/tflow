use crate::expression::{Template, is_dynamic_string};
use crate::input::{BindingKey, Key};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BindingId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindingState {
    Ready,
    Pending,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BindingRecord<T> {
    pub(crate) target: T,
    pub(crate) state: BindingState,
}

#[derive(Debug)]
pub(crate) struct BindingStore<T> {
    next_id: u64,
    records: HashMap<BindingId, BindingRecord<T>>,
}

impl<T> Default for BindingStore<T> {
    fn default() -> Self {
        Self {
            next_id: 1,
            records: HashMap::new(),
        }
    }
}

impl<T> BindingStore<T> {
    pub(crate) fn insert(&mut self, record: BindingRecord<T>) -> BindingId {
        let id = BindingId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.records.insert(id, record);
        id
    }

    pub(crate) fn get(&self, id: BindingId) -> Option<&BindingRecord<T>> {
        self.records.get(&id)
    }

    fn remove(&mut self, id: BindingId) {
        self.records.remove(&id);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayerEntry {
    Bind(BindingId),
    #[allow(dead_code)]
    Unbind,
}

#[derive(Debug, Clone)]
pub(crate) struct KeymapSnapshot<C> {
    context: C,
    revision: u64,
    bindings: HashMap<BindingKey, BindingId>,
}

impl<C: Copy> KeymapSnapshot<C> {
    pub(crate) fn context(&self) -> C {
        self.context
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    #[cfg(test)]
    pub(crate) fn resolve(&self, key: Key) -> Option<BindingId> {
        self.bindings.get(&key.binding_identity()).copied()
    }

    pub(crate) fn bindings(&self) -> impl Iterator<Item = (Key, BindingId)> + '_ {
        self.bindings.iter().map(|(key, id)| (key.key(), *id))
    }
}

#[derive(Debug, Default)]
struct ContextState {
    layers: BTreeMap<i16, HashMap<BindingKey, LayerEntry>>,
    effective: HashMap<BindingKey, BindingId>,
    revision: u64,
}

#[derive(Debug)]
pub(crate) struct DynamicKeymap<C> {
    contexts: HashMap<C, ContextState>,
    next_revision: u64,
}

impl<C> Default for DynamicKeymap<C> {
    fn default() -> Self {
        Self {
            contexts: HashMap::new(),
            next_revision: 1,
        }
    }
}

impl<C> DynamicKeymap<C>
where
    C: Copy + Eq + Hash,
{
    #[cfg(test)]
    pub(crate) fn set_layer(
        &mut self,
        context: C,
        priority: i16,
        entries: HashMap<BindingKey, LayerEntry>,
    ) {
        self.set_layers([(context, priority, entries)]);
    }

    pub(crate) fn set_layers(
        &mut self,
        updates: impl IntoIterator<Item = (C, i16, HashMap<BindingKey, LayerEntry>)>,
    ) {
        let mut changed = HashSet::new();
        for (context, priority, entries) in updates {
            let state = self.contexts.entry(context).or_default();
            if state.layers.get(&priority) != Some(&entries) {
                state.layers.insert(priority, entries);
                changed.insert(context);
            }
        }
        for context in changed {
            self.recompute(context);
        }
    }

    #[cfg(test)]
    pub(crate) fn remove_layer(&mut self, context: C, priority: i16) {
        let Some(state) = self.contexts.get_mut(&context) else {
            return;
        };
        if state.layers.remove(&priority).is_some() {
            self.recompute(context);
        }
    }

    pub(crate) fn remove_context(&mut self, context: C) {
        self.contexts.remove(&context);
    }

    pub(crate) fn resolve(&self, context: C, key: Key) -> Option<BindingId> {
        self.contexts
            .get(&context)?
            .effective
            .get(&key.binding_identity())
            .copied()
    }

    pub(crate) fn snapshot(&self, context: C) -> KeymapSnapshot<C> {
        let (revision, bindings) = self
            .contexts
            .get(&context)
            .map(|state| (state.revision, state.effective.clone()))
            .unwrap_or_default();
        KeymapSnapshot {
            context,
            revision,
            bindings,
        }
    }

    fn recompute(&mut self, context: C) {
        let state = self
            .contexts
            .get_mut(&context)
            .expect("context was inserted above");
        let mut effective = HashMap::new();
        for layer in state.layers.values() {
            for (key, entry) in layer {
                match entry {
                    LayerEntry::Bind(id) => {
                        effective.insert(*key, *id);
                    }
                    LayerEntry::Unbind => {
                        effective.remove(key);
                    }
                }
            }
        }
        if state.effective != effective {
            state.effective = effective;
            state.revision = self.next_revision;
            self.next_revision = self.next_revision.wrapping_add(1).max(1);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct InputContextId(u64);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct LayerId(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BindingEntry<T> {
    Bind(BindingRecord<T>),
    #[allow(dead_code)]
    Unbind,
}

type LayerReplacement<T> = (LayerId, Vec<(Key, BindingEntry<T>)>);

#[derive(Debug)]
struct MountedLayer<T> {
    context: InputContextId,
    priority: i16,
    entries: HashMap<BindingKey, BindingEntry<T>>,
    binding_ids: Vec<BindingId>,
}

#[derive(Debug)]
pub(crate) struct InputRouter<T> {
    next_context: u64,
    next_layer: u64,
    keymap: DynamicKeymap<InputContextId>,
    bindings: BindingStore<T>,
    layers: HashMap<LayerId, MountedLayer<T>>,
}

impl<T> Default for InputRouter<T> {
    fn default() -> Self {
        Self {
            next_context: 1,
            next_layer: 1,
            keymap: DynamicKeymap::default(),
            bindings: BindingStore::default(),
            layers: HashMap::new(),
        }
    }
}

impl<T> InputRouter<T>
where
    T: Clone + Eq,
{
    pub(crate) fn create_context(&mut self) -> InputContextId {
        let id = InputContextId(self.next_context);
        self.next_context = self.next_context.wrapping_add(1).max(1);
        id
    }

    pub(crate) fn mount_layer(&mut self, context: InputContextId, priority: i16) -> LayerId {
        assert!(
            !self
                .layers
                .values()
                .any(|layer| layer.context == context && layer.priority == priority),
            "an input context can only mount one layer at each priority"
        );
        let id = LayerId(self.next_layer);
        self.next_layer = self.next_layer.wrapping_add(1).max(1);
        self.layers.insert(
            id,
            MountedLayer {
                context,
                priority,
                entries: HashMap::new(),
                binding_ids: Vec::new(),
            },
        );
        id
    }

    #[cfg(test)]
    pub(crate) fn replace_layer(
        &mut self,
        layer: LayerId,
        entries: impl IntoIterator<Item = (Key, BindingEntry<T>)>,
    ) -> Result<()> {
        self.replace_layers(vec![(layer, entries.into_iter().collect())])
    }

    pub(crate) fn replace_layers(&mut self, replacements: Vec<LayerReplacement<T>>) -> Result<()> {
        let mut prepared = Vec::with_capacity(replacements.len());
        let mut seen_layers = HashSet::new();
        for (layer, entries) in replacements {
            anyhow::ensure!(
                seen_layers.insert(layer),
                "input layer is replaced more than once"
            );
            let mut normalized = HashMap::new();
            for (key, entry) in entries {
                if normalized.insert(key.binding_identity(), entry).is_some() {
                    anyhow::bail!(
                        "key {:?} is registered more than once in one input layer",
                        key.binding_name()
                    );
                }
            }
            anyhow::ensure!(
                self.layers.contains_key(&layer),
                "input layer is not mounted"
            );
            prepared.push((layer, normalized));
        }

        let mut keymap_updates = Vec::new();
        for (layer, entries) in prepared {
            if self.layers[&layer].entries == entries {
                continue;
            }
            let context = self.layers[&layer].context;
            let priority = self.layers[&layer].priority;
            let old_ids = self.layers[&layer].binding_ids.clone();
            let mut binding_ids = Vec::new();
            let mut resolved = HashMap::new();
            for (key, entry) in &entries {
                match entry {
                    BindingEntry::Bind(record) => {
                        let id = self.bindings.insert(record.clone());
                        binding_ids.push(id);
                        resolved.insert(*key, LayerEntry::Bind(id));
                    }
                    BindingEntry::Unbind => {
                        resolved.insert(*key, LayerEntry::Unbind);
                    }
                }
            }
            keymap_updates.push((context, priority, resolved));
            for id in old_ids {
                self.bindings.remove(id);
            }
            let mounted = self
                .layers
                .get_mut(&layer)
                .expect("layer was checked above");
            mounted.entries = entries;
            mounted.binding_ids = binding_ids;
        }
        self.keymap.set_layers(keymap_updates);
        Ok(())
    }

    pub(crate) fn remove_context(&mut self, context: InputContextId) {
        let layers = self
            .layers
            .iter()
            .filter_map(|(id, layer)| (layer.context == context).then_some(*id))
            .collect::<Vec<_>>();
        for layer in layers {
            let mounted = self.layers.remove(&layer).expect("layer exists above");
            for id in mounted.binding_ids {
                self.bindings.remove(id);
            }
        }
        self.keymap.remove_context(context);
    }

    pub(crate) fn snapshot(&self, context: InputContextId) -> KeymapSnapshot<InputContextId> {
        self.keymap.snapshot(context)
    }

    pub(crate) fn resolve(&self, context: InputContextId, key: Key) -> Option<&BindingRecord<T>> {
        let id = self.keymap.resolve(context, key)?;
        self.bindings.get(id)
    }

    pub(crate) fn record(&self, id: BindingId) -> Option<&BindingRecord<T>> {
        self.bindings.get(id)
    }

    #[cfg(test)]
    fn allocation_counts(&self) -> (usize, usize, usize) {
        (
            self.keymap.contexts.len(),
            self.layers.len(),
            self.bindings.records.len(),
        )
    }
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
        if let Some(source) = value.as_str() {
            if Template::parse(source)?.is_complete_path() {
                return Ok(());
            }
            bail!(
                "{} bindings must be an object or complete dynamic path",
                A::LABEL
            );
        }
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
                if is_dynamic_string(source) {
                    Template::parse(source)?;
                } else {
                    Key::parse_binding(source)
                        .with_context(|| format!("{} binding action {:?}", A::LABEL, name))?;
                }
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
            .with_context(|| format!("{} bindings must evaluate to an object", A::LABEL))?;
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

    #[cfg(test)]
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
    fn dynamic_layers_override_and_reveal_lower_bindings() {
        let mut store = BindingStore::default();
        let lower = store.insert(BindingRecord {
            target: Action::Back,
            state: BindingState::Ready,
        });
        let higher = store.insert(BindingRecord {
            target: Action::Copy,
            state: BindingState::Ready,
        });
        let mut keymap = DynamicKeymap::default();
        keymap.set_layer(
            "normal",
            100,
            HashMap::from([(Key::Enter.binding_identity(), LayerEntry::Bind(lower))]),
        );
        keymap.set_layer(
            "normal",
            200,
            HashMap::from([(Key::Enter.binding_identity(), LayerEntry::Bind(higher))]),
        );
        assert_eq!(keymap.snapshot("normal").resolve(Key::Enter), Some(higher));

        keymap.remove_layer("normal", 200);
        assert_eq!(keymap.snapshot("normal").resolve(Key::Enter), Some(lower));
    }

    #[test]
    fn unbind_stops_at_the_layer_without_consuming_the_key() {
        let mut store = BindingStore::default();
        let lower = store.insert(BindingRecord {
            target: Action::Back,
            state: BindingState::Ready,
        });
        let mut keymap = DynamicKeymap::default();
        keymap.set_layer(
            "normal",
            100,
            HashMap::from([(Key::Char('x').binding_identity(), LayerEntry::Bind(lower))]),
        );
        keymap.set_layer(
            "normal",
            200,
            HashMap::from([(Key::Char('x').binding_identity(), LayerEntry::Unbind)]),
        );
        assert_eq!(keymap.snapshot("normal").resolve(Key::Char('x')), None);
    }

    #[test]
    fn contexts_have_independent_snapshots_and_revisions() {
        let mut store = BindingStore::default();
        let binding = store.insert(BindingRecord {
            target: Action::Copy,
            state: BindingState::Ready,
        });
        let mut keymap = DynamicKeymap::default();
        keymap.set_layer(
            "normal",
            100,
            HashMap::from([(Key::Enter.binding_identity(), LayerEntry::Bind(binding))]),
        );
        let normal = keymap.snapshot("normal");
        let passthrough = keymap.snapshot("passthrough");
        assert!(normal.revision() > 0);
        assert_eq!(normal.context(), "normal");
        assert_eq!(normal.resolve(Key::Enter), Some(binding));
        assert_eq!(passthrough.revision(), 0);
        assert_eq!(passthrough.resolve(Key::Enter), None);

        keymap.remove_context("normal");
        let removed = keymap.snapshot("normal");
        assert_eq!(removed.revision(), 0);
        assert_eq!(removed.resolve(Key::Enter), None);
    }

    #[test]
    fn router_replaces_layers_incrementally_and_reclaims_contexts() {
        let mut router = InputRouter::default();
        let context = router.create_context();
        let action_layer = router.mount_layer(context, 100);
        let command_layer = router.mount_layer(context, 200);
        router
            .replace_layer(
                action_layer,
                [(
                    Key::Enter,
                    BindingEntry::Bind(BindingRecord {
                        target: Action::Back,
                        state: BindingState::Ready,
                    }),
                )],
            )
            .unwrap();
        router
            .replace_layer(
                command_layer,
                [(
                    Key::Enter,
                    BindingEntry::Bind(BindingRecord {
                        target: Action::Copy,
                        state: BindingState::Ready,
                    }),
                )],
            )
            .unwrap();
        assert_eq!(
            router.resolve(context, Key::Enter).unwrap().target,
            Action::Copy
        );

        let error = router
            .replace_layers(vec![
                (
                    action_layer,
                    vec![(
                        Key::Escape,
                        BindingEntry::Bind(BindingRecord {
                            target: Action::Copy,
                            state: BindingState::Ready,
                        }),
                    )],
                ),
                (
                    command_layer,
                    vec![
                        (
                            Key::Enter,
                            BindingEntry::Bind(BindingRecord {
                                target: Action::Back,
                                state: BindingState::Ready,
                            }),
                        ),
                        (
                            Key::Enter,
                            BindingEntry::Bind(BindingRecord {
                                target: Action::Copy,
                                state: BindingState::Ready,
                            }),
                        ),
                    ],
                ),
            ])
            .unwrap_err();
        assert!(error.to_string().contains("more than once"));
        assert_eq!(
            router.resolve(context, Key::Enter).unwrap().target,
            Action::Copy
        );
        assert!(router.resolve(context, Key::Escape).is_none());

        router.replace_layer(command_layer, []).unwrap();
        assert_eq!(
            router.resolve(context, Key::Enter).unwrap().target,
            Action::Back
        );
        router.remove_context(context);
        assert_eq!(router.allocation_counts(), (0, 0, 0));
        assert!(router.resolve(context, Key::Enter).is_none());
    }

    #[test]
    fn pending_binding_keeps_its_identity_and_does_not_expose_a_lower_layer() {
        let mut store = BindingStore::default();
        let lower = store.insert(BindingRecord {
            target: Action::Back,
            state: BindingState::Ready,
        });
        let pending = store.insert(BindingRecord {
            target: Action::Copy,
            state: BindingState::Pending,
        });
        let mut keymap = DynamicKeymap::default();
        keymap.set_layer(
            "normal",
            100,
            HashMap::from([(Key::Enter.binding_identity(), LayerEntry::Bind(lower))]),
        );
        keymap.set_layer(
            "normal",
            200,
            HashMap::from([(Key::Enter.binding_identity(), LayerEntry::Bind(pending))]),
        );

        let resolved = keymap.snapshot("normal").resolve(Key::Enter).unwrap();
        assert_eq!(resolved, pending);
        assert_eq!(store.get(resolved).unwrap().state, BindingState::Pending);
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
