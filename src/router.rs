use crate::config::{Config, ViewRef};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RouteResolution {
    NotMatched,
    Current {
        query: String,
    },
    Navigate {
        target: ViewRef,
        query: String,
    },
    Ambiguous {
        alias: String,
        targets: Vec<ViewRef>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouteDisplay {
    pub(crate) view_ref: ViewRef,
    pub(crate) alias: Option<String>,
}

impl RouteDisplay {
    pub(crate) fn label(&self) -> String {
        self.alias
            .as_deref()
            .map(|alias| format!("{} ({})", self.view_ref, alias))
            .unwrap_or_else(|| self.view_ref.clone())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Router {
    views: BTreeSet<ViewRef>,
    aliases: BTreeMap<String, Vec<ViewRef>>,
    display: BTreeMap<ViewRef, RouteDisplay>,
}

impl Router {
    pub(crate) fn new(config: &Config) -> Self {
        let views = config.views.keys().cloned().collect::<BTreeSet<_>>();
        let mut aliases = BTreeMap::<String, Vec<ViewRef>>::new();
        let mut display = BTreeMap::new();
        for (view_ref, view) in &config.views {
            if let Some(alias) = &view.alias {
                aliases
                    .entry(alias.clone())
                    .or_default()
                    .push(view_ref.clone());
            }
            display.insert(
                view_ref.clone(),
                RouteDisplay {
                    view_ref: view_ref.clone(),
                    alias: view.alias.clone(),
                },
            );
        }
        for targets in aliases.values_mut() {
            targets.sort();
            targets.dedup();
        }
        Self {
            views,
            aliases,
            display,
        }
    }

    pub(crate) fn resolve(&self, current_view_ref: &str, input: &str) -> RouteResolution {
        let Some((selector, query)) = split_selector(input) else {
            return RouteResolution::NotMatched;
        };
        if selector.is_empty() {
            return RouteResolution::NotMatched;
        }

        let targets = if selector.contains(':') {
            if !valid_view_ref(selector) || !self.views.contains(selector) {
                return RouteResolution::NotMatched;
            }
            vec![selector.to_string()]
        } else {
            let Some(targets) = self.aliases.get(selector) else {
                return RouteResolution::NotMatched;
            };
            targets.clone()
        };

        match targets.as_slice() {
            [target] if target == current_view_ref => RouteResolution::Current {
                query: query.to_string(),
            },
            [target] => RouteResolution::Navigate {
                target: target.clone(),
                query: query.to_string(),
            },
            _ => RouteResolution::Ambiguous {
                alias: selector.to_string(),
                targets,
            },
        }
    }

    pub(crate) fn display(&self, view_ref: &str) -> RouteDisplay {
        self.display
            .get(view_ref)
            .cloned()
            .unwrap_or_else(|| RouteDisplay {
                view_ref: view_ref.to_string(),
                alias: None,
            })
    }
}

fn split_selector(input: &str) -> Option<(&str, &str)> {
    input
        .split_once(char::is_whitespace)
        .map(|(selector, query)| (selector, query.trim_start()))
        .or_else(|| input.contains(':').then_some((input, "")))
}

fn valid_view_ref(view_ref: &str) -> bool {
    let Some((package, view)) = view_ref.split_once(':') else {
        return false;
    };
    !package.is_empty() && !view.is_empty() && !view.contains(':')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Defaults, DisplayType, ENGINE_PICKER, PluginMetadata, View};
    use serde_json::Value;
    use std::collections::BTreeMap;

    fn view(alias: Option<&str>) -> View {
        View {
            engine_type: ENGINE_PICKER.to_string(),
            display: DisplayType::Text,
            sources: Vec::new(),
            alias: alias.map(str::to_string),
            items: None,
            run_shell: None,
            commands: BTreeMap::new(),
            engine_config: toml::Table::new(),
        }
    }

    fn config() -> Config {
        Config {
            default_view: "core:default".to_string(),
            dmenu_view: "core:dmenu".to_string(),
            command_view: "core:command".to_string(),
            views: BTreeMap::from([
                ("core:default".to_string(), view(None)),
                ("package-a:default".to_string(), view(Some("temp"))),
                ("package-a:view2".to_string(), view(Some("detail"))),
            ]),
            plugins: BTreeMap::from([
                (
                    "core".to_string(),
                    PluginMetadata {
                        name: "core".to_string(),
                    },
                ),
                (
                    "package-a".to_string(),
                    PluginMetadata {
                        name: "template".to_string(),
                    },
                ),
            ]),
            defaults: Defaults::default(),
            plugin_roots: BTreeMap::new(),
            config_value: Value::Object(serde_json::Map::new()),
        }
    }

    #[test]
    fn resolves_alias_and_canonical_view_references() {
        let router = Router::new(&config());
        assert_eq!(
            router.resolve("core:default", "temp aa"),
            RouteResolution::Navigate {
                target: "package-a:default".to_string(),
                query: "aa".to_string(),
            }
        );
        assert_eq!(
            router.resolve("core:default", "temp"),
            RouteResolution::NotMatched
        );
        assert_eq!(
            router.resolve("core:default", "package-a:view2"),
            RouteResolution::Navigate {
                target: "package-a:view2".to_string(),
                query: String::new(),
            }
        );
        assert_eq!(
            router.resolve("core:default", "package-a:view2 aa"),
            RouteResolution::Navigate {
                target: "package-a:view2".to_string(),
                query: "aa".to_string(),
            }
        );
        assert_eq!(
            router.resolve("package-a:default", "temp aa"),
            RouteResolution::Current {
                query: "aa".to_string(),
            }
        );
    }

    #[test]
    fn plain_plugin_id_does_not_expand_to_default() {
        let router = Router::new(&config());
        assert_eq!(
            router.resolve("core:default", "package-a query"),
            RouteResolution::NotMatched
        );
    }

    #[test]
    fn duplicate_aliases_are_ambiguous_only_when_used() {
        let mut config = config();
        config
            .views
            .insert("package-b:default".to_string(), view(Some("temp")));
        let router = Router::new(&config);
        assert_eq!(
            router.resolve("core:default", "temp query"),
            RouteResolution::Ambiguous {
                alias: "temp".to_string(),
                targets: vec![
                    "package-a:default".to_string(),
                    "package-b:default".to_string(),
                ],
            }
        );
    }

    #[test]
    fn display_contains_the_optional_alias() {
        let router = Router::new(&config());
        assert_eq!(
            router.display("package-a:default").label(),
            "package-a:default (temp)"
        );
        assert_eq!(router.display("core:default").label(), "core:default");
    }
}
