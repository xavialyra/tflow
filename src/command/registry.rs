use crate::input::Key;
use anyhow::{Result, ensure};
use std::cell::Cell;
use std::collections::HashSet;

pub(crate) const MAX_COMMAND_CALL_DEPTH: u8 = 2;

thread_local! {
    static COMMAND_CALL_DEPTH: Cell<u8> = const { Cell::new(0) };
}

pub(crate) fn command_call_depth() -> u8 {
    COMMAND_CALL_DEPTH.with(Cell::get)
}

pub(crate) fn with_command_call_depth<T>(depth: u8, action: impl FnOnce() -> T) -> T {
    COMMAND_CALL_DEPTH.with(|current| {
        let previous = current.replace(depth);
        let result = action();
        current.set(previous);
        result
    })
}

/// Runs `action` with the command call depth set, rejecting invocations past
/// [`MAX_COMMAND_CALL_DEPTH`]. The actual handler is resolved by the caller, so
/// the registry itself never owns executable state.
pub(crate) fn execute_at_depth<T>(depth: u8, action: impl FnOnce() -> Result<T>) -> Result<T> {
    ensure!(
        depth <= MAX_COMMAND_CALL_DEPTH,
        "command invocation depth {} exceeds maximum {}",
        depth,
        MAX_COMMAND_CALL_DEPTH
    );
    with_command_call_depth(depth, action)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum BindingLayer {
    View,
    Engine,
    Host,
}

impl BindingLayer {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Engine => "engine",
            Self::Host => "host",
        }
    }

    /// Parses the config-facing layer name used by `unbind.layers`.
    pub(crate) fn from_layer(layer: &str) -> Option<Self> {
        Some(match layer {
            "view" => Self::View,
            "engine" => Self::Engine,
            "host" => Self::Host,
            _ => return None,
        })
    }
}

/// A pure binding projection. The registry stores no executable target; the
/// command definition or engine action is resolved by id at dispatch time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandEntry {
    pub(crate) id: String,
    pub(crate) label: Option<String>,
    pub(crate) key: Option<Key>,
    pub(crate) layer: BindingLayer,
}

impl CommandEntry {
    pub(crate) fn new(
        id: impl Into<String>,
        label: Option<String>,
        key: Option<Key>,
        layer: BindingLayer,
    ) -> Self {
        Self {
            id: id.into(),
            label,
            key,
            layer,
        }
    }

    /// An engine-layer entry that dispatches to the current View's `on_command`.
    pub(crate) fn for_event(
        id: impl Into<String>,
        label: Option<String>,
        key: Option<Key>,
        layer: BindingLayer,
    ) -> Self {
        Self::new(id, label, key, layer)
    }

