use crate::config::{Config, Rule, View};
use crate::engine::RuntimeHandle;
use crate::expression::ExpressionMethods;
use crate::text::{matches_query, sanitize_text};
use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender};

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

#[derive(Debug, Default)]
pub(crate) struct ItemsResult {
    pub(crate) items: Vec<Item>,
    pub(crate) errors: Vec<String>,
}

pub(crate) struct ItemsRequest {
    pub(crate) id: u64,
    pub(crate) view: String,
    pub(crate) input: String,
}

pub(crate) struct ItemsResponse {
    pub(crate) id: u64,
    pub(crate) view: String,
    pub(crate) input: String,
    pub(crate) active_rule: String,
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

pub(crate) fn items_result_error(
    result: &std::result::Result<ItemsResult, String>,
) -> (Vec<String>, Option<String>) {
    match result {
        Ok(result) => (result.errors.clone(), None),
        Err(error) => (Vec::new(), Some(error.clone())),
    }
}

pub(crate) fn items_worker(
    config: Config,
    runtime: RuntimeHandle,
    requests: Receiver<ItemsRequest>,
    responses: Sender<ItemsResponse>,
) {
    while let Ok(mut request) = requests.recv() {
        while let Ok(next_request) = requests.try_recv() {
            request = next_request;
        }

        let (source_prefix, query) = config.resolve_view_prefix(&request.view, &request.input);
        let (rule_name, rule, rule_query) = config.resolve_rule(&query);
        let runtime_value = runtime.read();
        let result = load_items(
            &config,
            &request.view,
            rule,
            &rule_query,
            source_prefix.as_deref(),
            &runtime_value,
        )
        .map_err(|error| error.to_string());
        let response = ItemsResponse {
            id: request.id,
            view: request.view,
            input: request.input,
            active_rule: rule_name.to_string(),
            query: rule_query,
            result,
        };
        if responses.send(response).is_err() {
            break;
        }
    }
}

fn load_items(
    config: &Config,
    view_ref: &str,
    rule: &Rule,
    query: &str,
    source_prefix: Option<&str>,
    runtime: &Value,
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
        let mut methods = ExpressionMethods::new(root);
        let value = match config.evaluate_view_items(&source_ref, runtime, &mut methods) {
            Ok(Some(value)) => value,
            Ok(None) => continue,
            Err(error) => {
                result.errors.push(format!("{}: {}", source_ref, error));
                continue;
            }
        };
        append_items(
            &mut result,
            &source_ref,
            view,
            &display_prefix,
            rule,
            query,
            value,
        );
    }

    Ok(result)
}

fn append_items(
    result: &mut ItemsResult,
    source_ref: &str,
    view: &View,
    display_prefix: &str,
    rule: &Rule,
    query: &str,
    value: Value,
) {
    let Some(items) = value.as_array() else {
        result.errors.push(format!(
            "{}: items expression must return a JSON array",
            source_ref
        ));
        return;
    };
    for (index, value) in items.iter().enumerate() {
        let parsed = match serde_json::from_value::<ItemValue>(value.clone()) {
            Ok(item) => item,
            Err(error) => {
                result.errors.push(format!(
                    "{}: invalid items JSON at index {}: {}",
                    source_ref, index, error
                ));
                continue;
            }
        };
        let text = sanitize_text(&parsed.label);
        if text.is_empty() {
            continue;
        }
        if rule.filter && view.filter && !matches_query(&text, query) {
            continue;
        }
        result.items.push(Item {
            prefix: display_prefix.to_string(),
            text,
            value: parsed.value,
            metadata: parsed.metadata,
            source_view: source_ref.to_string(),
        });
    }
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
                filter: true,
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
                filter: true,
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
            default_rule: "default".to_string(),
            rules: BTreeMap::from([("default".to_string(), Rule { filter: true })]),
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
            &Rule { filter: true },
            "",
            None,
            &serde_json::json!({
                "view": {"current": {"items": [{"label": "Termius"}]}}
            }),
        )
        .unwrap();
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
            &Rule { filter: true },
            "",
            None,
            &serde_json::json!({
                "view": {"current": {"query": "not-an-array"}}
            }),
        )
        .unwrap();
        assert!(result.items.is_empty());
        assert!(result.errors[0].contains("must return a JSON array"));
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
            &Rule { filter: true },
            "fire",
            None,
            &serde_json::json!({
                "view": {"current": {"query": "fire"}}
            }),
        )
        .unwrap();
        assert_eq!(result.items[0].text, "fire");
        fs::remove_dir_all(root).unwrap();
    }
}
