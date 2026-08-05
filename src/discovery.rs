use crate::config::{Config, Rule, View};
use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;
use std::process::Command;

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

pub fn discover(
    config: &Config,
    view_ref: &str,
    rule_name: &str,
    rule: &Rule,
    query: &str,
    source_prefix: Option<&str>,
) -> Result<DiscoveryResult> {
    let mut result = DiscoveryResult::default();

    for (source_ref, view) in config.source_views(view_ref)? {
        let display_prefix = display_prefix(&source_ref, view);
        if source_prefix.is_some_and(|prefix| prefix != display_prefix) {
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
        if let Some(root) = config.plugin_root(&source_ref) {
            process.current_dir(root);
            process.env("LAUNCHER_PLUGIN_DIR", root);
        }

        let output = match process.output() {
            Ok(output) => output,
            Err(error) => {
                result.errors.push(format!(
                    "{}: could not run discovery command: {}",
                    source_ref, error
                ));
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
    use crate::config::{Command, ViewType};
    use std::collections::BTreeMap;

    fn test_config() -> Config {
        let mut views = BTreeMap::new();
        views.insert(
            "core:default".to_string(),
            View {
                view_type: ViewType::Launcher,
                sources: vec!["apps:main".to_string()],
                display_prefix: None,
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
                view_type: ViewType::Launcher,
                sources: Vec::new(),
                display_prefix: Some("app".to_string()),
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
            default_rule: "default".to_string(),
            rules: BTreeMap::from([("default".to_string(), crate::config::Rule { filter: true })]),
            views,
            plugin_roots: BTreeMap::new(),
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
        let result = discover(
            &test_config(),
            "core:default",
            "default",
            &crate::config::Rule { filter: true },
            "",
            None,
        )
        .unwrap();
        assert_eq!(result.items[0].source_view, "apps:main");
        assert_eq!(result.items[0].prefix, "app");
    }

    #[test]
    fn strips_terminal_controls_from_items() {
        assert_eq!(sanitize_text("\u{1b}[31mred\u{1b}[0m\n"), "red");
    }
}
