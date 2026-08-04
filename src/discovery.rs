use crate::config::{Config, Rule};
use anyhow::{Context, Result};
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
    pub provider: String,
}

#[derive(Debug, Default)]
pub struct DiscoveryResult {
    pub items: Vec<Item>,
    pub errors: Vec<String>,
}

pub fn discover(
    config: &Config,
    rule_name: &str,
    rule: &Rule,
    query: &str,
    provider_prefix: Option<&str>,
) -> Result<DiscoveryResult> {
    let mut result = DiscoveryResult::default();

    for provider_name in config.providers.keys().map(String::as_str) {
        let provider = config
            .providers
            .get(provider_name)
            .with_context(|| format!("provider {:?} is not configured", provider_name))?;

        let display_prefix = provider
            .display_prefix
            .clone()
            .unwrap_or_else(|| provider_name.to_string());
        if provider_prefix.is_some_and(|prefix| prefix != display_prefix) {
            continue;
        }
        let script = if provider_prefix.is_some() {
            provider
                .query_discover
                .as_ref()
                .or(provider.discover.as_ref())
        } else {
            provider
                .default_discover
                .as_ref()
                .or(provider.discover.as_ref())
        };
        let Some(script) = script else {
            continue;
        };
        let shell = provider.discover_shell.as_deref().unwrap_or("sh");
        let mut process = Command::new(shell);
        process.args(["-c", script, "tui-launcher"]);
        process.env("LAUNCHER_RULE", rule_name);
        process.env("LAUNCHER_PROVIDER", provider_name);
        process.env("LAUNCHER_QUERY", query);

        let output = match process.output() {
            Ok(output) => output,
            Err(error) => {
                result.errors.push(format!(
                    "{}: could not run discovery command: {}",
                    provider_name, error
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
            result.errors.push(format!("{}: {}", provider_name, detail));
        }

        for (line_number, line) in String::from_utf8_lossy(&output.stdout).lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let parsed = match serde_json::from_str::<DiscoveryItem>(line) {
                Ok(item) => item,
                Err(error) => {
                    result.errors.push(format!(
                        "{}: invalid discovery JSON on line {}: {}",
                        provider_name,
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
            if rule.filter && provider.filter && !matches_query(&text, query) {
                continue;
            }
            result.items.push(Item {
                prefix: display_prefix.clone(),
                text,
                value: parsed.value,
                metadata: parsed.metadata,
                provider: provider_name.to_string(),
            });
        }
    }

    Ok(result)
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
    fn strips_terminal_controls_from_items() {
        assert_eq!(sanitize_text("\u{1b}[31mred\u{1b}[0m\n"), "red");
    }
}
