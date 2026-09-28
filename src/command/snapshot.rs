use super::registry::{BindingLayer, CommandRegistry};
use crate::input::Key;
#[cfg(test)]
use crate::view::{Binding, BindingSet};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedCommand {
    pub(crate) id: String,
    pub(crate) label: Option<String>,
    pub(crate) key: Option<Key>,
    pub(crate) layer: BindingLayer,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ChromeSnapshot {
    pub(crate) revision: u64,
    pub(crate) entries: Vec<ResolvedCommand>,
    pub(crate) active_instance: Option<crate::protocol::contracts::ViewInstanceId>,
    pub(crate) active_view: Option<String>,
    pub(crate) active_parameters: Value,
    pub(crate) active_raw_input: String,
    pub(crate) chrome_commands_show: Option<Vec<String>>,
}

impl ChromeSnapshot {
    pub(crate) fn from_registry(registry: &CommandRegistry) -> Self {
        let entries = registry
            .picker_entries()
            .into_iter()
            .map(|(e, key)| ResolvedCommand {
                id: e.id.clone(),
                label: e.label.clone(),
                key,
                layer: e.layer,
            })
            .collect();
        Self {
            revision: registry.revision(),
            entries,
            active_instance: None,
            active_view: None,
            active_parameters: Value::Null,
            active_raw_input: String::new(),
            chrome_commands_show: None,
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

    /// Resolves the configured physical keys (`chrome_commands_show`) against
    /// the current entries. `entries` is already ordered `View > Engine > Host`
    /// with shadowed keys cleared, so the first match is the winning binding.
    /// Unset or unbound keys produce no hint.
    pub(crate) fn footer_commands(&self) -> Vec<(String, String)> {
        let Some(bindings) = &self.chrome_commands_show else {
            return Vec::new();
        };
        bindings
            .iter()
            .filter_map(|binding| {
                let key = Key::parse_binding(binding).ok()?;
                self.entries
                    .iter()
                    .find(|entry| {
                        entry
                            .key
                            .is_some_and(|bound| bound.binding_identity() == key.binding_identity())
                    })
                    .map(|entry| {
                        (
                            binding.clone(),
                            entry.label.clone().unwrap_or_else(|| entry.id.clone()),
                        )
                    })
            })
            .collect()
    }

    pub(crate) fn with_chrome_commands_show(mut self, bindings: Vec<String>) -> Self {
        self.chrome_commands_show = Some(bindings);
        self
    }

    /// The single runtime command projection: `{revision, commands: [entry]}`.
    ///
    /// `revision` is a property of the snapshot as a whole (every entry shares
    /// it), so it lives on the envelope, never on an entry. Each entry carries
    /// the stable identity, display label, effective key (`null` when unbound,
    /// shadowed, or declared keyless), and the winning layer.
    pub(crate) fn runtime_envelope(&self) -> serde_json::Value {
        let commands = self
            .entries
            .iter()
            .map(|e| {
                json!({
                    "id": e.id,
                    "label": e.label.as_deref().unwrap_or(&e.id),
                    "key": e.key.and_then(|key| key.binding_name()),
                    "layer": e.layer.as_str(),
                })
            })
            .collect::<Vec<_>>();
        json!({ "revision": self.revision, "commands": commands })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::BindingLayer;
    use crate::input::Key;

    #[test]
    fn unset_chrome_commands_show_produces_no_hints() {
        let snapshot = ChromeSnapshot {
            entries: vec![ResolvedCommand {
                id: "open".to_string(),
                label: Some("Open".to_string()),
                key: Some(Key::Enter),
                layer: BindingLayer::View,
            }],
            ..Default::default()
        };
        assert!(snapshot.footer_commands().is_empty());
    }

    #[test]
    fn configured_footer_bindings_resolve_highest_layer_for_a_key() {
        let snapshot = ChromeSnapshot {
            chrome_commands_show: Some(vec!["enter".to_string()]),
            entries: vec![
                ResolvedCommand {
                    id: "view_open".to_string(),
                    label: Some("Open".to_string()),
                    key: Some(Key::Enter),
                    layer: BindingLayer::View,
                },
                ResolvedCommand {
                    id: "engine_submit".to_string(),
                    label: Some("Submit".to_string()),
                    key: Some(Key::Enter),
                    layer: BindingLayer::Engine,
                },
                ResolvedCommand {
                    id: "host_default".to_string(),
                    label: Some("Default".to_string()),
                    key: Some(Key::Enter),
                    layer: BindingLayer::Host,
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            snapshot.footer_commands(),
            vec![("enter".to_string(), "Open".to_string())]
        );
    }

    #[test]
    fn chrome_hint_is_omitted_when_the_entry_exposes_no_key() {
        // An entry with no key (declared keyless, shadowed, or unbound) cannot
        // satisfy `chrome_commands_show`, so it produces no footer hint.
        let snapshot = ChromeSnapshot {
            chrome_commands_show: Some(vec!["ctrl+k".to_string(), "enter".to_string()]),
            entries: vec![
                ResolvedCommand {
                    id: "palette".to_string(),
                    label: Some("Commands".to_string()),
                    key: None,
                    layer: BindingLayer::Host,
                },
                ResolvedCommand {
                    id: "open".to_string(),
                    label: Some("Open".to_string()),
                    key: Some(Key::Enter),
                    layer: BindingLayer::View,
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            snapshot.footer_commands(),
            vec![("enter".to_string(), "Open".to_string())]
        );
    }

    #[test]
    fn configured_footer_bindings_resolve_labels_and_skip_unknown_keys() {
        let snapshot = ChromeSnapshot {
            chrome_commands_show: Some(vec![
                "ctrl+p".to_string(),
                "ctrl+q".to_string(),
                "enter".to_string(),
            ]),
            entries: vec![
                ResolvedCommand {
                    id: "preview".to_string(),
                    label: Some("Preview".to_string()),
                    key: Some(Key::Ctrl('p')),
                    layer: BindingLayer::View,
                },
                ResolvedCommand {
                    id: "open".to_string(),
                    label: Some("Open".to_string()),
                    key: Some(Key::Enter),
                    layer: BindingLayer::View,
                },
            ],
            ..Default::default()
        };
        // `ctrl+q` is declared but bound to nothing, so it produces no hint.
        assert_eq!(
            snapshot.footer_commands(),
            vec![
                ("ctrl+p".to_string(), "Preview".to_string()),
                ("enter".to_string(), "Open".to_string()),
            ]
        );
    }

    #[test]
    fn empty_configured_footer_bindings_disable_hints() {
        let snapshot = ChromeSnapshot {
            chrome_commands_show: Some(Vec::new()),
            entries: vec![ResolvedCommand {
                id: "open".to_string(),
                label: Some("Open".to_string()),
                key: Some(Key::Enter),
                layer: BindingLayer::View,
            }],
            ..Default::default()
        };
        assert!(snapshot.footer_commands().is_empty());
    }

    /// The user-visible end of the multi-key fix: a command bound to two keys
    /// advertises both, instead of one key being dropped by the projection.
    #[test]
    fn footer_hints_cover_every_key_of_one_command() {
        let mut registry = CommandRegistry::new();
        registry
            .replace_layer(
                BindingLayer::Engine,
                vec![
                    crate::command::CommandEntry::for_event(
                        "picker.exit",
                        Some("Exit".to_string()),
                        Some(Key::Ctrl('c')),
                        BindingLayer::Engine,
                    ),
                    crate::command::CommandEntry::for_event(
                        "picker.exit",
                        Some("Exit".to_string()),
                        Some(Key::Ctrl('d')),
                        BindingLayer::Engine,
                    ),
                ],
            )
            .unwrap();

        let snapshot = ChromeSnapshot::from_registry(&registry)
            .with_chrome_commands_show(vec!["ctrl+c".to_string(), "ctrl+d".to_string()]);
        assert_eq!(
            snapshot.footer_commands(),
            vec![
                ("ctrl+c".to_string(), "Exit".to_string()),
                ("ctrl+d".to_string(), "Exit".to_string()),
            ]
        );
    }

    /// Pins the end-to-end unwinding of `unbind` through the registry-derived
    /// snapshot: the physical binding disappears (so Chrome hints stay honest)
    /// while the command remains discoverable and invokable by identity.
    #[test]
    fn registry_unbind_clears_the_key_but_keeps_the_palette_entry() {
        let mut registry = CommandRegistry::new();
        registry
            .replace_layer(
                BindingLayer::Host,
                vec![crate::command::CommandEntry::new(
                    "__commands.palette",
                    Some("Commands".to_string()),
                    Some(Key::Ctrl('k')),
                    BindingLayer::Host,
                )],
            )
            .unwrap();
        registry.replace_unbinds(crate::command::UnbindRules {
            layers: std::collections::HashSet::from([BindingLayer::Host]),
            ..Default::default()
        });

        let snapshot = ChromeSnapshot::from_registry(&registry)
            .with_chrome_commands_show(vec!["ctrl+k".to_string()]);

        // The key is unbound, so no footer hint is produced...
        assert!(snapshot.footer_commands().is_empty());
        // ...but the command stays in the command list without a key.
        let envelope = snapshot.runtime_envelope();
        let commands = envelope["commands"].clone();
        assert_eq!(commands.as_array().unwrap().len(), 1);
        assert_eq!(commands[0]["id"], "__commands.palette");
        assert!(commands[0]["key"].is_null());
    }

    #[test]
    fn runtime_envelope_orders_view_engine_host_with_metadata() {
        let snapshot = ChromeSnapshot {
            revision: 42,
            entries: vec![
                ResolvedCommand {
                    id: "view_select".to_string(),
                    label: Some("Select".to_string()),
                    key: Some(Key::Enter),
                    layer: BindingLayer::View,
                },
                ResolvedCommand {
                    id: "engine_filter".to_string(),
                    label: Some("Filter".to_string()),
                    key: None,
                    layer: BindingLayer::Engine,
                },
                ResolvedCommand {
                    id: "parameters".to_string(),
                    label: Some("Parameters".to_string()),
                    key: Some(Key::Ctrl('g')),
                    layer: BindingLayer::Host,
                },
            ],
            ..Default::default()
        };

        let envelope = snapshot.runtime_envelope();
        // The revision is a single snapshot-wide envelope field.
        assert_eq!(envelope["revision"], 42);
        let commands = envelope["commands"].as_array().unwrap();
        assert_eq!(commands.len(), 3);

        assert_eq!(commands[0]["id"], "view_select");
        assert_eq!(commands[0]["layer"], "view");
        assert_eq!(commands[0]["key"], "enter");

        // A keyless entry exposes `null`, not an empty string masquerading as a key.
        assert_eq!(commands[1]["id"], "engine_filter");
        assert_eq!(commands[1]["layer"], "engine");
        assert!(commands[1]["key"].is_null());

        assert_eq!(commands[2]["id"], "parameters");
        assert_eq!(commands[2]["layer"], "host");
        assert_eq!(commands[2]["key"], "ctrl+g");
    }
}
