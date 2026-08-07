use crate::command_runner::run_bounded_command;
use crate::config::{Config, Rule, View};
use crate::expression::ExpressionMethods;
use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct DiscoveryItem {
    label: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    metadata: Value,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub prefix: String,
    pub text: String,
    pub value: Option<String>,
    pub metadata: Value,
    pub source_view: String,
}

#[derive(Debug, Default)]
pub struct DiscoveryResult {
    pub items: Vec<Item>,
    pub errors: Vec<String>,
}

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DISCOVERY_STDOUT: usize = 1024 * 1024;
const MAX_DISCOVERY_STDERR: usize = 64 * 1024;

pub fn load_items(
    config: &Config,
    view_ref: &str,
    rule_name: &str,
    rule: &Rule,
    query: &str,
    source_prefix: Option<&str>,
    log_file: Option<&Path>,
    runtime: &Value,
) -> Result<DiscoveryResult> {
    load_items_internal(
        config,
        view_ref,
        rule_name,
        rule,
        query,
        source_prefix,
        log_file,
        Some(runtime),
    )
}

fn load_items_internal(
    config: &Config,
    view_ref: &str,
    rule_name: &str,
    rule: &Rule,
    query: &str,
    source_prefix: Option<&str>,
    log_file: Option<&Path>,
    runtime: Option<&Value>,
) -> Result<DiscoveryResult> {
    let mut result = DiscoveryResult::default();

    for (source_ref, view) in config.source_views(view_ref)? {
        let display_prefix = display_prefix(&source_ref, view);
        if source_prefix.is_some_and(|prefix| prefix != display_prefix) {
            continue;
        }

        if let Some(runtime) = runtime
            && view.items.is_some()
        {
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
            parse_item_values(
                &mut result,
                &source_ref,
                view,
                &display_prefix,
                rule,
                query,
                value,
            );
            continue;
        }

        let script = if source_prefix.is_some() {
            view.query_discover.as_ref().or(view.discover.as_ref())
        } else {
            view.default_discover.as_ref().or(view.discover.as_ref())
        };
        let Some(script) = script else {
            continue;
        };

        let shell = view.discover_shell.as_deref().unwrap_or("sh");
        let mut process = Command::new(shell);
        process.args(["-c", script, "tui-launcher"]);
        process.env("LAUNCHER_RULE", rule_name);
        process.env("LAUNCHER_PLUGIN", plugin_name(&source_ref));
        process.env("LAUNCHER_VIEW", view_name(&source_ref));
        process.env("LAUNCHER_VIEW_REF", &source_ref);
        process.env("LAUNCHER_PROVIDER", plugin_name(&source_ref));
        process.env("LAUNCHER_QUERY", query);
        if let Some(log_file) = log_file {
            process.env("LAUNCHER_LOG_FILE", log_file);
        }
        if let Some(root) = config.plugin_root(&source_ref) {
            process.current_dir(root);
            process.env("LAUNCHER_PLUGIN_DIR", root);
        }

        let output = match run_discovery_command(process) {
            Ok(output) => output,
            Err(error) => {
                result.errors.push(format!("{}: {}", source_ref, error));
                continue;
            }
        };

        if !output.status.success() {
            let stderr = sanitize_text(&String::from_utf8_lossy(&output.stderr));
            let detail = if stderr.is_empty() {
                format!("exit status {}", output.status)
            } else {
                stderr
            };
            result.errors.push(format!("{}: {}", source_ref, detail));
        }

        parse_items(
            &mut result,
            &source_ref,
            view,
            &display_prefix,
            rule,
            query,
            &String::from_utf8_lossy(&output.stdout),
        );
    }

    Ok(result)
}

fn run_discovery_command(process: Command) -> Result<std::process::Output> {
    run_bounded_command(
        process,
        DISCOVERY_TIMEOUT,
        MAX_DISCOVERY_STDOUT,
        MAX_DISCOVERY_STDERR,
    )
}

