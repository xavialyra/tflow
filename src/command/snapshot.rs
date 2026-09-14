use super::registry::{CommandRegistry, CommandScope};
use crate::input::Key;
#[cfg(test)]
use crate::view::{Binding, BindingSet};
use serde_json::{Value, json};

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
    pub(crate) active_view: Option<String>,
    pub(crate) active_parameters: Value,
    pub(crate) active_raw_input: String,
}

impl ChromeSnapshot {
    pub(crate) fn from_registry(registry: &CommandRegistry) -> Self {
        let entries = registry
            .effective_entries()
            .into_iter()
            .filter(|e| !(e.scope == CommandScope::Host && e.id == "parameters"))
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
            active_view: None,
            active_parameters: Value::Null,
            active_raw_input: String::new(),
        }
    }

    pub(crate) fn with_active_instance(
        mut self,
        instance: Option<crate::protocol::contracts::ViewInstanceId>,
    ) -> Self {
        self.active_instance = instance;
        self
    }

    pub(crate) fn with_active_view(
        mut self,
        view: Option<String>,
        parameters: Value,
        raw_input: impl Into<String>,
    ) -> Self {
        self.active_view = view;
        self.active_parameters = parameters;
        self.active_raw_input = raw_input.into();
        self
    }

    #[cfg(test)]
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

    pub(crate) fn footer_commands(&self) -> Vec<(String, String)> {
        let view_entries = self
            .entries
            .iter()
            .filter(|e| e.scope == CommandScope::View)
            .collect::<Vec<_>>();

        let enter_entry = view_entries
            .iter()
            .find(|e| e.key.and_then(|k| k.binding_name()).as_deref() == Some("enter"));

        let has_enter = enter_entry.is_some();
        let has_more_view_commands = if has_enter {
            view_entries.len() > 1
        } else {
            !view_entries.is_empty()
        };

        let mut commands = Vec::new();
        if let Some(enter) = enter_entry {
            let label = enter.label.clone().unwrap_or_else(|| "Enter".to_string());
            commands.push(("enter".to_string(), label));
        }
        if has_more_view_commands {
            if let Some(overflow) = self.overflow_command() {
                commands.push(overflow);
            }
        }
        commands
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

    pub(crate) fn to_picker_parameters(&self) -> serde_json::Value {
        let commands = self
            .entries
            .iter()
            .filter(|e| e.scope == CommandScope::View)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandScope;
    use crate::input::Key;

    fn host_commands_entry() -> ResolvedCommand {
        ResolvedCommand {
            id: "commands".to_string(),
            label: Some("Commands".to_string()),
            key: Some(Key::Ctrl('k')),
            scope: CommandScope::Host,
        }
    }

    #[test]
    fn footer_commands_shows_only_enter_when_only_enter_exists() {
        let snapshot = ChromeSnapshot {
            entries: vec![
                host_commands_entry(),
                ResolvedCommand {
                    id: "open".to_string(),
                    label: Some("Open".to_string()),
                    key: Some(Key::Enter),
                    scope: CommandScope::View,
                },
            ],
            ..Default::default()
        };
        let commands = snapshot.footer_commands();
        assert_eq!(commands, vec![("enter".to_string(), "Open".to_string())]);
    }

    #[test]
    fn footer_commands_shows_enter_and_commands_when_more_commands_exist() {
        let snapshot = ChromeSnapshot {
            entries: vec![
                host_commands_entry(),
                ResolvedCommand {
                    id: "open".to_string(),
                    label: Some("Open".to_string()),
                    key: Some(Key::Enter),
                    scope: CommandScope::View,
                },
                ResolvedCommand {
                    id: "preview".to_string(),
                    label: Some("Preview".to_string()),
                    key: Some(Key::Ctrl('p')),
                    scope: CommandScope::View,
                },
            ],
            ..Default::default()
        };
        let commands = snapshot.footer_commands();
        assert_eq!(
            commands,
            vec![
                ("enter".to_string(), "Open".to_string()),
                ("ctrl+k".to_string(), "Commands".to_string()),
            ]
        );
    }

    #[test]
    fn footer_commands_shows_only_commands_when_no_enter_but_other_commands_exist() {
        let snapshot = ChromeSnapshot {
            entries: vec![
                host_commands_entry(),
                ResolvedCommand {
                    id: "inspect".to_string(),
                    label: Some("Inspect".to_string()),
                    key: Some(Key::Char(' ')),
                    scope: CommandScope::View,
                },
            ],
            ..Default::default()
        };
        let commands = snapshot.footer_commands();
        assert_eq!(
            commands,
            vec![("ctrl+k".to_string(), "Commands".to_string())]
        );
    }

    #[test]
    fn footer_commands_shows_nothing_when_no_view_commands() {
        let snapshot = ChromeSnapshot {
            entries: vec![host_commands_entry()],
            ..Default::default()
        };
        let commands = snapshot.footer_commands();
        assert!(commands.is_empty());
    }
}
