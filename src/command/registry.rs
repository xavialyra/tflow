use crate::input::Key;
use crate::view::ViewDecision;
use anyhow::{Result, ensure};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum CommandScope {
    View,
    Engine,
    Host,
}

impl CommandScope {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Engine => "engine",
            Self::Host => "host",
        }
    }
}

pub(crate) trait CommandAction: Send + Sync {
    fn execute(&self) -> Result<ViewDecision>;
}

impl<F> CommandAction for F
where
    F: Fn() -> Result<ViewDecision> + Send + Sync,
{
    fn execute(&self) -> Result<ViewDecision> {
        (self)()
    }
}

#[derive(Clone)]
pub(crate) struct CommandEntry {
    pub(crate) id: String,
    pub(crate) label: Option<String>,
    pub(crate) key: Option<Key>,
    pub(crate) scope: CommandScope,
    pub(crate) action: Arc<dyn CommandAction>,
}

impl CommandEntry {
    pub(crate) fn new(
        id: impl Into<String>,
        label: Option<String>,
        key: Option<Key>,
        scope: CommandScope,
        action: Arc<dyn CommandAction>,
    ) -> Self {
        Self {
            id: id.into(),
            label,
            key,
            scope,
            action,
        }
    }

    pub(crate) fn matches_binding(&self, other_key: Key) -> bool {
        self.key
            .as_ref()
            .is_some_and(|k| k.binding_identity() == other_key.binding_identity())
    }

    pub(crate) fn same_definition(&self, other: &Self) -> bool {
        self.id == other.id
            && self.label == other.label
            && self.key.map(|k| k.binding_identity()) == other.key.map(|k| k.binding_identity())
            && self.scope == other.scope
    }
}

impl std::fmt::Debug for CommandEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandEntry")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("key", &self.key)
            .field("scope", &self.scope)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommandsChanged {
    pub(crate) revision: u64,
}

#[derive(Default)]
pub(crate) struct CommandRegistry {
    revision: u64,
    host_entries: Vec<CommandEntry>,
    engine_entries: Vec<CommandEntry>,
    view_entries: Vec<CommandEntry>,
}

