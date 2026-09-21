//! Application composition adapters over compiled workflow configuration.

use crate::view::{ParsedQuery, QuerySchema, RouteCatalog, ViewLocation};
use anyhow::{Context, Result};
use std::sync::Arc;

/// Read-only adapter that resolves navigation targets and validates query
/// schemas against the compiled configuration.
pub(crate) struct CompiledRouteCatalog {
    config: Arc<crate::workflow::config::CompiledConfig>,
}

impl CompiledRouteCatalog {
    pub(crate) fn new(config: Arc<crate::workflow::config::CompiledConfig>) -> Self {
        Self { config }
    }
}

impl RouteCatalog for CompiledRouteCatalog {
    fn resolve(&self, selector: &str) -> Option<ViewLocation> {
        // The Router re-resolves targets that command preparation already
        // canonicalized; an unknown target is reported by the caller.
        let target = self.config.resolve_view(selector).ok()?;
        let alias = self
            .config
            .public_alias_for_view(&target)
            .map(str::to_string);
        Some(ViewLocation { target, alias })
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

    #[test]
    fn route_resolution_carries_the_public_alias() {
        let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let catalog = CompiledRouteCatalog::new(config);
        let aliased = catalog.resolve("sys").unwrap();
        assert_eq!(aliased.target, "sys:main");
        assert_eq!(aliased.label(), "sys");
        let canonical = catalog.resolve("core:default").unwrap();
        assert_eq!(canonical.label(), "core");
        // Alias-less and internal Views keep their reference as the label.
        assert_eq!(catalog.resolve("sys:output").unwrap().label(), "sys:output");
        assert_eq!(
            catalog.resolve("__commands:main").unwrap().label(),
            "__commands:main"
        );
        assert!(catalog.resolve("unknown").is_none());
    }
}
