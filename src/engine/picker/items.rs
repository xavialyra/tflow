use super::PendingAction;
use crate::cancellation::CancellationToken;
use crate::config::{
    Config, EvaluationSnapshot, InvocationScope, OwnerViewScope, ResolvedScriptSource, SessionScope,
};
use crate::script_runner::{ensure_script_success, run_script};
use crate::state::StateInstance;
use crate::text::sanitize_text;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const MAX_ITEMS_PER_SESSION: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FeedId(pub(crate) String);

#[derive(Debug, Clone)]
pub(crate) struct FeedContext {
    pub(crate) owner_view: String,
    pub(crate) state: StateInstance,
    pub(crate) binding_raw: String,
}

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

#[derive(Debug, Clone)]
pub(crate) struct Item {
    pub(crate) prefix: String,
    pub(crate) text: String,
    pub(crate) value: Option<String>,
    pub(crate) metadata: Value,
    /// Stable provenance only; the response-level context owns state and raw binding.
    pub(crate) source_view: String,
    pub(crate) feed_id: FeedId,
}

#[derive(Debug, Default)]
pub(crate) struct ItemsResult {
    pub(crate) items: Vec<Item>,
    pub(crate) contexts: BTreeMap<FeedId, FeedContext>,
    pub(crate) errors: Vec<String>,
}

pub(crate) struct ItemsRequest {
    pub(crate) view: String,
    /// Complete request identity prevents accepting a result for an older state snapshot.
    pub(crate) generation: u64,
    /// Chrome raw buffer used for stale-result matching.
    pub(crate) input: String,
    /// Coordinating page committed input used as the feed binding raw.
    pub(crate) binding_raw: String,
    /// Coordinating page state for single-source pickers; ignored for feeds pages.
    pub(crate) page_state: StateInstance,
}

pub(crate) struct ItemsResponse {
    pub(crate) view: String,
    pub(crate) generation: u64,
    pub(crate) input: String,
    pub(crate) query: String,
    pub(crate) result: std::result::Result<ItemsResult, String>,
}

pub(crate) struct ItemsEvent {
    pub(crate) view: String,
    pub(crate) errors: Vec<String>,
    pub(crate) failure: Option<String>,
    pub(crate) pending_action: Option<PendingAction>,
}

pub(crate) type ItemsTaskHandle = crate::engine::TaskHandle;

