use crate::cancellation::CancellationToken;
use crate::config::{Config, ConfigReadContext, ConfigScope};
use crate::engine::{TaskHandle, TaskScheduler};
use crate::state::StateInstance;
use crate::text::sanitize_text;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct ItemValue {
    label: String,
    #[serde(default)]
    allow_empty: bool,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    metadata: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct Item {
    pub(crate) prefix: String,
    pub(crate) text: String,
    pub(crate) value: Option<String>,
    pub(crate) metadata: Value,
    pub(crate) source_view: String,
}

#[derive(Debug, Default)]
pub(crate) struct ItemsResult {
    pub(crate) items: Vec<Item>,
    pub(crate) errors: Vec<String>,
}

pub(crate) struct ItemsRequest {
    pub(crate) view: String,
    pub(crate) input: String,
    pub(crate) query: String,
    pub(crate) source_states: BTreeMap<String, StateInstance>,
}

pub(crate) struct ItemsResponse {
    pub(crate) view: String,
    pub(crate) input: String,
    pub(crate) query: String,
    pub(crate) result: std::result::Result<ItemsResult, String>,
}

pub(crate) struct ItemsEvent {
    pub(crate) current: bool,
    pub(crate) view: String,
    pub(crate) errors: Vec<String>,
    pub(crate) failure: Option<String>,
    pub(crate) pending_command: Option<crate::input::Key>,
}

pub(crate) type ItemsTaskHandle = TaskHandle<ItemsResponse>;

pub(crate) fn submit_items_task(
    tasks: &TaskScheduler,
    config: &Arc<Config>,
    request: ItemsRequest,
) -> ItemsTaskHandle {
    let config = Arc::clone(config);
    tasks.submit_keyed(
        request,
        "picker-items".to_string(),
        move |request, runtime_value, cancellation| {
            let result = load_items_with_states(
                &config,
                &request.view,
                &request.source_states,
                &runtime_value,
                &cancellation,
            )
            .map_err(|error| error.to_string());
            ItemsResponse {
                view: request.view,
                input: request.input,
                query: request.query,
                result,
            }
        },
    )
}

fn load_items_with_states(
    config: &Config,
    view_ref: &str,
    source_states: &BTreeMap<String, StateInstance>,
    runtime: &Value,
    cancellation: &CancellationToken,
) -> Result<ItemsResult> {
    let mut result = ItemsResult::default();

    for (source_ref, view) in config.source_views(view_ref)? {
        let prefix = view.alias.clone().unwrap_or_else(|| source_ref.clone());
        if view.items.is_none() {
            continue;
        }

        let fallback_state = StateInstance::empty(&source_ref);
        let state = source_states.get(&source_ref).unwrap_or(&fallback_state);
        let value = match config.get(
            ConfigReadContext {
                scope: ConfigScope::View(state),
                runtime,
                input: &config.input_value,
                cancellation: Some(cancellation.clone()),
            },
            &["items"],
        ) {
            Ok(Some(value)) => value,
            Ok(None) => continue,
            Err(error) => {
                result.errors.push(format!("{}: {}", source_ref, error));
                continue;
            }
        };
        append_items(&mut result, &source_ref, &prefix, value);
    }

    Ok(result)
}

#[cfg(test)]
fn load_items(
    config: &Config,
    view_ref: &str,
    runtime: &Value,
    cancellation: &CancellationToken,
) -> Result<ItemsResult> {
    load_items_with_states(config, view_ref, &BTreeMap::new(), runtime, cancellation)
}

fn append_items(result: &mut ItemsResult, source_ref: &str, prefix: &str, value: Value) {
    let Some(items) = value.as_array() else {
        result.errors.push(format!(
            "{}: items expression must return a JSON array",
            source_ref
        ));
        return;
    };
    let mut parsed_items = Vec::with_capacity(items.len());
    for (index, value) in items.iter().enumerate() {
        let parsed = match serde_json::from_value::<ItemValue>(value.clone()) {
            Ok(item) => item,
            Err(error) => {
                result.errors.push(format!(
                    "{}: invalid items JSON at index {}: {}",
                    source_ref, index, error
                ));
                return;
            }
        };
        let text = sanitize_text(&parsed.label);
        if text.is_empty() && !parsed.allow_empty {
            result.errors.push(format!(
                "{}: items JSON at index {} has an empty label",
                source_ref, index
            ));
            return;
        }
        parsed_items.push(Item {
            prefix: prefix.to_string(),
            text,
            value: parsed.value,
            metadata: parsed.metadata,
            source_view: source_ref.to_string(),
        });
    }
    result.items.extend(parsed_items);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Command, CommandAction, Defaults, ENGINE_PICKER, PluginMetadata, View};
    use std::collections::BTreeMap;
    use std::env;
    use std::fs;

    fn test_config() -> Config {
        let mut views = BTreeMap::new();
        views.insert(
            "core:default".to_string(),
            View {
                engine_type: ENGINE_PICKER.to_string(),
                sources: vec!["apps:main".to_string()],
                alias: None,
                items: None,
                run_shell: None,
                cancel_exit_code: None,
                query: None,
                commands: BTreeMap::new(),
                engine_config: toml::Table::new(),
            },
        );
        views.insert(
            "apps:main".to_string(),
            View {
                engine_type: ENGINE_PICKER.to_string(),
                sources: Vec::new(),
                alias: Some("app".to_string()),
                items: Some("{{ runtime:view.active.items }}".to_string()),
                run_shell: None,
                cancel_exit_code: None,
                query: None,
                commands: BTreeMap::from([(
                    "open".to_string(),
                    Command {
                        key: "enter".to_string(),
                        label: "Open".to_string(),
                        action: CommandAction::Run {
                            payload: crate::config::RunPayload {
                                handler: ":".to_string(),
                                shell: None,
                                exit: false,
                            },
                        },
                    },
                )]),
                engine_config: toml::Table::new(),
            },
        );
        Config {
            default_view: "core:default".to_string(),
            command_view: "core:command".to_string(),
            views,
            plugins: BTreeMap::from([
                (
                    "core".to_string(),
                    PluginMetadata {
                        name: "core".to_string(),
                    },
                ),
                (
                    "apps".to_string(),
                    PluginMetadata {
                        name: "applications".to_string(),
                    },
                ),
            ]),
            defaults: Defaults::default(),
            plugin_roots: BTreeMap::new(),
            config_value: Value::Object(serde_json::Map::new()),
            input_value: Value::Null,
            state_registry: crate::state::StateRegistry::default(),
            invocation_state: crate::state::StateInstance::default(),
        }
    }

    #[test]
    fn parses_structured_items() {
        let item: ItemValue = serde_json::from_str(
            r#"{"label":"Termius","value":"termius.desktop","metadata":{"kind":"app"}}"#,
        )
        .unwrap();
        assert_eq!(item.label, "Termius");
        assert_eq!(item.value.as_deref(), Some("termius.desktop"));
        assert_eq!(item.metadata["kind"], "app");
    }

    #[test]
    fn items_keep_their_source_view() {
        let result = load_items(
            &test_config(),
            "core:default",
            &serde_json::json!({
                "view": {
                    "active": {
                        "items": [{"label": "Second"}, {"label": "First"}]
                    }
                }
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(
            result
                .items
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            ["Second", "First"]
        );
        assert_eq!(result.items[0].source_view, "apps:main");
        assert_eq!(result.items[0].prefix, "app");
    }

    #[test]
    fn item_expressions_require_json_arrays() {
        let mut config = test_config();
        config.views.get_mut("apps:main").unwrap().items =
            Some("{{ runtime:view.active.query }}".to_string());
        let result = load_items(
            &config,
            "core:default",
            &serde_json::json!({
                "view": {"active": {"query": "not-an-array"}}
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(result.items.is_empty());
        assert!(result.errors[0].contains("must return a JSON array"));
    }

    #[test]
    fn invalid_items_are_reported_without_partial_results() {
        let result = load_items(
            &test_config(),
            "core:default",
            &serde_json::json!({
                "view": {
                    "active": {
                        "items": [{"label": "Valid"}, {"value": "missing-label"}]
                    }
                }
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(result.items.is_empty());
        assert!(result.errors[0].contains("index 1"));
    }

    #[test]
    fn item_expressions_pass_runtime_input_to_scripts() {
        let root =
            env::temp_dir().join(format!("tui-launcher-items-script-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("items.sh"),
            "input=$(cat)\nprintf '[{\\\"label\\\":%s}]\\n' \"$input\"\n",
        )
        .unwrap();

        let mut config = test_config();
        config.views.get_mut("apps:main").unwrap().items =
            Some("{{ script(\"items.sh\", runtime:view.active.query) }}".to_string());
        config.plugin_roots.insert("apps".to_string(), root.clone());
        let result = load_items(
            &config,
            "core:default",
            &serde_json::json!({
                "view": {"active": {"query": "fire"}}
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(result.items[0].text, "fire");
        fs::remove_dir_all(root).unwrap();
    }
}
