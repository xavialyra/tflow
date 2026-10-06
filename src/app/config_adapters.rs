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

    fn default_companion(&self, target: &str) -> Option<String> {
        let view = self.config.view(target)?;
        let companion_target = view.companion.as_ref()?;
        self.config
            .resolve_view_scoped(companion_target, target)
            .ok()
    }

    fn default_companion_info(&self, target: &str) -> Option<crate::view::DefaultCompanionInfo> {
        let view = self.config.view(target)?;
        let companion_target = view.companion.as_ref()?;
        let pkg = crate::workflow::config::package_id(target);

        // 1. Check if companion_target refers to a companion command
        if let Some(cmd) = self.config.find_command(pkg, companion_target)
            && let crate::workflow::config::CommandAction::Companion { payload, .. } = &cmd.action
            && let toml::Value::Table(table) = payload
        {
            let target_name = table
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or(companion_target);
            let resolved_target = self.config.resolve_view_scoped(target_name, target).ok()?;
            let args = table.get("args").or_else(|| table.get("query")).cloned();
            return Some(crate::view::DefaultCompanionInfo {
                target: resolved_target,
                slot: Some(companion_target.clone()),
                args_template: args,
            });
        }

        // 2. Check if companion_target refers to a view companion slot
        if let Some(slot) = view.companions.get(companion_target) {
            let resolved_target = self.config.resolve_view_scoped(&slot.target, target).ok()?;
            Some(crate::view::DefaultCompanionInfo {
                target: resolved_target,
                slot: Some(companion_target.clone()),
                args_template: slot.args.clone(),
            })
        } else {
            let resolved_target = self
                .config
                .resolve_view_scoped(companion_target, target)
                .ok()?;
            Some(crate::view::DefaultCompanionInfo {
                target: resolved_target,
                slot: Some(companion_target.clone()),
                args_template: Some(toml::Value::Table(
                    [(
                        "item".to_string(),
                        toml::Value::String("$selection".to_string()),
                    )]
                    .into_iter()
                    .collect(),
                )),
            })
        }
    }

    fn parse_query(&self, target: &str, query: Option<serde_json::Value>) -> Result<ParsedQuery> {
        let mut state = self.config.instantiate_parameters(target)?;
        if let Some(ref q) = query
            && !q.is_null()
        {
            if q.is_string() {
                self.config
                    .update_sanitized_initial_parameter_values(&mut state, q)?;
            } else if let Some(_obj) = q.as_object() {
                let is_plain = self
                    .config
                    .query_definition(target)
                    .map(|d| d.get("type").and_then(|t| t.as_str()) == Some("string"))
                    .unwrap_or(true);
                if is_plain {
                    let str_val = serde_json::Value::String(q.to_string());
                    self.config
                        .update_sanitized_initial_parameter_values(&mut state, &str_val)?;
                } else {
                    self.config
                        .update_sanitized_initial_parameter_values(&mut state, q)?;
                }
            } else {
                self.config
                    .update_sanitized_initial_parameter_values(&mut state, q)?;
            }
        }
        let values = self.config.parameter_values(&state)?;
        Ok(ParsedQuery::new(target, "query", values))
    }

    fn default_query(&self, target: &str) -> Result<ParsedQuery> {
        let state = self.config.instantiate_parameters(target)?;
        let values = self.config.parameter_values(&state)?;
        Ok(ParsedQuery::new(target, "query", values))
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
        // Alias-less Views keep their reference as the label.
        assert_eq!(catalog.resolve("sys:output").unwrap().label(), "sys:output");
        assert!(catalog.resolve("unknown").is_none());
    }
}
