use super::schema::{
    QueryType, ViewQuerySchema, compile_query, render_input_value, validate_required,
};
use crate::expression::{EvaluationStage, TemplateRegistry};
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default)]
pub(crate) struct StateRegistry {
    views: BTreeMap<String, ViewQuerySchema>,
}

impl StateRegistry {
    #[cfg(test)]
    pub(crate) fn compile(config: &Value) -> Result<Self> {
        let templates = TemplateRegistry::compile_json_tree(config)?;
        Self::compile_with_templates(config, &templates)
    }

    pub(crate) fn compile_with_templates(
        config: &Value,
        templates: &TemplateRegistry,
    ) -> Result<Self> {
        let plugins = config
            .get("plugins")
            .and_then(Value::as_object)
            .context("configuration plugins must be an object")?;
        let mut views = BTreeMap::new();
        for (plugin_id, plugin) in plugins {
            let view_values = plugin
                .get("views")
                .and_then(Value::as_object)
                .with_context(|| format!("plugin {:?} views must be an object", plugin_id))?;
            for (view_name, view) in view_values {
                let view_ref = format!("{plugin_id}:{view_name}");
                if let Some(query) = view.get("query") {
                    templates.requirements_for_value(query)?.validate_stage(
                        EvaluationStage::Bootstrap,
                        &format!("view {view_ref:?} query schema"),
                    )?;
                }
                views.insert(
                    view_ref,
                    ViewQuerySchema {
                        query: compile_query(view)?,
                    },
                );
            }
        }
        Ok(Self { views })
    }

    pub(crate) fn instantiate(&self, view_ref: &str) -> Result<StateInstance> {
        let Some(schema) = self.views.get(view_ref) else {
            return Ok(StateInstance {
                view_ref: view_ref.to_string(),
                values: BTreeMap::new(),
                revision: 0,
            });
        };
        let values = if schema.query.plain {
            BTreeMap::new()
        } else {
            schema
                .query
                .fields
                .iter()
                .map(|(name, field)| (name.clone(), field.default.clone()))
                .collect()
        };
        Ok(StateInstance {
            view_ref: view_ref.to_string(),
            values,
            revision: 0,
        })
    }

