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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ViewCandidate {
    pub(crate) view_ref: ViewRef,
    pub(crate) alias: Option<String>,
    pub(crate) plugin_name: String,
    pub(crate) engine_type: String,
}

impl ViewCandidate {
    pub(crate) fn primary_label(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.view_ref)
    }

    pub(crate) fn secondary_label(&self) -> &str {
        &self.view_ref
    }
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
    candidates: Vec<ViewCandidate>,
}

impl Router {
    pub(crate) fn new(config: &Config) -> Self {
        let views = config.views.keys().cloned().collect::<BTreeSet<_>>();
        let mut aliases = BTreeMap::<String, Vec<ViewRef>>::new();
        let mut display = BTreeMap::new();
        let mut candidates = Vec::with_capacity(config.views.len());
        for (view_ref, view) in &config.views {
            let plugin = view_ref
                .split_once(':')
                .map(|(plugin, _)| plugin)
                .unwrap_or(view_ref);
            let plugin_name = config
                .plugins
                .get(plugin)
                .map(|metadata| metadata.name.clone())
                .unwrap_or_else(|| plugin.to_string());
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
            candidates.push(ViewCandidate {
                view_ref: view_ref.clone(),
                alias: view.alias.clone(),
                plugin_name,
                engine_type: view.engine_type.clone(),
            });
        }
        for targets in aliases.values_mut() {
            targets.sort();
            targets.dedup();
        }
        candidates.sort_by(|left, right| left.view_ref.cmp(&right.view_ref));
        Self {
            views,
            aliases,
            display,
            candidates,
        }
    }

    pub(crate) fn complete_views(&self, query: &str) -> Vec<ViewCandidate> {
        let query = query.trim().to_lowercase();
        let mut matches = self
            .candidates
            .iter()
            .filter_map(|candidate| {
                view_match_score(candidate, &query).map(|score| (score, candidate.clone()))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.view_ref.cmp(&right.1.view_ref))
        });
        matches
            .into_iter()
            .map(|(_, candidate)| candidate)
            .collect()
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

fn view_match_score(candidate: &ViewCandidate, query: &str) -> Option<(u8, usize)> {
    if query.is_empty() {
        return Some((10, 0));
    }

    let alias = candidate
        .alias
        .as_deref()
        .unwrap_or_default()
        .to_lowercase();
    let view_ref = candidate.view_ref.to_lowercase();
    let plugin_name = candidate.plugin_name.to_lowercase();
    let fields = [&alias, &view_ref, &plugin_name];
    if !query
        .split_whitespace()
        .all(|token| fields.iter().any(|field| field.contains(token)))
    {
        return None;
    }

    let score = if alias == query {
        0
    } else if view_ref == query {
        1
    } else if alias.starts_with(query) {
        2
    } else if view_ref.starts_with(query) {
        3
    } else if plugin_name.starts_with(query) {
        4
    } else if alias.contains(query) {
        5
    } else if view_ref.contains(query) {
        6
    } else {
        7
    };
    Some((score, query.len()))
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

    #[test]
    fn completes_all_views_by_alias_and_reference() {
        let router = Router::new(&config());
        let matches = router.complete_views("det");
        assert_eq!(
            matches
                .iter()
                .map(|candidate| candidate.view_ref.as_str())
                .collect::<Vec<_>>(),
            vec!["package-a:view2"]
        );
        assert_eq!(
            router.complete_views("package-a:")[0].view_ref,
            "package-a:default"
        );
    }

    #[test]
    fn completion_prefers_exact_aliases() {
        let router = Router::new(&config());
        let matches = router.complete_views("temp");
        assert_eq!(matches[0].alias.as_deref(), Some("temp"));
        assert_eq!(matches[0].view_ref, "package-a:default");
    }
}
