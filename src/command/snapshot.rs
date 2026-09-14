use super::registry::{CommandRegistry, CommandScope};
use crate::input::Key;
use crate::view::{Binding, BindingSet};
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedCommand {
    pub(crate) id: String,
    pub(crate) label: Option<String>,
    pub(crate) key: Option<Key>,
    pub(crate) scope: CommandScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ChromeSnapshot {
    pub(crate) revision: u64,
    pub(crate) entries: Vec<ResolvedCommand>,
    pub(crate) active_instance: Option<crate::protocol::contracts::ViewInstanceId>,
}

impl ChromeSnapshot {
    pub(crate) fn from_registry(registry: &CommandRegistry) -> Self {
        let entries = registry
            .effective_entries()
            .into_iter()
            .map(|e| ResolvedCommand {
                id: e.id.clone(),
                label: e.label.clone(),
                key: e.key,
                scope: e.scope,
            })
            .collect();
        Self {
            revision: registry.revision(),
            entries,
            active_instance: None,
        }
    }

    pub(crate) fn with_active_instance(
        mut self,
        instance: Option<crate::protocol::contracts::ViewInstanceId>,
    ) -> Self {
        self.active_instance = instance;
        self
    }

    pub(crate) fn to_binding_set(&self) -> BindingSet {
        let mut seen = std::collections::HashSet::new();
        let bindings = self
            .entries
            .iter()
            .filter_map(|e| {
                let key = e.key?;
                if !seen.insert(key.binding_identity()) {
                    return None;
                }
                Some(Binding {
                    key,
                    label: e.label.clone(),
                })
            })
            .collect::<Vec<_>>();
        BindingSet::new(bindings)
    }

    pub(crate) fn overflow_command(&self) -> Option<(String, String)> {
        self.entries
            .iter()
            .find(|e| e.scope == CommandScope::Host && e.id == "commands")
            .and_then(|e| {
                let key_name = e.key.and_then(|k| k.binding_name())?;
                let label = e.label.clone().unwrap_or_else(|| "Commands".to_string());
                Some((key_name, label))
            })
    }

    pub(crate) fn has_unbound(&self) -> bool {
        self.entries.iter().any(|e| e.key.is_none())
    }

    pub(crate) fn to_picker_parameters(&self) -> serde_json::Value {
        let commands = self
            .entries
            .iter()
            .map(|e| {
                let key_name = e.key.and_then(|k| k.binding_name()).unwrap_or_default();
                let label = e.label.as_deref().unwrap_or(&e.id);
                let scope_name = e.scope.as_str();
                json!({
                    "ref": {
                        "view": scope_name,
                        "id": e.id,
                        "revision": self.revision,
                    },
                    "label": label,
                    "key": key_name,
                    "owner": scope_name,
                })
            })
            .collect::<Vec<_>>();
        json!({ "commands": commands })
    }
}