fn parse_item_values(
    result: &mut DiscoveryResult,
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
        let parsed = match serde_json::from_value::<DiscoveryItem>(value.clone()) {
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

fn parse_items(
    result: &mut DiscoveryResult,
    source_ref: &str,
    view: &View,
    display_prefix: &str,
    rule: &Rule,
    query: &str,
    stdout: &str,
) {
    for (line_number, line) in stdout.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed = match serde_json::from_str::<DiscoveryItem>(line) {
            Ok(item) => item,
            Err(error) => {
                result.errors.push(format!(
                    "{}: invalid discovery JSON on line {}: {}",
                    source_ref,
                    line_number + 1,
                    error
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

fn plugin_name(view_ref: &str) -> &str {
    view_ref
        .split_once(':')
        .map(|(plugin, _)| plugin)
        .unwrap_or(view_ref)
}

fn view_name(view_ref: &str) -> &str {
    view_ref
        .split_once(':')
        .map(|(_, view)| view)
        .unwrap_or(view_ref)
}

pub fn matches_query(text: &str, query: &str) -> bool {
    let folded = text.to_lowercase();
    query
        .split_whitespace()
        .all(|token| folded.contains(&token.to_lowercase()))
}

pub fn sanitize_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut escape = false;
    let mut csi = false;
    let mut osc = false;
    let mut osc_escape = false;

    for character in text.chars() {
        if osc {
            if osc_escape {
                osc_escape = false;
                if character == '\\' {
                    osc = false;
                }
            } else if character == '\u{7}' {
                osc = false;
            } else if character == '\u{1b}' {
                osc_escape = true;
            }
            continue;
        }
        if csi {
            if ('@'..='~').contains(&character) {
                csi = false;
            }
            continue;
        }
        if escape {
            escape = false;
            match character {
                '[' => csi = true,
                ']' => osc = true,
                _ => {}
            }
            continue;
        }
        if character == '\u{1b}' {
            escape = true;
            continue;
        }
        if character.is_control() {
            if character == '\t' {
                output.push(' ');
            }
            continue;
        }
        output.push(character);
    }

    output.trim().to_string()
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
                discover: None,
                default_discover: None,
                query_discover: None,
                discover_shell: None,
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
                items: None,
                discover: Some("printf '%s\\n' '{\"label\":\"Termius\"}'".to_string()),
                default_discover: None,
                query_discover: None,
                discover_shell: None,
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
            rules: BTreeMap::from([("default".to_string(), crate::config::Rule { filter: true })]),
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
    fn matches_all_query_tokens() {
        assert!(matches_query("Restart API service", "api start"));
        assert!(!matches_query("Restart API service", "api database"));
    }

    #[test]
    fn parses_structured_discovery_items() {
        let item: DiscoveryItem = serde_json::from_str(
            r#"{"label":"Termius","value":"termius.desktop","metadata":{"kind":"app"}}"#,
        )
        .unwrap();
        assert_eq!(item.label, "Termius");
        assert_eq!(item.value.as_deref(), Some("termius.desktop"));
        assert_eq!(item.metadata["kind"], "app");
    }

    #[test]
    fn discovered_items_keep_their_source_view() {
        let result = load_items(
            &test_config(),
            "core:default",
            "default",
            &crate::config::Rule { filter: true },
            "",
            None,
            None,
            &Value::Null,
        )
        .unwrap();
        assert_eq!(result.items[0].source_view, "apps:main");
        assert_eq!(result.items[0].prefix, "app");
    }

    #[test]
    fn item_expressions_load_json_arrays() {
        let mut config = test_config();
        let source = config.views.get_mut("apps:main").unwrap();
        source.discover = None;
        source.items = Some("{{ runtime:view.current.items }}".to_string());
        let runtime = serde_json::json!({
            "view": {
                "current": {
                    "items": [{"label": "Termius", "value": "termius"}]
                }
            }
        });
        let result = load_items(
            &config,
            "core:default",
            "default",
            &crate::config::Rule { filter: true },
            "",
            None,
            None,
            &runtime,
        )
        .unwrap();
        assert_eq!(result.errors, Vec::<String>::new());
        assert_eq!(result.items[0].text, "Termius");
        assert_eq!(result.items[0].source_view, "apps:main");
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
        let source = config.views.get_mut("apps:main").unwrap();
        source.discover = None;
        source.items = Some("{{ script(\"items.sh\", runtime:view.current.query) }}".to_string());
        config.plugin_roots.insert("apps".to_string(), root.clone());
        let runtime = serde_json::json!({
            "view": {"current": {"query": "fire"}}
        });
        let result = load_items(
            &config,
            "core:default",
            "default",
            &crate::config::Rule { filter: true },
            "fire",
            None,
            None,
            &runtime,
        )
        .unwrap();
        assert_eq!(result.items[0].text, "fire");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn strips_terminal_controls_from_items() {
        assert_eq!(sanitize_text("\u{1b}[31mred\u{1b}[0m\n"), "red");
    }
}