pub(crate) fn load_items_for_page(
    config: &Config,
    view_ref: &str,
    page_state: &StateInstance,
    binding_raw: &str,
    runtime: &Value,
    cancellation: &CancellationToken,
) -> Result<ItemsResult> {
    let mut result = ItemsResult::default();

    for (owner_ref, view) in config.feed_views(view_ref)? {
        if cancellation.is_cancelled() {
            return Ok(result);
        }
        if result.items.len() >= MAX_ITEMS_PER_SESSION {
            result.errors.push(format!(
                "items exceeded the session limit of {}",
                MAX_ITEMS_PER_SESSION
            ));
            break;
        }
        let prefix = view.alias.clone().unwrap_or_else(|| owner_ref.clone());
        if view.selected_items().is_none() {
            continue;
        }

        // Each provider gets exactly one typed state and one raw binding snapshot.
        let (state, this_binding_raw) = if owner_ref == page_state.view_ref() {
            (page_state.clone(), Some(binding_raw))
        } else {
            match config.ephemeral_feed_state(&owner_ref, binding_raw) {
                Ok(state) => (state, Some(binding_raw)),
                Err(error) => {
                    result.errors.push(format!("{}: {}", owner_ref, error));
                    continue;
                }
            }
        };
        if let Err(error) = config.validate_query_state(&state) {
            result.errors.push(format!("{}: {}", owner_ref, error));
            continue;
        }
        let feed_id = FeedId(owner_ref.clone());
        result.contexts.insert(
            feed_id.clone(),
            FeedContext {
                owner_view: owner_ref.clone(),
                state: state.clone(),
                binding_raw: binding_raw.to_string(),
            },
        );
        let owner_scope = OwnerViewScope::new(&state).with_binding_raw(this_binding_raw);
        let snapshot = EvaluationSnapshot::new(
            InvocationScope::new(&config.input_value),
            SessionScope::new(runtime),
            Some(owner_scope),
            Some(cancellation),
        );
        let value = match config.items_value(&owner_ref, &snapshot) {
            Ok(Some(value)) if ResolvedScriptSource::is_candidate(&value) => {
                ResolvedScriptSource::parse(&value)
                    .and_then(|source| run_items_source(config, &owner_ref, &source, cancellation))
                    .map(Some)
            }
            value => value,
        };
        let value = match value {
            Ok(Some(value)) => value,
            Ok(None) => continue,
            Err(error) => {
                result.errors.push(format!("{}: {}", owner_ref, error));
                continue;
            }
        };
        append_items_value(
            &mut result,
            &owner_ref,
            &feed_id,
            &prefix,
            value,
            cancellation,
        );
        if cancellation.is_cancelled() {
            return Ok(result);
        }
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
    let mut config = config.clone();
    config.test_rebuild_compiled()?;
    let page_state = config.instantiate_state(view_ref)?;
    load_items_for_page(&config, view_ref, &page_state, "", runtime, cancellation)
}

fn append_items_value(
    result: &mut ItemsResult,
    source_ref: &str,
    feed_id: &FeedId,
    prefix: &str,
    value: Value,
    cancellation: &CancellationToken,
) {
    if !value.is_array() {
        result.errors.push(format!(
            "{}: items must resolve to an array or a script source, got {}",
            source_ref,
            value_type(&value)
        ));
        return;
    }
    append_items_array(result, source_ref, feed_id, prefix, value, cancellation);
}

fn append_items_array(
    result: &mut ItemsResult,
    source_ref: &str,
    feed_id: &FeedId,
    prefix: &str,
    value: Value,
    cancellation: &CancellationToken,
) {
    let Value::Array(items) = value else {
        result.errors.push(format!(
            "{}: items source must produce a JSON array",
            source_ref
        ));
        return;
    };
    let remaining = MAX_ITEMS_PER_SESSION.saturating_sub(result.items.len());
    if items.len() > remaining {
        result.errors.push(format!(
            "{}: items exceeded the session limit of {}",
            source_ref, MAX_ITEMS_PER_SESSION
        ));
        return;
    }
    let mut parsed_items = Vec::with_capacity(items.len());
    for (index, value) in items.into_iter().enumerate() {
        if cancellation.is_cancelled() {
            return;
        }
        let parsed = match serde_json::from_value::<ItemValue>(value) {
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
            feed_id: feed_id.clone(),
        });
    }
    result.items.extend(parsed_items);
}

#[cfg(test)]
fn append_items(
    result: &mut ItemsResult,
    source_ref: &str,
    feed_id: &FeedId,
    prefix: &str,
    value: Value,
    cancellation: &CancellationToken,
) {
    append_items_array(result, source_ref, feed_id, prefix, value, cancellation);
}