    pub(crate) fn bind_cli(&self, view_ref: &str, arguments: &[String]) -> Result<StateInstance> {
        let mut state = self.instantiate(view_ref)?;
        let schema = &self.schema_for(view_ref, &state)?.query;
        if schema.plain {
            if arguments.is_empty() {
                return Ok(state);
            }
            bail!(
                "view {:?} does not declare keyed query parameters",
                view_ref
            );
        }
        let mut assigned = BTreeSet::new();
        for argument in arguments {
            let raw = argument.strip_prefix("--").with_context(|| {
                format!(
                    "view {:?} does not accept positional argument {:?}",
                    view_ref, argument
                )
            })?;
            if raw.is_empty() {
                bail!("view {:?} does not accept positional arguments", view_ref);
            }
            let (name, value, typed) = if let Some((name, source)) = raw.split_once(":=") {
                (
                    name,
                    serde_json::from_str(source).with_context(|| {
                        format!("query parameter --{name}:= contains invalid JSON")
                    })?,
                    true,
                )
            } else if let Some((name, source)) = raw.split_once('=') {
                (name, Value::String(source.to_string()), false)
            } else if let Some(name) = raw.strip_prefix("no-") {
                (name, Value::Bool(false), true)
            } else {
                (raw, Value::Bool(true), true)
            };
            if name.is_empty()
                || !name.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                })
            {
                bail!("invalid query parameter name {:?}", name);
            }
            let field = schema.fields.get(name).with_context(|| {
                format!(
                    "view {:?} does not declare query parameter --{}",
                    view_ref, name
                )
            })?;
            if !assigned.insert(name.to_string()) {
                bail!("query parameter --{} may be specified only once", name);
            }
            let value = if typed {
                value
            } else {
                field
                    .value_type
                    .parse_cli(value.as_str().expect("CLI input is a string"))?
            };
            field
                .validate(&value)
                .with_context(|| format!("invalid query parameter --{}", name))?;
            state.values.insert(name.to_string(), value);
        }
        validate_required(schema, &state.values)?;
        if !assigned.is_empty() {
            state.revision = state.revision.wrapping_add(1);
        }
        Ok(state)
    }

    pub(crate) fn render_input(&self, state: &StateInstance) -> Result<String> {
        if self.implicit_plain(state) {
            return Ok(state
                .values
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string());
        }
        let schema = &self.schema_for(&state.view_ref, state)?.query;
        if schema.plain {
            return Ok(state
                .values
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string());
        }
        if schema.input_order.is_empty() {
            return Ok(String::new());
        }
        if schema.input_order.len() == 1 {
            let name = &schema.input_order[0];
            let field = schema.fields.get(name).expect("query field disappeared");
            if field.value_type == QueryType::String {
                return Ok(state
                    .values
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string());
            }
        }
        Ok(schema
            .input_order
            .iter()
            .map(|name| {
                let field = schema.fields.get(name).expect("query field disappeared");
                render_input_value(state.values.get(name).unwrap_or(&field.default))
            })
            .collect::<Vec<_>>()
            .join(" "))
    }

    pub(crate) fn update_input(&self, state: &mut StateInstance, source: &str) -> Result<bool> {
        if self.implicit_plain(state) {
            let value = Value::String(source.to_string());
            if state.values.get("query") == Some(&value) {
                return Ok(false);
            }
            state.values.insert("query".to_string(), value);
            state.revision = state.revision.wrapping_add(1);
            return Ok(true);
        }
        let schema = &self.schema_for(&state.view_ref, state)?.query;
        if schema.plain {
            let value = Value::String(source.to_string());
            if state.values.get("query") == Some(&value) {
                return Ok(false);
            }
            state.values.insert("query".to_string(), value);
            state.revision = state.revision.wrapping_add(1);
            return Ok(true);
        }
        if schema.input_order.is_empty() {
            return Ok(false);
        }
        let tokens = if schema.input_order.len() == 1
            && schema.fields[&schema.input_order[0]].value_type == QueryType::String
        {
            vec![source.to_string()]
        } else {
            shell_words::split(source).context("query input contains invalid quoting")?
        };
        if tokens.len() > schema.input_order.len() {
            bail!(
                "query input expected at most {} values, received {}",
                schema.input_order.len(),
                tokens.len()
            );
        }
        let mut values = state.values.clone();
        for (index, name) in schema.input_order.iter().enumerate() {
            let field = schema.fields.get(name).expect("query field disappeared");
            let value = match tokens.get(index) {
                Some(token) => field
                    .value_type
                    .parse_text(token)
                    .with_context(|| format!("query input field {:?} is invalid", name))?,
                None if field.required => bail!("query input is missing required field {:?}", name),
                None => field.default.clone(),
            };
            field.validate(&value)?;
            values.insert(name.clone(), value);
        }
        if values == state.values {
            return Ok(false);
        }
        state.values = values;
        state.revision = state.revision.wrapping_add(1);
        Ok(true)
    }

    pub(crate) fn bind_feed_input(&self, state: &mut StateInstance, source: &str) -> Result<bool> {
        if source.is_empty() {
            return Ok(false);
        }
        if !self.implicit_plain(state) {
            let schema = &self.schema_for(&state.view_ref, state)?.query;
            if !schema.plain && schema.input_order.is_empty() {
                bail!("non-empty feed binding cannot be parsed because query input_order is empty");
            }
        }
        self.update_input(state, source)
    }

    pub(crate) fn update_value(&self, state: &mut StateInstance, value: &Value) -> Result<bool> {
        if value.is_null() {
            return Ok(false);
        }
        if self.implicit_plain(state) {
            return self.update_input(
                state,
                value
                    .as_str()
                    .context("string query expects a string value")?,
            );
        }
        if value.is_string() {
            return self.update_input(state, value.as_str().expect("query is a string"));
        }
        let schema = &self.schema_for(&state.view_ref, state)?.query;
        if schema.plain {
            bail!("string query expects a string value")
        }
        let object = value
            .as_object()
            .context("object query expects a JSON object")?;
        for name in object.keys() {
            if !schema.fields.contains_key(name) {
                bail!("query contains unknown parameter {:?}", name);
            }
        }
        let mut values = BTreeMap::new();
        for (name, field) in &schema.fields {
            let value = object.get(name).unwrap_or(&field.default);
            field
                .validate(value)
                .with_context(|| format!("invalid query parameter {:?}", name))?;
            values.insert(name.clone(), value.clone());
        }
        validate_required(schema, &values)?;
        if values == state.values {
            return Ok(false);
        }
        state.values = values;
        state.revision = state.revision.wrapping_add(1);
        Ok(true)
    }

    pub(crate) fn query_value(&self, state: &StateInstance) -> Result<Value> {
        if self.implicit_plain(state) {
            return Ok(state
                .values
                .get("query")
                .cloned()
                .unwrap_or_else(|| Value::String(String::new())));
        }
        let schema = &self.schema_for(&state.view_ref, state)?.query;
        if schema.plain {
            return Ok(state
                .values
                .get("query")
                .cloned()
                .unwrap_or_else(|| Value::String(String::new())));
        }
        Ok(state
            .values
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<Map<_, _>>()
            .into())
    }

    pub(crate) fn validate_instance(&self, state: &StateInstance) -> Result<()> {
        if self.implicit_plain(state) {
            return Ok(());
        }
        let schema = &self.schema_for(&state.view_ref, state)?.query;
        if schema.plain {
            return Ok(());
        }
        validate_required(schema, &state.values)?;
        for (name, field) in &schema.fields {
            field
                .validate(state.values.get(name).unwrap_or(&field.default))
                .with_context(|| format!("invalid query parameter {:?}", name))?;
        }
        Ok(())
    }

    fn implicit_plain(&self, state: &StateInstance) -> bool {
        !self.views.contains_key(&state.view_ref)
    }

    fn schema_for<'a>(
        &'a self,
        view_ref: &str,
        state: &StateInstance,
    ) -> Result<&'a ViewQuerySchema> {
        if state.view_ref != view_ref {
            bail!(
                "query instance for {:?} cannot evaluate view {:?}",
                state.view_ref,
                view_ref
            );
        }
        self.views
            .get(view_ref)
            .with_context(|| format!("view {:?} has no query schema", view_ref))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct StateInstance {
    view_ref: String,
    values: BTreeMap<String, Value>,
    revision: u64,
}