impl CommandRegistry {
    pub(crate) fn new() -> Self {
        Self {
            revision: 0,
            host_entries: Vec::new(),
            engine_entries: Vec::new(),
            view_entries: Vec::new(),
        }
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn scope_entries(&self, scope: CommandScope) -> &[CommandEntry] {
        match scope {
            CommandScope::View => &self.view_entries,
            CommandScope::Engine => &self.engine_entries,
            CommandScope::Host => &self.host_entries,
        }
    }

    /// Atomically replaces all entries in the given scope.
    ///
    /// Validates that no two entries within the scope share the same physical key.
    /// If validation fails, returns an error and leaves the existing scope unchanged.
    /// If the new entries are identical to the committed scope, returns Ok(None).
    /// If entries changed, increments `revision` and returns Ok(Some(CommandsChanged)).
    pub(crate) fn replace_scope(
        &mut self,
        scope: CommandScope,
        entries: Vec<CommandEntry>,
    ) -> Result<Option<CommandsChanged>> {
        let mut seen = HashSet::new();
        for entry in &entries {
            if let Some(key) = entry.key {
                let identity = key.binding_identity();
                ensure!(
                    seen.insert(identity),
                    "scope {:?} contains duplicate key {:?}",
                    scope,
                    key.binding_name()
                );
            }
        }

        let current = self.scope_entries(scope);
        let definition_changed = current.len() != entries.len()
            || !current
                .iter()
                .zip(&entries)
                .all(|(a, b)| a.same_definition(b));

        match scope {
            CommandScope::View => self.view_entries = entries,
            CommandScope::Engine => self.engine_entries = entries,
            CommandScope::Host => self.host_entries = entries,
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
    pub(crate) fn resolve(&self, key: Key) -> Option<&CommandEntry> {
        self.view_entries
            .iter()
            .find(|e| e.matches_binding(key))
            .or_else(|| self.engine_entries.iter().find(|e| e.matches_binding(key)))
            .or_else(|| self.host_entries.iter().find(|e| e.matches_binding(key)))
    }

    /// Resolves an entry by its ID, honoring priority View > Engine > Host.
    pub(crate) fn resolve_id(&self, id: &str) -> Option<&CommandEntry> {
        self.view_entries
            .iter()
            .find(|e| e.id == id)
            .or_else(|| self.engine_entries.iter().find(|e| e.id == id))
            .or_else(|| self.host_entries.iter().find(|e| e.id == id))
    }

    /// Dispatches a command by resolving its ID from the current registry and executing it.
    /// Returns an error if the command ID is no longer present in the registry.
    pub(crate) fn dispatch_id(&self, id: &str) -> Result<ViewDecision> {
        let entry = self
            .resolve_id(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("command {:?} is no longer available", id))?;
        entry.action.execute()
    }

    /// Returns the effective command set at the current revision.
    ///
    /// Deduplicates conflicting key bindings and duplicate IDs by scope priority (View > Engine > Host).
    #[allow(dead_code)]
    pub(crate) fn effective_entries(&self) -> Vec<&CommandEntry> {
        let mut seen_keys = HashSet::new();
        let mut seen_ids = HashSet::new();
        let mut effective = Vec::new();

        let all = self
            .view_entries
            .iter()
            .chain(self.engine_entries.iter())
            .chain(self.host_entries.iter());

        for entry in all {
            if !seen_ids.insert(entry.id.clone()) {
                continue;
            }
            if let Some(key) = entry.key {
                if !seen_keys.insert(key.binding_identity()) {
                    continue;
                }
            }
            effective.push(entry);
        }
        effective
    }

    /// Returns all available command entries, ordered by View > Engine > Host.
    ///
    /// For entries whose key bindings conflict with higher-priority scopes,
    /// the entry is retained but its key binding is cleared (unbound command).
    /// Entries with duplicate IDs from lower-priority scopes are shadowed and skipped.
    pub(crate) fn picker_entries(&self) -> Vec<(&CommandEntry, Option<Key>)> {
        let mut seen_keys = HashSet::new();
        let mut seen_ids = HashSet::new();
        let mut result = Vec::new();

        let all = self
            .view_entries
            .iter()
            .chain(self.engine_entries.iter())
            .chain(self.host_entries.iter());

        for entry in all {
            if !seen_ids.insert(entry.id.clone()) {
                continue;
            }
            let key = match entry.key {
                Some(k) if seen_keys.insert(k.binding_identity()) => Some(k),
                _ => None,
            };
            result.push((entry, key));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop_action() -> Arc<dyn CommandAction> {
        Arc::new(|| Ok(ViewDecision::Stay))
    }

    #[test]
    fn resolution_priority_is_view_then_engine_then_host() {
        let mut registry = CommandRegistry::new();
        let key = Key::Char('a');

        registry
            .replace_scope(
                CommandScope::Host,
                vec![CommandEntry::new(
                    "host_a",
                    Some("Host A".into()),
                    Some(key),
                    CommandScope::Host,
                    noop_action(),
                )],
            )
            .unwrap();

        assert_eq!(registry.resolve(key).unwrap().id, "host_a");

        registry
            .replace_scope(
                CommandScope::Engine,
                vec![CommandEntry::new(
                    "engine_a",
                    Some("Engine A".into()),
                    Some(key),
                    CommandScope::Engine,
                    noop_action(),
                )],
            )
            .unwrap();

        assert_eq!(registry.resolve(key).unwrap().id, "engine_a");

        registry
            .replace_scope(
                CommandScope::View,
                vec![CommandEntry::new(
                    "view_a",
                    Some("View A".into()),
                    Some(key),
                    CommandScope::View,
                    noop_action(),
                )],
            )
            .unwrap();

        assert_eq!(registry.resolve(key).unwrap().id, "view_a");
    }

    #[test]
    fn same_scope_conflict_is_rejected_and_leaves_existing_entries() {
        let mut registry = CommandRegistry::new();
        let key = Key::Char('x');

        let initial = vec![CommandEntry::new(
            "view_x",
            Some("View X".into()),
            Some(key),
            CommandScope::View,
            noop_action(),
        )];
        registry.replace_scope(CommandScope::View, initial).unwrap();
        let rev = registry.revision();

        let conflicting = vec![
            CommandEntry::new(
                "one",
                Some("One".into()),
                Some(key),
                CommandScope::View,
                noop_action(),
            ),
            CommandEntry::new(
                "two",
                Some("Two".into()),
                Some(key),
                CommandScope::View,
                noop_action(),
            ),
        ];

        let result = registry.replace_scope(CommandScope::View, conflicting);
        assert!(result.is_err());
        assert_eq!(registry.revision(), rev);
        assert_eq!(registry.resolve(key).unwrap().id, "view_x");
    }

    #[test]
    fn empty_replacement_clears_scope_and_increments_revision() {
        let mut registry = CommandRegistry::new();
        let key = Key::Char('y');
        registry
            .replace_scope(
                CommandScope::View,
                vec![CommandEntry::new(
                    "view_y",
                    Some("Y".into()),
                    Some(key),
                    CommandScope::View,
                    noop_action(),
                )],
            )
            .unwrap();

        assert!(registry.resolve(key).is_some());
        let rev_before = registry.revision();

        let changed = registry
            .replace_scope(CommandScope::View, Vec::new())
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
            CommandScope::Engine,
            noop_action(),
        )];

        let changed1 = registry
            .replace_scope(CommandScope::Engine, entries.clone())
            .unwrap();
        assert!(changed1.is_some());
        let rev1 = registry.revision();

        let changed2 = registry
            .replace_scope(CommandScope::Engine, entries)
            .unwrap();
        assert!(changed2.is_none());
        assert_eq!(registry.revision(), rev1);
    }

    #[test]
    fn picker_entries_orders_view_engine_host_and_degrades_conflicting_keys() {
        let mut registry = CommandRegistry::new();
        let key_common = Key::Ctrl('p');
        let key_host = Key::Ctrl('g');

        registry
            .replace_scope(
                CommandScope::Host,
                vec![
                    CommandEntry::new(
                        "host_print",
                        Some("Host Print".into()),
                        Some(key_common),
                        CommandScope::Host,
                        noop_action(),
                    ),
                    CommandEntry::new(
                        "parameters",
                        Some("Parameters".into()),
                        Some(key_host),
                        CommandScope::Host,
                        noop_action(),
                    ),
                ],
            )
            .unwrap();

        registry
            .replace_scope(
                CommandScope::Engine,
                vec![CommandEntry::new(
                    "engine_cmd",
                    Some("Engine Cmd".into()),
                    None,
                    CommandScope::Engine,
                    noop_action(),
                )],
            )
            .unwrap();

        registry
            .replace_scope(
                CommandScope::View,
                vec![CommandEntry::new(
                    "view_print",
                    Some("View Print".into()),
                    Some(key_common),
                    CommandScope::View,
                    noop_action(),
                )],
            )
            .unwrap();

        let entries = registry.picker_entries();
        // 顺序应为 View -> Engine -> Host
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].0.id, "view_print");
        assert_eq!(entries[0].0.scope, CommandScope::View);
        assert_eq!(entries[0].1, Some(key_common));

        assert_eq!(entries[1].0.id, "engine_cmd");
        assert_eq!(entries[1].0.scope, CommandScope::Engine);
        assert_eq!(entries[1].1, None);

        // host_print 的 key_common 被 view_print 抢占，降级为 None，但条目保留
        assert_eq!(entries[2].0.id, "host_print");
        assert_eq!(entries[2].0.scope, CommandScope::Host);
        assert_eq!(entries[2].1, None);

        assert_eq!(entries[3].0.id, "parameters");
        assert_eq!(entries[3].0.scope, CommandScope::Host);
        assert_eq!(entries[3].1, Some(key_host));
    }
}