fn run_items_source(
    config: &Config,
    source_ref: &str,
    source: &ResolvedScriptSource,
    cancellation: &CancellationToken,
) -> Result<Value> {
    let root = config
        .plugin_root(source_ref)
        .with_context(|| format!("items source {:?} has no plugin root", source_ref))?;
    let args = source.script_args("picker script args")?;
    let output = run_script(
        root,
        &source.file,
        &args,
        source.max_output_bytes,
        cancellation,
    )?;
    ensure_script_success(&output)?;
    if output.stdout.is_empty() {
        bail!("script produced no JSON output");
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("script {} did not produce valid JSON", source.file))
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        Command, CommandAction, ENGINE_PICKER, EngineOptions, EngineSpec, FeedSpec, PluginMetadata,
        View,
    };
    use std::collections::BTreeMap;
    use std::env;
    use std::fs;

    fn script_source(file: &str, args: Option<toml::Value>) -> toml::Value {
        let mut source = toml::Table::new();
        source.insert("source".to_string(), "script".into());
        source.insert("file".to_string(), file.into());
        if let Some(args) = args {
            source.insert("args".to_string(), args);
        }
        toml::Value::Table(source)
    }

    fn test_config() -> Config {
        let mut views = BTreeMap::new();
        views.insert(
            "core:default".to_string(),
            View {
                engine: EngineSpec {
                    engine_type: ENGINE_PICKER.to_string(),
                    config: EngineOptions {
                        feeds: vec![FeedSpec {
                            view: "apps:main".to_string(),
                        }],
                        ..Default::default()
                    },
                },
                alias: None,
                run_shell: None,
                cancel_exit_code: None,
                query: None,
                keymap: None,
                commands: BTreeMap::new(),
            },
        );
        views.insert(
            "apps:main".to_string(),
            View {
                engine: EngineSpec {
                    engine_type: ENGINE_PICKER.to_string(),
                    config: EngineOptions {
                        items: Some("{{ page.items }}".into()),
                        ..Default::default()
                    },
                },
                alias: Some("app".to_string()),
                run_shell: None,
                cancel_exit_code: None,
                query: None,
                keymap: None,
                commands: BTreeMap::from([(
                    "open".to_string(),
                    Command {
                        key: "enter".to_string(),
                        label: "Open".to_string(),
                        scope: crate::config::CommandScope::Selection,
                        requires: crate::config::CommandRequirement::Items,
                        action: CommandAction::Run {
                            payload: crate::config::RunPayload {
                                handler: crate::config::ScriptSourceSpec::script_file(
                                    "scripts/run.sh",
                                )
                                .as_toml_value(),
                                args: None,
                                shell: None,
                                exit: false,
                            },
                        },
                    },
                )]),
            },
        );
        Config::test_new(
            Some("core:default".to_string()),
            views,
            BTreeMap::from([
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
            BTreeMap::new(),
            Value::Object(serde_json::Map::new()),
        )
        .unwrap()
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
    fn item_aggregation_has_a_session_limit_and_observes_cancellation() {
        let mut result = ItemsResult::default();
        let items = Value::Array(
            (0..=MAX_ITEMS_PER_SESSION)
                .map(|index| serde_json::json!({"label": index.to_string()}))
                .collect(),
        );
        append_items(
            &mut result,
            "feed:main",
            &FeedId("feed:main".to_string()),
            "feed",
            items,
            &CancellationToken::new(),
        );
        assert!(result.items.is_empty());
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.contains("session limit"))
        );

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        append_items(
            &mut result,
            "feed:main",
            &FeedId("feed:main".to_string()),
            "feed",
            serde_json::json!([{"label": "ignored"}]),
            &cancellation,
        );
        assert!(result.items.is_empty());
    }

    #[test]
    fn items_keep_their_source_view() {
        let result = load_items(
            &test_config(),
            "core:default",
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
        assert_eq!(result.contexts.len(), 1, "one context per feed response");
        assert!(
            result
                .items
                .iter()
                .all(|item| item.feed_id == FeedId("apps:main".to_string()))
        );
    }

    #[test]
    fn feed_owner_and_active_page_are_distinct_dynamic_roots() {
        let mut config = test_config();
        config
            .views
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some(toml::Value::Array(vec![toml::Value::Table(
            [(
                "label".to_string(),
                "{{ view.ref }} <- {{ page.ref }}".into(),
            )]
            .into_iter()
            .collect(),
        )]));
        let result = load_items(
            &config,
            "core:default",
            &serde_json::json!({
                "view": {
                    "current": {
                        "ref": "core:default",
                        "input": "",
                        "raw_input": "",
                        "query": "",
                        "items": []
                    }
                }
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.items[0].text, "apps:main <- core:default");
    }

    #[test]
    fn called_picker_items_read_declared_query_values() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut state = config.instantiate_state("selectors:commands").unwrap();
        config
            .update_query_value(
                &mut state,
                &serde_json::json!({
                    "commands": [{
                        "ref": {"view": "apps:main", "id": "open"},
                        "owner": "apps:main",
                        "key": "enter",
                        "label": "Open",
                    }],
                }),
            )
            .unwrap();
        let result = load_items_for_page(
            &config,
            "selectors:commands",
            &state,
            "open",
            &serde_json::json!({}),
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(result.items.iter().any(|item| item.metadata["command"]
            == serde_json::json!({"view": "apps:main", "id": "open"})));
    }

    #[test]
    fn evaluated_items_require_json_arrays() {
        let mut config = test_config();
        config
            .views
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some("{{ page.query }}".into());
        let result = load_items(
            &config,
            "core:default",
            &serde_json::json!({
                "view": {"current": {"query": "not-an-array"}}
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(result.items.is_empty());
        assert!(result.errors[0].contains("items must resolve to an array"));
    }

    #[test]
    fn dynamic_item_labels_are_evaluated_recursively() {
        let mut config = test_config();
        config
            .views
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some(toml::Value::Array(vec![toml::Value::Table(
            [("label".to_string(), "{{ page.input }}".into())]
                .into_iter()
                .collect(),
        )]));

        let result = load_items(
            &config,
            "core:default",
            &serde_json::json!({"view": {"current": {"input": "dynamic"}}}),
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.items[0].text, "dynamic");
    }

    #[test]
    fn path_results_can_become_script_sources() {
        let root = env::temp_dir().join(format!(
            "tui-launcher-items-dynamic-source-{}",
            std::process::id()
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("items.sh"),
            "printf '%s\\n' '[{\"label\":\"manufactured source\"}]'\n",
        )
        .unwrap();

        let mut config = test_config();
        config
            .views
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some("{{ page.query }}".into());
        config.config_value = serde_json::json!({
            "plugins": {
                "apps": {
                    "views": {
                        "main": {
                            "type": "picker",
                            "items": "{{ page.query }}"
                        }
                    }
                }
            }
        });
        config.state_registry = crate::state::StateRegistry::compile(&config.config_value).unwrap();
        config.rebuild_template_registry().unwrap();
        config.plugin_roots.insert("apps".to_string(), root.clone());
        let result = load_items(
            &config,
            "core:default",
            &serde_json::json!({
                "view": {"current": {"query": {"source": "script", "file": "items.sh"}}}
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.items[0].text, "manufactured source");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_items_are_reported_without_partial_results() {
        let result = load_items(
            &test_config(),
            "core:default",
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
    fn dynamic_source_args_pass_view_query_to_scripts() {
        let root =
            env::temp_dir().join(format!("tui-launcher-items-script-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("items.sh"),
            "jq -cn --arg label \"$1\" '[{label: $label}]'\n",
        )
        .unwrap();

        let mut config = test_config();
        config
            .views
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some(script_source(
            "items.sh",
            Some(toml::Value::Array(vec!["{{ view.query }}".into()])),
        ));
        config.config_value = serde_json::json!({
            "plugins": {
                "core": {
                    "views": {
                        "default": {"type": "picker", "feeds": [{"view": "apps:main"}]}
                    }
                },
                "apps": {
                    "views": {
                        "main": {
                            "type": "picker",
                            "alias": "app",
                            "items": {"source": "script", "file": "items.sh", "args": ["{{ view.query }}"]}
                        }
                    }
                }
            }
        });
        config.state_registry = crate::state::StateRegistry::compile(&config.config_value).unwrap();
        config.rebuild_template_registry().unwrap();
        config.plugin_roots.insert("apps".to_string(), root.clone());
        let page_state = config.instantiate_state("core:default").unwrap();
        let result = load_items_for_page(
            &config,
            "core:default",
            &page_state,
            "fire",
            &serde_json::json!({}),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(
            result.items.len(),
            1,
            "items={:?} errors={:?}",
            result.items,
            result.errors
        );
        assert_eq!(result.items[0].text, "fire");
        assert_eq!(result.contexts.len(), 1);
        let context = &result.contexts[&result.items[0].feed_id];
        assert_eq!(context.binding_raw, "fire");
        assert_eq!(config.query_value(&context.state).unwrap(), "fire");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn feed_binding_raw_input_keeps_empty_binding() {
        let root = env::temp_dir().join(format!("tui-launcher-items-raw-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("items.sh"),
            concat!(
                "raw=$1\n",
                "text=$2\n",
                "jq -cn --arg raw \"$raw\" --arg text \"$text\" \
",
                "  '[{label:(\"RAW:\" + $raw + \"|TEXT:\" + $text)}]'\n",
            ),
        )
        .unwrap();

        let mut config = test_config();
        config
            .views
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some(script_source(
            "items.sh",
            Some(toml::Value::Array(vec![
                "{{ view.raw_input }}".into(),
                "{{ view.query.text }}".into(),
            ])),
        ));
        config.config_value = serde_json::json!({
            "plugins": {
                "apps": {
                    "views": {
                        "main": {
                            "type": "picker",
                            "query": {
                                "type": "object",
                                "input_order": ["text"],
                                "text": {"type": "string", "default": "source-default"}
                            },
                            "items": {"source": "script", "file": "items.sh", "args": ["{{ view.raw_input }}", "{{ view.query.text }}"]}
                        }
                    }
                }
            }
        });
        config.state_registry = crate::state::StateRegistry::compile(&config.config_value).unwrap();
        config.rebuild_template_registry().unwrap();
        config.plugin_roots.insert("apps".to_string(), root.clone());
        let page_state = config.instantiate_state("core:default").unwrap();
        let result = load_items_for_page(
            &config,
            "core:default",
            &page_state,
            "",
            &serde_json::json!({}),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(
            result.items[0].text, "RAW:|TEXT:source-default",
            "errors={:?}",
            result.errors
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn required_feed_failure_does_not_block_a_later_defaulted_feed() {
        let root = env::temp_dir().join(format!(
            "tui-launcher-items-required-isolation-{}",
            std::process::id()
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        let invalid_provider_marker = root.join("invalid-provider-ran");
        fs::write(
            root.join("invalid-items.sh"),
            "printf 'ran\\n' > invalid-provider-ran\nprintf '[]\\n'\n",
        )
        .unwrap();
        fs::write(
            root.join("items.sh"),
            concat!(
                "payload=$1\n",
                "raw=$(printf '%s' \"$payload\" | jq -r .raw_input)\n",
                "text=$(printf '%s' \"$payload\" | jq -r .query.text)\n",
                "jq -cn --arg raw \"$raw\" --arg text \"$text\" ",
                "'[{label:(\"RAW:\" + $raw + \"|TEXT:\" + $text)}]'\n",
            ),
        )
        .unwrap();

        let mut config = test_config();
        config.views.insert(
            "sys:main".to_string(),
            View {
                engine: EngineSpec {
                    engine_type: ENGINE_PICKER.to_string(),
                    config: EngineOptions {
                        items: Some(script_source(
                            "items.sh",
                            Some(toml::Value::Array(vec!["{{ view }}".into()])),
                        )),
                        ..Default::default()
                    },
                },
                alias: Some("sys".to_string()),
                run_shell: None,
                cancel_exit_code: None,
                query: None,
                keymap: None,
                commands: BTreeMap::new(),
            },
        );
        config
            .views
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some(script_source("invalid-items.sh", None));
        config
            .views
            .get_mut("core:default")
            .unwrap()
            .engine
            .config
            .feeds = vec![
            FeedSpec {
                view: "apps:main".to_string(),
            },
            FeedSpec {
                view: "sys:main".to_string(),
            },
        ];
        config.config_value = serde_json::json!({
            "plugins": {
                "apps": {
                    "views": {
                        "main": {
                            "type": "picker",
                            "query": {
                                "type": "object",
                                "input_order": [],
                                "token": {"type": "string"}
                            },
                            "items": {"source": "script", "file": "invalid-items.sh"}
                        }
                    }
                },
                "sys": {
                    "views": {
                        "main": {
                            "type": "picker",
                            "query": {
                                "type": "object",
                                "input_order": ["text"],
                                "text": {"type": "string", "default": "later-default"}
                            },
                            "items": {"source": "script", "file": "items.sh", "args": ["{{ view }}"]}
                        }
                    }
                }
            }
        });
        config.state_registry = crate::state::StateRegistry::compile(&config.config_value).unwrap();
        config.rebuild_template_registry().unwrap();
        config.plugin_roots.insert("apps".to_string(), root.clone());
        config.plugin_roots.insert("sys".to_string(), root.clone());
        let page_state = config.instantiate_state("core:default").unwrap();
        let result = load_items_for_page(
            &config,
            "core:default",
            &page_state,
            "",
            &serde_json::json!({}),
            &CancellationToken::new(),
        )
        .unwrap();

        assert_eq!(result.items.len(), 1, "errors={:?}", result.errors);
        assert_eq!(result.items[0].text, "RAW:|TEXT:later-default");
        assert_eq!(result.items[0].source_view, "sys:main");
        assert_eq!(result.contexts.len(), 1);
        assert!(
            result
                .contexts
                .contains_key(&FeedId("sys:main".to_string()))
        );
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("apps:main"));
        assert!(result.errors[0].contains("--token is required"));
        assert!(
            !invalid_provider_marker.exists(),
            "invalid feed provider ran before state validation"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nonempty_binding_does_not_run_a_feed_without_ordered_fields() {
        let mut config = test_config();
        config
            .views
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some("{{ page.query.provider_should_not_run }}".into());
        config.config_value = serde_json::json!({
            "plugins": {
                "apps": {
                    "views": {
                        "main": {
                            "type": "picker",
                            "query": {
                                "type": "object",
                                "input_order": [],
                                "token": {"type": "string", "default": "fixed"}
                            },
                            "items": "{{ page.query.provider_should_not_run }}"
                        }
                    }
                }
            }
        });
        config.state_registry = crate::state::StateRegistry::compile(&config.config_value).unwrap();
        config.rebuild_template_registry().unwrap();
        let page_state = config.instantiate_state("core:default").unwrap();

        let result = load_items_for_page(
            &config,
            "core:default",
            &page_state,
            "needle",
            &serde_json::json!({}),
            &CancellationToken::new(),
        )
        .unwrap();

        assert!(result.items.is_empty());
        assert!(result.contexts.is_empty());
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("input_order is empty"));
        assert!(!result.errors[0].contains("provider_should_not_run"));
    }

    #[test]
    fn feeds_merge_items_in_config_order() {
        let mut config = test_config();
        config.views.insert(
            "sys:main".to_string(),
            View {
                engine: EngineSpec {
                    engine_type: ENGINE_PICKER.to_string(),
                    config: EngineOptions {
                        items: Some(toml::Value::Array(vec![toml::Value::Table(
                            [(
                                "label".to_string(),
                                toml::Value::String("SysItem".to_string()),
                            )]
                            .into_iter()
                            .collect(),
                        )])),
                        ..Default::default()
                    },
                },
                alias: Some("sys".to_string()),
                run_shell: None,
                cancel_exit_code: None,
                query: None,
                keymap: None,
                commands: BTreeMap::new(),
            },
        );
        config
            .views
            .get_mut("core:default")
            .unwrap()
            .engine
            .config
            .feeds = vec![
            FeedSpec {
                view: "apps:main".to_string(),
            },
            FeedSpec {
                view: "sys:main".to_string(),
            },
        ];
        let result = load_items(
            &config,
            "core:default",
            &serde_json::json!({
                "view": {"current": {"items": [{"label": "AppItem"}]}},
            }),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(
            result
                .items
                .iter()
                .map(|item| (item.text.as_str(), item.source_view.as_str()))
                .collect::<Vec<_>>(),
            [("AppItem", "apps:main"), ("SysItem", "sys:main")]
        );
    }

    #[test]
    fn feed_object_defaults_apply_with_empty_input() {
        let root = env::temp_dir().join(format!(
            "tui-launcher-items-defaults-{}",
            std::process::id()
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("items.sh"),
            "payload=${1#query=}\njq -cn --arg label \"$(printf '%s' \"$payload\" | jq -r .text)\" '[{label:(\"VALUE:\" + $label)}]'\n",
        )
        .unwrap();

        let mut config = test_config();
        config
            .views
            .get_mut("apps:main")
            .unwrap()
            .engine
            .config
            .items = Some(script_source(
            "items.sh",
            Some(toml::Value::Array(vec!["query={{ view.query }}".into()])),
        ));
        config.config_value = serde_json::json!({
            "plugins": {
                "apps": {
                    "views": {
                        "main": {
                            "type": "picker",
                            "query": {
                                "type": "object",
                                "input_order": ["text"],
                                "text": {"type": "string", "default": "source-default"}
                            },
                            "items": {"source": "script", "file": "items.sh", "args": ["query={{ view.query }}"]}
                        }
                    }
                }
            }
        });
        config.state_registry = crate::state::StateRegistry::compile(&config.config_value).unwrap();
        config.rebuild_template_registry().unwrap();
        config.plugin_roots.insert("apps".to_string(), root.clone());
        let page_state = config.instantiate_state("core:default").unwrap();
        let result = load_items_for_page(
            &config,
            "core:default",
            &page_state,
            "",
            &serde_json::json!({}),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(result.items[0].text, "VALUE:source-default");
        fs::remove_dir_all(root).unwrap();
    }
}
