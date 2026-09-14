use crate::workflow::config::{CompiledConfig, ViewRef};
use std::collections::{BTreeMap, BTreeSet};

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
    pub(crate) workflow_name: String,
    pub(crate) engine_type: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Router {
    views: BTreeSet<ViewRef>,
    aliases: BTreeMap<String, ViewRef>,
    display: BTreeMap<ViewRef, RouteDisplay>,
    candidates: Vec<ViewCandidate>,
}

impl Router {
    pub(crate) fn new(config: &CompiledConfig) -> Self {
        let views = config
            .iter_views()
            .map(|(view_ref, _)| view_ref.clone())
            .collect::<BTreeSet<_>>();
        let mut aliases = BTreeMap::<String, ViewRef>::new();
        let mut display = BTreeMap::new();
        let mut candidates = Vec::with_capacity(config.view_count());
        for (view_ref, view) in config.iter_public_views() {
            let workflow = view_ref
                .split_once(':')
                .map(|(workflow, _)| workflow)
                .unwrap_or(view_ref);
            let workflow_name = config
                .workflow_display_name(workflow)
                .unwrap_or(workflow)
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
                workflow_name,
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

    pub(crate) fn resolve_selector(&self, selector: &str) -> Option<ViewRef> {
        if selector.contains(':') {
            return (valid_view_ref(selector) && self.views.contains(selector))
                .then(|| selector.to_string());
        }
        self.aliases.get(selector).cloned()
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
    let workflow_name = candidate.workflow_name.to_lowercase();
    let fields = [&alias, &view_ref, &workflow_name];
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
    } else if workflow_name.starts_with(query) {
        4
    } else if alias.contains(query) {
        5
    } else if view_ref.contains(query) {
        6
    } else {
        7
    })
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

    fn router() -> Router {
        let config = crate::workflow::config::load_test_fixture().unwrap();
        Router::new(&config)
    }

    #[test]
    fn resolves_alias_and_canonical_view_references() {
        let router = router();
        assert_eq!(router.resolve_selector("sys"), Some("sys:main".to_string()));
        assert_eq!(
            router.resolve_selector("sys:main"),
            Some("sys:main".to_string())
        );
        assert_eq!(router.resolve_selector("unknown"), None);
        assert_eq!(router.resolve_selector("sys:missing"), None);
    }

    #[test]
    fn completion_searches_aliases_references_and_workflow_names() {
        let router = router();
        assert_eq!(
            router.complete_views("sys", "core:default")[0].view_ref,
            "sys:main"
        );
        assert_eq!(
            router.complete_views("system", "core:default")[0].view_ref,
            "sys:main"
        );
        assert!(
            router
                .complete_views("core:default", "core:default")
                .is_empty()
        );
    }

    #[test]
    fn display_contains_the_optional_alias() {
        let router = router();
        let aliased = router.display("sys:main");
        assert_eq!(aliased.view_ref, "sys:main");
        assert_eq!(aliased.alias.as_deref(), Some("sys"));
        assert_eq!(aliased.label(), "sys");
        let canonical = router.display("core:default");
        assert_eq!(canonical.alias, None);
        assert_eq!(canonical.label(), "core:default");
    }
}
