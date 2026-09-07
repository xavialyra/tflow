//! Application composition adapters over compiled workflow configuration.

use crate::view::{ParsedQuery, QuerySchema, RouteCandidate, RouteCatalog, RouteTarget};
use anyhow::{Context, Result};
use std::sync::Arc;

/// Read-only adapter over the compiled configuration route index and query registry.
pub(crate) struct CompiledRouteCatalog {
    routes: crate::workflow::navigation::Router,
    config: Arc<crate::workflow::config::CompiledConfig>,
}

impl CompiledRouteCatalog {
    pub(crate) fn new(config: Arc<crate::workflow::config::CompiledConfig>) -> Self {
        Self {
            routes: crate::workflow::navigation::Router::new(&config),
            config,
        }
    }
}

impl RouteCatalog for CompiledRouteCatalog {
    fn resolve(&self, selector: &str) -> Option<RouteTarget> {
        self.routes.resolve_selector(selector).map(|reference| {
            let display = self.routes.display(&reference);
            RouteTarget {
                reference,
                label: Some(display.label().to_string()),
            }
        })
    }

    fn complete(&self, prefix: &str) -> Vec<RouteCandidate> {
        self.routes
            .complete_views(prefix, "")
            .into_iter()
            .map(|candidate| RouteCandidate {
                target: RouteTarget {
                    reference: candidate.view_ref.clone(),
                    label: Some(
                        candidate
                            .alias
                            .clone()
                            .unwrap_or_else(|| candidate.view_ref.clone()),
                    ),
                },
                label: candidate
                    .alias
                    .clone()
                    .unwrap_or_else(|| candidate.view_ref.clone()),
            })
            .collect()
    }

    fn query_schema(&self, target: &str) -> Option<QuerySchema> {
        self.config.view(target).map(|_| QuerySchema {
            id: "query".to_string(),
        })
    }

    fn validate_query(&self, query: &ParsedQuery) -> Result<()> {
        query.validate_shape()?;
        let schema = self
            .query_schema(&query.target)
            .with_context(|| format!("unknown query schema for {:?}", query.target))?;
        anyhow::ensure!(
            schema.id == query.schema,
            "query schema does not match target"
        );
        let mut state = self.config.instantiate_parameters(&query.target)?;
        self.config
            .update_sanitized_initial_parameter_values(&mut state, &query.values)?;
        self.config
            .parameter_binding(&query.target)?
            .validate_instance(&state)?;
        let values = self.config.parameter_values(&state)?;
        anyhow::ensure!(
            values == query.values,
            "parsed query values do not match target schema"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_config_route_catalog_validates_without_invocation_state() {
        let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let parameters = config.instantiate_parameters("core:default").unwrap();
        let values = config.parameter_values(&parameters).unwrap();
        let catalog = CompiledRouteCatalog::new(config);
        assert!(
            catalog
                .validate_query(&ParsedQuery::new("core:default", "query", values))
                .is_ok()
        );
    }
}
