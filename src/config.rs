use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_rule_name")]
    pub default_rule: String,
    #[serde(default)]
    pub rules: BTreeMap<String, Rule>,
    #[serde(default)]
    pub providers: BTreeMap<String, Provider>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    #[serde(default = "default_true")]
    pub filter: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Provider {
    pub discover: Option<String>,
    pub default_discover: Option<String>,
    pub query_discover: Option<String>,
    pub run: Option<String>,
    #[serde(default)]
    pub display_prefix: Option<String>,
    #[serde(default)]
    pub discover_shell: Option<String>,
    #[serde(default)]
    pub run_shell: Option<String>,
    #[serde(default = "default_true")]
    pub filter: bool,
    #[serde(default)]
    pub mode: ActionMode,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionMode {
    #[default]
    Oneshot,
    Capture,
    Embedded,
    Takeover,
}

impl Config {
    pub fn load_with_defaults(default_source: &str, user_path: &Path) -> Result<Self> {
        let mut merged: toml::Value = toml::from_str(default_source)
            .context("cannot parse built-in default configuration")?;

        match fs::read_to_string(user_path) {
            Ok(user_source) => {
                let user_config: toml::Value = toml::from_str(&user_source)
                    .with_context(|| format!("cannot parse config {}", user_path.display()))?;
                merge_values(&mut merged, user_config);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("cannot read config {}", user_path.display()));
            }
        }

        let mut config: Self = merged
            .try_into()
            .context("merged configuration does not match the launcher schema")?;
        if !config.rules.contains_key(&config.default_rule) {
            config
                .rules
                .insert(config.default_rule.clone(), Rule { filter: true });
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if !self.rules.contains_key(&self.default_rule) {
            bail!(
                "default rule {:?} is not defined in [rules]",
                self.default_rule
            );
        }

        for (provider_name, provider) in &self.providers {
            if let Some(command) = &provider.discover {
                validate_script(command, "discover", provider_name)?;
            }
            if let Some(command) = &provider.default_discover {
                validate_script(command, "default_discover", provider_name)?;
            }
            if let Some(command) = &provider.query_discover {
                validate_script(command, "query_discover", provider_name)?;
            }
            if let Some(shell) = &provider.discover_shell {
                validate_script(shell, "discover_shell", provider_name)?;
            }
            if let Some(script) = &provider.run {
                validate_script(script, "run", provider_name)?;
            }
            if let Some(shell) = &provider.run_shell {
                validate_script(shell, "run_shell", provider_name)?;
            }
        }

        Ok(())
    }

    pub fn resolve_rule<'a>(&'a self, input: &str) -> (&'a str, &'a Rule, String) {
        let rule_name = self.default_rule.as_str();
        (rule_name, &self.rules[rule_name], input.to_string())
    }

    pub fn resolve_provider_prefix(&self, input: &str) -> (Option<String>, String) {
        let Some((prefix, query)) = input.split_once(' ') else {
            return (None, input.to_string());
        };
        if prefix.is_empty() {
            return (None, input.to_string());
        }

        let matches_provider = self.providers.iter().any(|(provider_name, provider)| {
            provider.display_prefix.as_deref().unwrap_or(provider_name) == prefix
        });
        if matches_provider {
            (Some(prefix.to_string()), query.trim_start().to_string())
        } else {
            (None, input.to_string())
        }
    }
}

fn merge_values(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                if let Some(existing) = base.get_mut(&key) {
                    merge_values(existing, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

fn validate_script(script: &str, kind: &str, provider_name: &str) -> Result<()> {
    if script.trim().is_empty() {
        bail!("provider {:?} has an empty {} script", provider_name, kind);
    }
    Ok(())
}

fn default_rule_name() -> String {
    "default".to_string()
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        let mut rules = BTreeMap::new();
        rules.insert("default".to_string(), Rule { filter: true });
        Config {
            default_rule: "default".to_string(),
            rules,
            providers: BTreeMap::new(),
        }
    }

    #[test]
    fn default_rule_keeps_the_full_query() {
        let config = test_config();
        let (name, _, query) = config.resolve_rule("sys date");
        assert_eq!(name, "default");
        assert_eq!(query, "sys date");
    }

    #[test]
    fn display_prefix_routes_queries_after_a_space() {
        let mut config = test_config();
        config.providers.insert(
            "system".to_string(),
            Provider {
                discover: None,
                default_discover: None,
                query_discover: None,
                run: None,
                display_prefix: Some("sys".to_string()),
                discover_shell: None,
                run_shell: None,
                filter: true,
                mode: ActionMode::default(),
            },
        );

        assert_eq!(
            config.resolve_provider_prefix("sys date"),
            (Some("sys".to_string()), "date".to_string())
        );
        assert_eq!(
            config.resolve_provider_prefix("sysdate"),
            (None, "sysdate".to_string())
        );
    }

    #[test]
    fn provider_discovery_modes_are_deserialized_separately() {
        let config: Config = toml::from_str(
            r#"
            default_rule = "default"
            [rules.default]
            [providers.trans]
            default_discover = "default-command"
            query_discover = "query-command"
            "#,
        )
        .unwrap();
        let provider = &config.providers["trans"];
        assert_eq!(
            provider.default_discover.as_deref(),
            Some("default-command")
        );
        assert_eq!(provider.query_discover.as_deref(), Some("query-command"));
    }

    #[test]
    fn user_tables_extend_defaults_and_arrays_are_replaced() {
        let mut base: toml::Value = toml::from_str(
            r#"
            default_rule = "default"
            [rules.default]
            [providers.base]
            discover = "base-discover"
            run = "base-run"
            "#,
        )
        .unwrap();
        let overlay: toml::Value = toml::from_str(
            r#"
            [providers.base]
            discover = "user-discover"
            query_discover = "user-query"
            "#,
        )
        .unwrap();

        merge_values(&mut base, overlay);
        let config: Config = base.try_into().unwrap();
        assert_eq!(
            config.providers["base"].discover,
            Some("user-discover".to_string())
        );
        assert_eq!(
            config.providers["base"].query_discover,
            Some("user-query".to_string())
        );
    }
}