    pub(crate) fn same_definition(&self, other: &Self) -> bool {
        self.id == other.id
            && self.label == other.label
            && self.key.map(|k| k.binding_identity()) == other.key.map(|k| k.binding_identity())
            && self.layer == other.layer
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommandsChanged {
    pub(crate) revision: u64,
}

/// A View's resolved unbinding rules: the priority layers to drop, the command
/// FQIDs to strip of their keys, and the physical keys to release.
///
/// This is the resolved form of the config `Unbind` table: addresses have
/// already become registry ids and key names have already become binding
/// identities, so the command layer never parses workflow syntax.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct UnbindRules {
    pub(crate) layers: HashSet<BindingLayer>,
    pub(crate) commands: HashSet<String>,
    pub(crate) keys: HashSet<crate::input::BindingKey>,
}

#[derive(Default)]
pub(crate) struct CommandRegistry {
    revision: u64,
    host_entries: Vec<CommandEntry>,
    engine_entries: Vec<CommandEntry>,
    view_entries: Vec<CommandEntry>,
    unbind: UnbindRules,
}

impl CommandRegistry {
    pub(crate) fn new() -> Self {
        Self {
            revision: 0,
            host_entries: Vec::new(),
            engine_entries: Vec::new(),
            view_entries: Vec::new(),
            unbind: UnbindRules::default(),
        }
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn layer_entries(&self, layer: BindingLayer) -> &[CommandEntry] {
        match layer {
            BindingLayer::View => &self.view_entries,
            BindingLayer::Engine => &self.engine_entries,
            BindingLayer::Host => &self.host_entries,
        }
    }

    /// Atomically replaces all entries in the given layer.
    ///
    /// Validates that no two entries within the layer share the same physical key.
    /// If validation fails, returns an error and leaves the existing layer unchanged.
    /// If the new entries are identical to the committed layer, returns Ok(None).
    /// If entries changed, increments `revision` and returns Ok(Some(CommandsChanged)).
    pub(crate) fn replace_layer(
        &mut self,
        layer: BindingLayer,
        entries: Vec<CommandEntry>,
    ) -> Result<Option<CommandsChanged>> {
        let mut seen = HashSet::new();
        for entry in &entries {
            if let Some(key) = entry.key {
                let identity = key.binding_identity();
                ensure!(
                    seen.insert(identity),
                    "layer {:?} contains duplicate key {:?}",
                    layer,
                    key.binding_name()
                );
            }
        }

        let current = self.layer_entries(layer);
        let definition_changed = current.len() != entries.len()
            || !current
                .iter()
                .zip(&entries)
                .all(|(a, b)| a.same_definition(b));

        match layer {
            BindingLayer::View => self.view_entries = entries,
            BindingLayer::Engine => self.engine_entries = entries,
            BindingLayer::Host => self.host_entries = entries,
        }

        if definition_changed {
            self.revision = self.revision.wrapping_add(1).max(1);
            Ok(Some(CommandsChanged {
                revision: self.revision,
            }))
        } else {
            Ok(None)
        }
    }

    /// Resolves a key event to a CommandEntry using priority:
    /// View > Engine > Host.
    ///
    /// Returns `None` when no entry is bound to `key`. A key whose binding was
    /// removed by `unbind` (or that was never bound) is not found here, so the
    /// caller falls through to raw input handling.
    pub(crate) fn resolve(&self, key: Key) -> Option<&CommandEntry> {
        self.view_entries
            .iter()
            .chain(self.engine_entries.iter())
            .chain(self.host_entries.iter())
            .find(|entry| {
                self.effective_key(entry)
                    .is_some_and(|bound| bound.binding_identity() == key.binding_identity())
            })
    }

    /// Resolves an entry by its ID, honoring priority View > Engine > Host.
    ///
    /// Identity invocation is key-independent, so a command whose key was
    /// removed by `unbind` stays invokable by reference.
    pub(crate) fn resolve_id(&self, id: &str) -> Option<&CommandEntry> {
        self.view_entries
            .iter()
            .chain(self.engine_entries.iter())
            .chain(self.host_entries.iter())
            .find(|entry| entry.id == id)
    }

    pub(crate) fn replace_unbinds(&mut self, rules: UnbindRules) -> bool {
        if self.unbind == rules {
            return false;
        }
        self.unbind = rules;
        self.revision = self.revision.wrapping_add(1).max(1);
        true
    }

    /// The key an entry exposes after `unbind` is applied.
    ///
    /// Unbinding removes the key-to-command association, so an unbound entry
    /// behaves exactly like one declared without a key: it stays discoverable
    /// and invokable by identity, but exposes no binding.
    fn effective_key(&self, entry: &CommandEntry) -> Option<Key> {
        if self.is_unbound(entry) {
            None
        } else {
            entry.key
        }
    }

    fn is_unbound(&self, entry: &CommandEntry) -> bool {
        self.unbind.layers.contains(&entry.layer)
            || self.unbind.commands.contains(&entry.id)
            || entry
                .key
                .is_some_and(|key| self.unbind.keys.contains(&key.binding_identity()))
    }

    /// Returns the effective bindings in priority order: View > Engine > Host.
    ///
    /// One row per effective binding, not per command: a command bound to two
    /// keys keeps both rows, so no shortcut is silently lost from the footer
    /// hints or the command palette. A key a higher layer already claimed
    /// degrades that row to `None`, which is how a shadowed command stays
    /// discoverable. The one row not emitted is a keyless one for a command that
    /// is already listed with a key: a lower layer's shadowed binding of a
    /// reachable command adds no way to reach it.
    pub(crate) fn picker_entries(&self) -> Vec<(&CommandEntry, Option<Key>)> {
        let mut seen_keys = HashSet::new();
        let mut resolved = Vec::new();

        let all = self
            .view_entries
            .iter()
            .chain(self.engine_entries.iter())
            .chain(self.host_entries.iter());

        for entry in all {
            let key = self
                .effective_key(entry)
                .filter(|key| seen_keys.insert(key.binding_identity()));
            resolved.push((entry, key));
        }

        let reachable: HashSet<&str> = resolved
            .iter()
            .filter_map(|(entry, key)| key.map(|_| entry.id.as_str()))
            .collect();
        resolved.retain(|(entry, key)| key.is_some() || !reachable.contains(entry.id.as_str()));
        resolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The config-facing layer names and the layer enum agree, so `unbind.layers`
    /// and validation parse exactly one vocabulary.
    #[test]
    fn binding_layer_names_round_trip() {
        for layer in [BindingLayer::View, BindingLayer::Engine, BindingLayer::Host] {
            assert_eq!(BindingLayer::from_layer(layer.as_str()), Some(layer));
        }
        assert_eq!(BindingLayer::from_layer("view "), None);
        assert_eq!(BindingLayer::from_layer("global"), None);
    }

    /// `unbind.commands` targets one command by its registry id, independent of
    /// the entry's priority layer, while leaving sibling bindings alone.
    #[test]
    fn unbind_commands_removes_one_entrys_key_regardless_of_layer() {
        let mut registry = CommandRegistry::new();
        registry
            .replace_layer(
                BindingLayer::Engine,
                vec![
                    CommandEntry::for_event(
                        "picker.clear_input",
                        Some("Clear Input".into()),
                        Some(Key::Ctrl('u')),
                        BindingLayer::Engine,
                    ),
                    CommandEntry::for_event(
                        "picker.delete_word",
                        Some("Delete Word".into()),
                        Some(Key::Ctrl('w')),
                        BindingLayer::Engine,
                    ),
                ],
            )
            .unwrap();

        registry.replace_unbinds(UnbindRules {
            commands: HashSet::from(["picker.clear_input".to_string()]),
            ..Default::default()
        });

        assert!(registry.resolve(Key::Ctrl('u')).is_none());
        assert!(registry.resolve(Key::Ctrl('w')).is_some());
        // The command stays discoverable and invokable by identity.
        assert!(registry.resolve_id("picker.clear_input").is_some());
    }

    /// The three unbind axes are independent: a key, a command, and a layer.
    #[test]
    fn unbind_keys_and_layers_are_separate_axes() {
        let mut registry = CommandRegistry::new();
        registry
            .replace_layer(
                BindingLayer::Host,
                vec![CommandEntry::new(
                    "host_palette",
                    Some("Commands".into()),
                    Some(Key::Ctrl('k')),
                    BindingLayer::Host,
                )],
            )
            .unwrap();

        // Unbinding a key touches only that physical key.
        registry.replace_unbinds(UnbindRules {
            keys: HashSet::from([Key::Ctrl('k').binding_identity()]),
            ..Default::default()
        });
        assert!(registry.resolve(Key::Ctrl('k')).is_none());
        assert!(registry.resolve_id("host_palette").is_some());

        // Unbinding a layer ignores every entry in it, without naming a command.
        registry.replace_unbinds(UnbindRules {
            layers: HashSet::from([BindingLayer::Host]),
            ..Default::default()
        });
        assert!(registry.resolve(Key::Ctrl('k')).is_none());
    }

    #[test]
    fn resolution_priority_is_view_then_engine_then_host() {
        let mut registry = CommandRegistry::new();
        let key = Key::Char('a');

        registry
            .replace_layer(
                BindingLayer::Host,
                vec![CommandEntry::new(
                    "host_a",
                    Some("Host A".into()),
                    Some(key),
                    BindingLayer::Host,
                )],
            )
            .unwrap();

        assert_eq!(registry.resolve(key).unwrap().id, "host_a");

        registry
            .replace_layer(
                BindingLayer::Engine,
                vec![CommandEntry::new(
                    "engine_a",
                    Some("Engine A".into()),
                    Some(key),
                    BindingLayer::Engine,
                )],
            )
            .unwrap();

        assert_eq!(registry.resolve(key).unwrap().id, "engine_a");

        registry
            .replace_layer(
                BindingLayer::View,
                vec![CommandEntry::new(
                    "view_a",
                    Some("View A".into()),
                    Some(key),
                    BindingLayer::View,
                )],
            )
            .unwrap();

        assert_eq!(registry.resolve(key).unwrap().id, "view_a");
    }

    #[test]
    fn same_layer_conflict_is_rejected_and_leaves_existing_entries() {
        let mut registry = CommandRegistry::new();
        let key = Key::Char('x');

        let initial = vec![CommandEntry::new(
            "view_x",
            Some("View X".into()),
            Some(key),
            BindingLayer::View,
        )];
        registry.replace_layer(BindingLayer::View, initial).unwrap();
        let rev = registry.revision();

        let conflicting = vec![
            CommandEntry::new("one", Some("One".into()), Some(key), BindingLayer::View),
            CommandEntry::new("two", Some("Two".into()), Some(key), BindingLayer::View),
        ];

        let result = registry.replace_layer(BindingLayer::View, conflicting);
        assert!(result.is_err());
        assert_eq!(registry.revision(), rev);
        assert_eq!(registry.resolve(key).unwrap().id, "view_x");
    }

    #[test]
    fn empty_replacement_clears_layer_and_increments_revision() {
        let mut registry = CommandRegistry::new();
        let key = Key::Char('y');
        registry
            .replace_layer(
                BindingLayer::View,
                vec![CommandEntry::new(
                    "view_y",
                    Some("Y".into()),
                    Some(key),
                    BindingLayer::View,
                )],
            )
            .unwrap();

        assert!(registry.resolve(key).is_some());
        let rev_before = registry.revision();

        let changed = registry
            .replace_layer(BindingLayer::View, Vec::new())
            .unwrap();
        assert!(changed.is_some());
        assert!(registry.revision() > rev_before);
        assert!(registry.resolve(key).is_none());
    }

    #[test]
    fn identical_replacement_does_not_increment_revision() {
        let mut registry = CommandRegistry::new();
        let key = Key::Char('z');
        let entries = vec![CommandEntry::new(
            "z",
            Some("Z".into()),
            Some(key),
            BindingLayer::Engine,
        )];

        let changed1 = registry
            .replace_layer(BindingLayer::Engine, entries.clone())
            .unwrap();
        assert!(changed1.is_some());
        let rev1 = registry.revision();

        let changed2 = registry
            .replace_layer(BindingLayer::Engine, entries)
            .unwrap();
        assert!(changed2.is_none());
        assert_eq!(registry.revision(), rev1);
    }

    #[test]
    fn picker_entries_clear_keys_for_unbound_commands() {
        let mut registry = CommandRegistry::new();
        registry
            .replace_layer(
                BindingLayer::Host,
                vec![CommandEntry::new(
                    "host_palette",
                    Some("Commands".into()),
                    Some(Key::Ctrl('k')),
                    BindingLayer::Host,
                )],
            )
            .unwrap();
        registry.replace_unbinds(UnbindRules {
            layers: HashSet::from([BindingLayer::Host]),
            ..Default::default()
        });

        // Physical dispatch is suppressed...
        assert!(registry.resolve(Key::Ctrl('k')).is_none());
        // ...and the entry exposes no key, exactly like one declared without a
        // binding, while remaining discoverable and invokable by identity.
        let entries = registry.picker_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1, None);
        assert!(registry.resolve_id("host_palette").is_some());
    }

    /// A command bound to two keys keeps both rows: the projection lists
    /// bindings, not commands, so a second key is not silently lost from the
    /// footer hints or the command palette.
    #[test]
    fn picker_entries_keeps_every_key_of_one_command() {
        let mut registry = CommandRegistry::new();
        registry
            .replace_layer(
                BindingLayer::Engine,
                vec![
                    CommandEntry::for_event(
                        "picker.exit",
                        Some("Exit".into()),
                        Some(Key::Ctrl('c')),
                        BindingLayer::Engine,
                    ),
                    CommandEntry::for_event(
                        "picker.exit",
                        Some("Exit".into()),
                        Some(Key::Ctrl('d')),
                        BindingLayer::Engine,
                    ),
                ],
            )
            .unwrap();

        let entries = registry.picker_entries();
        let rows: Vec<(String, Option<String>)> = entries
            .iter()
            .map(|(entry, key)| (entry.id.clone(), key.and_then(Key::binding_name)))
            .collect();
        assert_eq!(
            rows,
            [
                ("picker.exit".to_string(), Some("ctrl+c".to_string())),
                ("picker.exit".to_string(), Some("ctrl+d".to_string())),
            ]
        );

        // Both keys dispatch, which is what the rows promise.
        assert!(registry.resolve(Key::Ctrl('c')).is_some());
        assert!(registry.resolve(Key::Ctrl('d')).is_some());
    }

    /// A lower layer's binding of a command that is already reachable by a
    /// higher-layer key adds no row, because the command is listed anyway.
    #[test]
    fn picker_entries_drops_a_shadowed_binding_of_a_reachable_command() {
        let mut registry = CommandRegistry::new();
        let back = |key| {
            CommandEntry::for_event(
                "picker.back",
                Some("Back".into()),
                Some(key),
                BindingLayer::Engine,
            )
        };
        registry
            .replace_layer(
                BindingLayer::Engine,
                vec![back(Key::Escape), back(Key::Ctrl('b'))],
            )
            .unwrap();
        registry
            .replace_layer(
                BindingLayer::View,
                vec![CommandEntry::new(
                    "picker.back",
                    Some("Back".into()),
                    Some(Key::Escape),
                    BindingLayer::View,
                )],
            )
            .unwrap();

        let entries = registry.picker_entries();
        let rows: Vec<(Option<String>, BindingLayer)> = entries
            .iter()
            .map(|(entry, key)| (key.and_then(Key::binding_name), entry.layer))
            .collect();
        assert_eq!(
            rows,
            [
                (Some("escape".to_string()), BindingLayer::View),
                (Some("ctrl+b".to_string()), BindingLayer::Engine),
            ],
            "the Engine's shadowed escape binding of an already-listed command adds no row"
        );
    }

    #[test]
    fn picker_entries_orders_view_engine_host_and_degrades_conflicting_keys() {
        let mut registry = CommandRegistry::new();
        let key_common = Key::Ctrl('p');
        let key_host = Key::Ctrl('g');

        registry
            .replace_layer(
                BindingLayer::Host,
                vec![
                    CommandEntry::new(
                        "host_print",
                        Some("Host Print".into()),
                        Some(key_common),
                        BindingLayer::Host,
                    ),
                    CommandEntry::new(
                        "parameters",
                        Some("Parameters".into()),
                        Some(key_host),
                        BindingLayer::Host,
                    ),
                ],
            )
            .unwrap();

        registry
            .replace_layer(
                BindingLayer::Engine,
                vec![CommandEntry::new(
                    "engine_cmd",
                    Some("Engine Cmd".into()),
                    None,
                    BindingLayer::Engine,
                )],
            )
            .unwrap();

        registry
            .replace_layer(
                BindingLayer::View,
                vec![CommandEntry::new(
                    "view_print",
                    Some("View Print".into()),
                    Some(key_common),
                    BindingLayer::View,
                )],
            )
            .unwrap();

        let entries = registry.picker_entries();
        // Scope order should be View -> Engine -> Host.
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].0.id, "view_print");
        assert_eq!(entries[0].0.layer, BindingLayer::View);
        assert_eq!(entries[0].1, Some(key_common));