impl StateInstance {
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn view_ref(&self) -> &str {
        &self.view_ref
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Value {
        serde_json::json!({
            "plugins": { "trans": { "views": { "default": {
                "query": {
                    "type": "object",
                    "input_order": ["source", "target", "text"],
                    "source": {"type": "string", "nullable": true},
                    "target": {"type": "string", "nullable": true},
                    "text": {"type": "string", "default": ""}
                }
            }}}}
        })
    }

    #[test]
    fn object_query_uses_defaults_and_materializes_values() {
        let registry = StateRegistry::compile(&config()).unwrap();
        let mut state = registry.instantiate("trans:default").unwrap();
        registry
            .update_input(&mut state, "ja en 'good morning'")
            .unwrap();
        assert_eq!(
            registry.query_value(&state).unwrap(),
            serde_json::json!({"source":"ja", "target":"en", "text":"good morning"})
        );
    }

    #[test]
    fn object_query_can_be_initialized_directly() {
        let registry = StateRegistry::compile(&config()).unwrap();
        let mut state = registry.instantiate("trans:default").unwrap();
        registry
            .update_value(
                &mut state,
                &serde_json::json!({"source":"en", "target":"zh"}),
            )
            .unwrap();
        assert_eq!(registry.render_input(&state).unwrap(), "en zh ''");
        assert_eq!(registry.query_value(&state).unwrap()["text"], "");
    }

    #[test]
    fn cli_parses_untyped_values_using_their_declared_types() {
        let registry = StateRegistry::compile(&serde_json::json!({
            "plugins": {"core": {"views": {"default": {"query": {
                "type":"object", "count":{"type":"integer", "default":1},
                "enabled":{"type":"boolean", "default":false}
            }}}}}
        }))
        .unwrap();
        let state = registry
            .bind_cli("core:default", &["--count=5".into(), "--enabled".into()])
            .unwrap();
        assert_eq!(
            registry.query_value(&state).unwrap(),
            serde_json::json!({"count":5,"enabled":true})
        );
    }

    #[test]
    fn plain_query_is_still_implicit() {
        let config = serde_json::json!({"plugins":{"core":{"views":{"default":{}}}}});
        let registry = StateRegistry::compile(&config).unwrap();
        let mut state = registry.instantiate("core:default").unwrap();
        registry.update_input(&mut state, "needle").unwrap();
        assert_eq!(registry.query_value(&state).unwrap(), "needle");
    }

    #[test]
    fn instance_validation_checks_required_fields_outside_input_order() {
        let registry = StateRegistry::compile(&serde_json::json!({
            "plugins": {"apps": {"views": {"default": {"query": {
                "type": "object",
                "input_order": [],
                "token": {"type": "string"}
            }}}}}
        }))
        .unwrap();
        let state = registry.instantiate("apps:default").unwrap();
        let error = registry
            .validate_instance(&state)
            .expect_err("missing required field must be rejected");
        assert!(error.to_string().contains("--token is required"));
    }

    #[test]
    fn feed_binding_rejects_nonempty_input_without_ordered_fields() {
        let registry = StateRegistry::compile(&serde_json::json!({
            "plugins": {"apps": {"views": {"default": {"query": {
                "type": "object",
                "input_order": [],
                "token": {"type": "string", "default": "fixed"}
            }}}}}
        }))
        .unwrap();
        let mut state = registry.instantiate("apps:default").unwrap();

        let error = registry
            .bind_feed_input(&mut state, "needle")
            .expect_err("non-empty binding needs an ordered query field");

        assert!(error.to_string().contains("input_order is empty"));
        assert_eq!(registry.query_value(&state).unwrap()["token"], "fixed");
    }
}
