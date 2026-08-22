use crate::config::{Config, ViewRef};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RouteResolution {
    NotMatched,
    Current { query: String },
    Navigate { target: ViewRef, query: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouteDisplay {
    pub(crate) view_ref: ViewRef,
    pub(crate) alias: Option<String>,
}

impl RouteDisplay {
    pub(crate) fn label(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.view_ref)
    }
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
}

#[derive(Debug, Clone)]
pub(crate) struct Router {
    views: BTreeSet<ViewRef>,
    aliases: BTreeMap<String, ViewRef>,
    display: BTreeMap<ViewRef, RouteDisplay>,
    candidates: Vec<ViewCandidate>,
}

impl Router {
    pub(crate) fn new(config: &Config) -> Self {
        let views = config
            .iter_views()
            .map(|(view_ref, _)| view_ref.clone())
            .collect::<BTreeSet<_>>();
        let mut aliases = BTreeMap::<String, ViewRef>::new();
        let mut display = BTreeMap::new();
        let mut candidates = Vec::with_capacity(config.view_count());
        for (view_ref, view) in config.iter_views() {
            let plugin = view_ref
                .split_once(':')
                .map(|(plugin, _)| plugin)
                .unwrap_or(view_ref);
            let plugin_name = config
                .plugin_display_name(plugin)
                .unwrap_or(plugin)
                .to_string();
            if let Some(alias) = &view.alias {
                aliases.insert(alias.clone(), view_ref.clone());
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
                engine_type: view.selected_engine_type().to_string(),
            });
        }
        candidates.sort_by(|left, right| left.view_ref.cmp(&right.view_ref));
        Self {
            views,
            aliases,
            display,
            candidates,
        }
    }

    pub(crate) fn complete_views(&self, query: &str, current_view_ref: &str) -> Vec<ViewCandidate> {
        let query = query.trim().to_lowercase();
        let mut matches = self
            .candidates
            .iter()
            .filter(|candidate| candidate.view_ref != current_view_ref)
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

        let target = if selector.contains(':') {
            if !valid_view_ref(selector) || !self.views.contains(selector) {
                return RouteResolution::NotMatched;
            }
            selector
        } else {
            let Some(target) = self.aliases.get(selector) else {
                return RouteResolution::NotMatched;
            };
            target
        };

        if target == current_view_ref {
            RouteResolution::Current {
                query: query.to_string(),
            }
        } else {
            RouteResolution::Navigate {
                target: target.to_string(),
                query: query.to_string(),
            }
        }
    }

    pub(crate) fn recognized_prefix_end(
        &self,
        current_view_ref: &str,
        input: &str,
    ) -> Option<usize> {
        let (selector, _) = split_selector(input)?;
        match self.resolve(current_view_ref, input) {
            RouteResolution::Current { .. } | RouteResolution::Navigate { .. } => {
                Some(selector.len())
            }
            RouteResolution::NotMatched => None,
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

fn view_match_score(candidate: &ViewCandidate, query: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(10);
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

    Some(if alias == query {
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
    })
}

fn split_selector(input: &str) -> Option<(&str, &str)> {
    input
        .split_once(char::is_whitespace)
        .map(|(selector, query)| (selector, query.trim_start()))
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
    use crate::config::{ENGINE_PICKER, EngineOptions, EngineSpec, PluginMetadata, View};
    use serde_json::Value;
    use std::collections::BTreeMap;

    fn view(alias: Option<&str>) -> View {
        View {
            engine: EngineSpec {
                engine_type: ENGINE_PICKER.to_string(),
                config: EngineOptions::default(),
            },
            alias: alias.map(str::to_string),
            run_shell: None,
            cancel_exit_code: None,
            query: None,
            keymap: None,
            commands: BTreeMap::new(),
        }
    }

    fn config() -> Config {
        Config::test_new(
            Some("core:default".to_string()),
            BTreeMap::from([
                ("core:default".to_string(), view(None)),
                ("package-a:default".to_string(), view(Some("temp"))),
                ("package-a:view2".to_string(), view(Some("detail"))),
            ]),
            BTreeMap::from([
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
            BTreeMap::new(),
            Value::Object(serde_json::Map::new()),
        )
        .unwrap()
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
            RouteResolution::NotMatched
        );
        assert_eq!(
            router.resolve("core:default", "package-a:view2 "),
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
    fn completion_searches_aliases_references_and_plugin_names() {
        let router = Router::new(&config());
        assert_eq!(
            router.complete_views("det", "core:default")[0].view_ref,
            "package-a:view2"
        );
        assert_eq!(
            router.complete_views("package-a:", "core:default")[0].view_ref,
            "package-a:default"
        );
        assert_eq!(router.complete_views("template", "core:default").len(), 2);
        assert_eq!(
            router.complete_views("temp", "core:default")[0]
                .alias
                .as_deref(),
            Some("temp")
        );
        assert!(router.complete_views("core", "core:default").is_empty());
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
    fn display_contains_the_optional_alias() {
        let router = Router::new(&config());
        let aliased = router.display("package-a:default");
        assert_eq!(aliased.view_ref, "package-a:default");
        assert_eq!(aliased.alias.as_deref(), Some("temp"));
        let canonical = router.display("core:default");
        assert_eq!(canonical.view_ref, "core:default");
        assert_eq!(canonical.alias, None);
    }
}