        assert_eq!(entries[1].0.id, "engine_cmd");
        assert_eq!(entries[1].0.layer, BindingLayer::Engine);
        assert_eq!(entries[1].1, None);

        // host_print's key_common is preempted by view_print, degraded to None while retaining the entry.
        assert_eq!(entries[2].0.id, "host_print");
        assert_eq!(entries[2].0.layer, BindingLayer::Host);
        assert_eq!(entries[2].1, None);

        assert_eq!(entries[3].0.id, "parameters");
        assert_eq!(entries[3].0.layer, BindingLayer::Host);
        assert_eq!(entries[3].1, Some(key_host));
    }

    #[test]
    fn command_execution_rejects_depth_above_two_and_restores_context() {
        assert!(execute_at_depth(2, || Ok::<_, anyhow::Error>(())).is_ok());
        assert!(execute_at_depth(3, || Ok::<_, anyhow::Error>(())).is_err());
        assert_eq!(command_call_depth(), 0);
    }

    /// A key can be rebound at the View layer. The View entry then *replaces*
    /// the Engine-layer binding for that key; the Engine entry keeps its own
    /// row but exposes no key, so the shortcut is not advertised twice.
    #[test]
    fn a_view_binding_shadows_the_engine_key_it_replaces() {
        let mut registry = CommandRegistry::new();
        let key = Key::Escape;
        registry
            .replace_layer(
                BindingLayer::Engine,
                vec![CommandEntry::for_event(
                    "picker.back",
                    Some("Back".into()),
                    Some(key),
                    BindingLayer::Engine,
                )],
            )
            .unwrap();
        registry
            .replace_layer(
                BindingLayer::View,
                vec![CommandEntry::new(
                    "core.exit",
                    Some("Exit".into()),
                    Some(key),
                    BindingLayer::View,
                )],
            )
            .unwrap();

        assert_eq!(registry.resolve(key).unwrap().id, "core.exit");

        let entries = registry.picker_entries();
        let back = entries
            .iter()
            .find(|(entry, _)| entry.id == "picker.back")
            .expect("engine entry is retained");
        assert_eq!(back.1, None);
    }

