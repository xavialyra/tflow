use crate::cancellation::CancellationToken;
use crate::config::{Config, View};
use crate::engine::{RuntimeHandle, TaskHandle, TaskScheduler};
use crate::expression::ExpressionMethods;
use crate::text::sanitize_text;
use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct ItemValue {
    label: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    metadata: Value,
}

#[derive(Debug, Clone)]
pub(crate) struct Item {
    pub(crate) prefix: String,
    pub(crate) text: String,
    pub(crate) value: Option<String>,
    pub(crate) metadata: Value,
    pub(crate) source_view: String,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ItemsResult {
    pub(crate) items: Vec<Item>,
    pub(crate) errors: Vec<String>,
}

pub(crate) struct ItemsRequest {
    pub(crate) view: String,
    pub(crate) input: String,
}

#[derive(Clone)]
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
    pub(crate) pending_command: Option<super::super::Key>,
}

pub(crate) type ItemsTaskScheduler = TaskScheduler<ItemsRequest, ItemsResponse, String>;
pub(crate) type ItemsTaskHandle = TaskHandle<ItemsRequest, ItemsResponse, String>;

pub(crate) fn spawn_items_scheduler(config: Config, runtime: RuntimeHandle) -> ItemsTaskScheduler {
    TaskScheduler::spawn(
        runtime,
        move |request: ItemsRequest, runtime_value, cancellation| {
            let (source_prefix, query) = config.resolve_view_prefix(&request.view, &request.input);
            let result = load_items(
                &config,
                &request.view,
                source_prefix.as_deref(),
                &runtime_value,
                &cancellation,
            )
            .map_err(|error| error.to_string());
            ItemsResponse {
                view: request.view,
                input: request.input,
                query,
                result,
            }
        },
    )
}

fn load_items(
    config: &Config,
    view_ref: &str,
    source_prefix: Option<&str>,
    runtime: &Value,
    cancellation: &CancellationToken,
) -> Result<ItemsResult> {
    let mut result = ItemsResult::default();

    for (source_ref, view) in config.source_views(view_ref)? {
        let display_prefix = display_prefix(&source_ref, view);
        if source_prefix.is_some_and(|prefix| prefix != display_prefix) {
            continue;
        }
        if view.items.is_none() {
            continue;
        }

        let root = config
            .plugin_root(&source_ref)
            .unwrap_or_else(|| Path::new("."));
        let mut methods = ExpressionMethods::with_cancellation(root, cancellation.clone());
        let value = match config.evaluate_view_items(&source_ref, runtime, &mut methods) {
            Ok(Some(value)) => value,
            Ok(None) => continue,
            Err(error) => {
                result.errors.push(format!("{}: {}", source_ref, error));
                continue;
            }
        };
        append_items(&mut result, &source_ref, &display_prefix, value);
    }

    Ok(result)
}

fn append_items(result: &mut ItemsResult, source_ref: &str, display_prefix: &str, value: Value) {
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
        if text.is_empty() {
            result.errors.push(format!(
                "{}: items JSON at index {} has an empty label",
                source_ref, index
            ));
            return;
        }
        parsed_items.push(Item {
            prefix: display_prefix.to_string(),
            text,
            value: parsed.value,
            metadata: parsed.metadata,
            source_view: source_ref.to_string(),
        });
    }
    result.items.extend(parsed_items);
}

fn display_prefix(source_ref: &str, view: &View) -> String {
    view.display_prefix.clone().unwrap_or_else(|| {
        source_ref
            .split_once(':')
            .map(|(_, view_name)| view_name.to_string())
            .unwrap_or_else(|| source_ref.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        Command, DisplayType, ENGINE_LAUNCHER, EngineDefinition, ViewTypeDefinition,
    };
    use std::collections::BTreeMap;
    use std::env;
    use std::fs;

    fn test_config() -> Config {
        let mut views = BTreeMap::new();
        views.insert(
            "core:default".to_string(),
            View {
                view_type: ENGINE_LAUNCHER.to_string(),
                display: DisplayType::Text,
                sources: vec!["apps:main".to_string()],
                display_prefix: None,
                items: None,
                run_shell: None,
                commands: BTreeMap::new(),
            },
        );
        views.insert(
            "apps:main".to_string(),
            View {
                view_type: ENGINE_LAUNCHER.to_string(),
                display: DisplayType::Text,
                sources: Vec::new(),
                display_prefix: Some("app".to_string()),
                items: Some("{{ runtime:view.current.items }}".to_string()),
                run_shell: None,
                commands: BTreeMap::from([(
                    "open".to_string(),
                    Command {
                        key: "enter".to_string(),
                        label: "Open".to_string(),
                        run: None,
                        shell: None,
                        view: None,
                        exit: false,
                    },
                )]),
            },
        );
        Config {
            default_view: "core:default".to_string(),
            dmenu_view: "core:dmenu".to_string(),
            command_view: "core:command".to_string(),
            views,
            viewtypes: BTreeMap::from([(
                ENGINE_LAUNCHER.to_string(),
                ViewTypeDefinition {
                    engine: EngineDefinition {
                        engine_type: ENGINE_LAUNCHER.to_string(),
                        config: toml::Table::new(),
                    },
                },
            )]),
            plugin_roots: BTreeMap::new(),
            config_value: Value::Object(serde_json::Map::new()),
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
            None,
            &serde_json::json!({
                "view": {
                    "current": {
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
            Some("{{ runtime:view.current.query }}".to_string());
        let result = load_items(
            &config,
            "core:default",
            None,
            &serde_json::json!({
                "view": {"current": {"query": "not-an-array"}}
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
            None,
            &serde_json::json!({
                "view": {
                    "current": {
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
            Some("{{ script(\"items.sh\", runtime:view.current.query) }}".to_string());
        config.plugin_roots.insert("apps".to_string(), root.clone());
        let result = load_items(
            &config,
            "core:default",
            None,
            &serde_json::json!({
                "view": {"current": {"query": "fire"}}
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(result.items[0].text, "fire");
        fs::remove_dir_all(root).unwrap();
    }
}