    /// `unbind.keys` releases the physical key everywhere, so no lower layer
    /// takes the key over: it drops through to raw input handling instead.
    #[test]
    fn unbind_keys_releases_a_key_in_every_layer() {
        let mut registry = CommandRegistry::new();
        let key = Key::Ctrl('k');
        registry
            .replace_layer(
                BindingLayer::Host,
                vec![CommandEntry::new(
                    "host_palette",
                    Some("Commands".into()),
                    Some(key),
                    BindingLayer::Host,
                )],
            )
            .unwrap();
        registry
            .replace_layer(
                BindingLayer::View,
                vec![CommandEntry::new(
                    "core.page",
                    Some("Page".into()),
                    Some(key),
                    BindingLayer::View,
                )],
            )
            .unwrap();

        registry.replace_unbinds(UnbindRules {
            keys: HashSet::from([key.binding_identity()]),
            ..Default::default()
        });

        assert!(registry.resolve(key).is_none());
        assert!(registry.resolve_id("host_palette").is_some());
        assert!(registry.resolve_id("core.page").is_some());

        // Releasing a key never hides a command: both entries survive as keyless
        // rows, so the palette still offers them and neither id is lost.
        let rows = registry.picker_entries();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|(_, bound)| bound.is_none()));
    }

    /// Unbinding one command releases only its key, so a lower layer's binding
    /// for the same physical key takes over.
    #[test]
    fn unbind_commands_lets_a_lower_layer_take_the_key() {
        let mut registry = CommandRegistry::new();
        let key = Key::Ctrl('k');
        registry
            .replace_layer(
                BindingLayer::Host,
                vec![CommandEntry::new(
                    "host_palette",
                    Some("Commands".into()),
                    Some(key),
                    BindingLayer::Host,
                )],
            )
            .unwrap();
        registry
            .replace_layer(
                BindingLayer::View,
                vec![CommandEntry::new(
                    "core.page",
                    Some("Page".into()),
                    Some(key),
                    BindingLayer::View,
                )],
            )
            .unwrap();

        registry.replace_unbinds(UnbindRules {
            commands: HashSet::from(["core.page".to_string()]),
            ..Default::default()
        });

        assert_eq!(registry.resolve(key).unwrap().id, "host_palette");
    }
}
