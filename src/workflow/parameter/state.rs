use super::schema::{
    ParameterSchema, ParameterType, ViewParameterSchema, compile_parameter_schema,
    render_input_value, validate_required,
};
use crate::input::InputSourceIdentity;
use crate::terminal::sanitize_terminal_text;
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub(crate) struct ParameterRegistry {
    views: BTreeMap<String, ViewParameterSchema>,
}

impl ParameterRegistry {
    #[cfg(test)]
    pub(crate) fn compile(config: &Value) -> Result<Self> {
        let workflows = config
            .get("workflows")
            .and_then(Value::as_object)
            .context("configuration workflows must be an object")?;
        let mut views = BTreeMap::new();
        for (workflow_id, workflow) in workflows {
            let view_values = workflow
                .get("views")
                .and_then(Value::as_object)
                .with_context(|| format!("workflow {:?} views must be an object", workflow_id))?;
            for (view_name, view) in view_values {
                let view_ref = format!("{workflow_id}:{view_name}");
                views.insert(
                    view_ref,
                    ViewParameterSchema {
                        schema: compile_parameter_schema(view)?,
                    },
                );
            }
        }
        Ok(Self { views })
    }

    pub(crate) fn compile_view_queries(
        queries: impl IntoIterator<Item = (String, Option<toml::Table>)>,
    ) -> Result<Self> {
        let mut views = BTreeMap::new();
        for (view_ref, query) in queries {
            let schema = match query {
                Some(query) => {
                    let query = serde_json::to_value(query)
                        .context("view query cannot be represented as JSON")?;
                    compile_parameter_schema(&serde_json::json!({ "query": query }))?
                }
                None => compile_parameter_schema(&Value::Null)?,
            };
            views.insert(view_ref, ViewParameterSchema { schema });
        }
        Ok(Self { views })
    }

    pub(crate) fn parameter_binding(self: &Arc<Self>, view_ref: &str) -> Result<ParameterBinding> {
        let schema = self
            .views
            .get(view_ref)
            .with_context(|| format!("view {:?} has no query schema", view_ref))?
            .schema
            .clone();
        Ok(ParameterBinding {
            registry: Arc::clone(self),
            view_ref: view_ref.to_string(),
            schema,
        })
    }

    pub(crate) fn instantiate(&self, view_ref: &str) -> Result<ParameterState> {
        let schema = self
            .views
            .get(view_ref)
            .with_context(|| format!("view {:?} has no query schema", view_ref))?;
        let values = if schema.schema.plain {
            BTreeMap::new()
        } else {
            schema
                .schema
                .fields
                .iter()
                .filter(|(_, field)| !field.required)
                .map(|(name, field)| (name.clone(), field.default.clone()))
                .collect()
        };
        let mut state = ParameterState {
            view_ref: view_ref.to_string(),
            values,
            revision: 0,
            raw_input: String::new(),
            input_rejected: false,
        };
        state.raw_input = self.render_input(&state)?;
        Ok(state)
    }

    pub(crate) fn bind_cli(&self, view_ref: &str, arguments: &[String]) -> Result<ParameterState> {
        let mut state = self.instantiate(view_ref)?;
        let schema = &self.schema_for(view_ref, &state)?.schema;
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
        state.raw_input = self.render_input(&state)?;
        Ok(state)
    }

    pub(crate) fn render_input(&self, state: &ParameterState) -> Result<String> {
        let schema = &self.schema_for(&state.view_ref, state)?.schema;
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
            if field.value_type == ParameterType::String {
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

    pub(crate) fn update_input(&self, state: &mut ParameterState, source: &str) -> Result<bool> {
        self.update_input_with_required_validation(state, source, true)
    }

    pub(crate) fn update_initial_input(
        &self,
        state: &mut ParameterState,
        source: &str,
    ) -> Result<bool> {
        self.update_input_with_required_validation(state, source, false)
    }

    fn update_input_with_required_validation(
        &self,
        state: &mut ParameterState,
        source: &str,
        validate_required_fields: bool,
    ) -> Result<bool> {
        let schema = &self.schema_for(&state.view_ref, state)?.schema;
        if schema.plain {
            let value = Value::String(source.to_string());
            if state.values.get("query") == Some(&value) {
                state.raw_input = source.to_string();
                return Ok(false);
            }
            state.values.insert("query".to_string(), value);
            state.raw_input = source.to_string();
            state.revision = state.revision.wrapping_add(1);
            return Ok(true);
        }
        if schema.input_order.is_empty() {
            if !validate_required_fields {
                if state.raw_input != source {
                    state.raw_input = source.to_string();
                }
                return Ok(false);
            }
            if source.is_empty() {
                validate_required(schema, &state.values)?;
                state.raw_input = source.to_string();
                return Ok(false);
            }
            bail!("query input cannot be parsed because input_order is empty");
        }
        let tokens = if !validate_required_fields && source.is_empty() {
            Vec::new()
        } else if schema.input_order.len() == 1
            && schema.fields[&schema.input_order[0]].value_type == ParameterType::String
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
                None if field.required && validate_required_fields => {
                    bail!("query input is missing required field {:?}", name)
                }
                None if field.required && source.is_empty() => {
                    values.remove(name);
                    continue;
                }
                None => field.default.clone(),
            };
            field.validate(&value)?;
            values.insert(name.clone(), value);
        }
        if validate_required_fields {
            validate_required(schema, &values)?;
        }
        if values == state.values {
            state.raw_input = source.to_string();
            return Ok(false);
        }
        state.values = values;
        state.raw_input = source.to_string();
        state.revision = state.revision.wrapping_add(1);
        Ok(true)
    }

    #[cfg(test)]
    pub(crate) fn update_value(&self, state: &mut ParameterState, value: &Value) -> Result<bool> {
        if value.is_null() {
            return Ok(false);
        }
        if value.is_string() {
            return self.update_input(state, value.as_str().expect("query is a string"));
        }
        let schema = &self.schema_for(&state.view_ref, state)?.schema;
        if schema.plain {
            bail!("string query expects a string value")
        }
        let object = value
            .as_object()
            .context("object query expects a JSON object")?;
        for name in object.keys() {
            if !schema.fields.contains_key(name) && !name.starts_with("__") {
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
            state.raw_input = self.render_input(state)?;
            return Ok(false);
        }
        state.values = values;
        state.raw_input = self.render_input(state)?;
        state.revision = state.revision.wrapping_add(1);
        Ok(true)
    }

    // Initialization may carry a partial typed object. Keep omitted required
    // fields absent while retaining strict unknown-field and value validation.
    pub(crate) fn update_initial_value(
        &self,
        state: &mut ParameterState,
        value: &Value,
    ) -> Result<bool> {
        if value.is_null() {
            return Ok(false);
        }
        if value.is_string() {
            return self.update_initial_input(state, value.as_str().expect("query is a string"));
        }
        let schema = &self.schema_for(&state.view_ref, state)?.schema;
        if schema.plain {
            bail!("string query expects a string value")
        }
        let object = value
            .as_object()
            .context("object query expects a JSON object")?;
        for name in object.keys() {
            if !schema.fields.contains_key(name) && !name.starts_with("__") {
                bail!("query contains unknown parameter {:?}", name);
            }
        }
        let mut values = state.values.clone();
        for (name, value) in object {
            if name.starts_with("__") {
                continue;
            }
            let field = schema
                .fields
                .get(name)
                .expect("unknown query field was checked above");
            field
                .validate(value)
                .with_context(|| format!("invalid query parameter {:?}", name))?;
            values.insert(name.clone(), value.clone());
        }
        if values == state.values {
            state.raw_input = self.render_input(state)?;
            return Ok(false);
        }
        state.values = values;
        state.raw_input = self.render_input(state)?;
        state.revision = state.revision.wrapping_add(1);
        Ok(true)
    }

    pub(crate) fn parameter_values(&self, state: &ParameterState) -> Result<Value> {
        let schema = &self.schema_for(&state.view_ref, state)?.schema;
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

    pub(crate) fn validate_instance(&self, state: &ParameterState) -> Result<()> {
        let schema = &self.schema_for(&state.view_ref, state)?.schema;
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

    fn schema_for<'a>(
        &'a self,
        view_ref: &str,
        state: &ParameterState,
    ) -> Result<&'a ViewParameterSchema> {
        if state.view_ref != view_ref {
            bail!(
                "query instance for {:?} cannot be used for view {:?}",
                state.view_ref,
                view_ref
            );
        }
        self.views
            .get(view_ref)
            .with_context(|| format!("view {:?} has no query schema", view_ref))
    }
}

fn sanitize_parameter_value(value: &Value) -> Value {
    match value {
        Value::String(value) => Value::String(sanitize_terminal_text(value)),
        Value::Array(values) => Value::Array(values.iter().map(sanitize_parameter_value).collect()),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(name, value)| (name.clone(), sanitize_parameter_value(value)))
                .collect(),
        ),
        value => value.clone(),
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ParameterState {
    view_ref: String,
    values: BTreeMap<String, Value>,
    revision: u64,
    raw_input: String,
    input_rejected: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParameterSnapshot {
    values: Value,
    raw_input: String,
    source: InputSourceIdentity,
    revision: u64,
}

impl ParameterSnapshot {
    pub(crate) fn from_parts(
        values: Value,
        raw_input: String,
        source: InputSourceIdentity,
        revision: u64,
    ) -> Self {
        Self {
            values,
            raw_input,
            source,
            revision,
        }
    }

    pub(crate) fn values(&self) -> &Value {
        &self.values
    }

    pub(crate) fn raw_input(&self) -> &str {
        &self.raw_input
    }

    pub(crate) fn source(&self) -> InputSourceIdentity {
        self.source
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ParameterBinding {
    registry: Arc<ParameterRegistry>,
    view_ref: String,
    schema: ParameterSchema,
}

impl ParameterBinding {
    #[cfg(test)]
    pub(crate) fn schema(&self) -> &ParameterSchema {
        &self.schema
    }

    pub(crate) fn parameter_values(&self, state: &ParameterState) -> Result<Value> {
        anyhow::ensure!(
            state.view_ref() == self.view_ref,
            "parameter binding for {:?} cannot read {:?}",
            self.view_ref,
            state.view_ref()
        );
        self.registry.parameter_values(state)
    }

    pub(crate) fn update_sanitized_initial_value(
        &self,
        state: &mut ParameterState,
        value: &Value,
    ) -> Result<bool> {
        anyhow::ensure!(
            state.view_ref() == self.view_ref,
            "parameter binding for {:?} cannot update {:?}",
            self.view_ref,
            state.view_ref()
        );
        if !self.schema.plain
            && let Some(object) = value.as_object()
        {
            for name in object.keys() {
                if !self.schema.fields.contains_key(name) && !name.starts_with("__") {
                    bail!("query contains unknown parameter {:?}", name);
                }
            }
        }
        let mut clean_value = value.clone();
        if let Some(object) = clean_value.as_object_mut() {
            object.retain(|k, _| !k.starts_with("__"));
        }
        let value = sanitize_parameter_value(&clean_value);
        self.registry.update_initial_value(state, &value)
    }

    pub(crate) fn sanitize_typed_values(&self, state: &mut ParameterState) -> Result<bool> {
        let value = self.parameter_values(state)?;
        let sanitized = sanitize_parameter_value(&value);
        if sanitized == value {
            return Ok(false);
        }
        self.registry.update_initial_value(state, &sanitized)
    }

    pub(crate) fn instantiate(&self) -> Result<ParameterState> {
        self.registry.instantiate(&self.view_ref)
    }

    pub(crate) fn state_from_snapshot(
        &self,
        snapshot: &ParameterSnapshot,
        input_rejected: bool,
    ) -> Result<ParameterState> {
        let mut state = self.instantiate()?;
        if self.schema.plain {
            let query = snapshot
                .values
                .as_str()
                .context("string query snapshot must contain a string value")?;
            state
                .values
                .insert("query".to_string(), Value::String(query.to_string()));
        } else {
            let object = snapshot
                .values
                .as_object()
                .context("object query snapshot must be a JSON object")?;
            for name in object.keys() {
                if !self.schema.fields.contains_key(name) && !name.starts_with("__") {
                    bail!("query contains unknown parameter {:?}", name);
                }
            }
            for (name, field) in &self.schema.fields {
                let Some(value) = object.get(name) else {
                    if field.required {
                        continue;
                    }
                    let value = &field.default;
                    field
                        .validate(value)
                        .with_context(|| format!("invalid query parameter {:?}", name))?;
                    state.values.insert(name.clone(), value.clone());
                    continue;
                };
                field
                    .validate(value)
                    .with_context(|| format!("invalid query parameter {:?}", name))?;
                state.values.insert(name.clone(), value.clone());
            }
        }
        // Initialization snapshots may intentionally omit required fields;
        // strict validation remains at user input and typed patch boundaries.
        state.raw_input = snapshot.raw_input.clone();
        state.revision = snapshot.revision;
        state.input_rejected = input_rejected;
        Ok(state)
    }

    pub(crate) fn bind_cli(&self, arguments: &[String]) -> Result<ParameterState> {
        self.registry.bind_cli(&self.view_ref, arguments)
    }

    pub(crate) fn parse_input(&self, state: &mut ParameterState, source: &str) -> Result<bool> {
        anyhow::ensure!(
            state.view_ref() == self.view_ref,
            "parameter binding for {:?} cannot update {:?}",
            self.view_ref,
            state.view_ref()
        );
        self.registry.update_input(state, source)
    }

    pub(crate) fn update_initial_input(
        &self,
        state: &mut ParameterState,
        source: &str,
    ) -> Result<bool> {
        anyhow::ensure!(
            state.view_ref() == self.view_ref,
            "parameter binding for {:?} cannot update {:?}",
            self.view_ref,
            state.view_ref()
        );
        self.registry.update_initial_input(state, source)
    }

    pub(crate) fn render_input(&self, state: &ParameterState) -> Result<String> {
        anyhow::ensure!(
            state.view_ref() == self.view_ref,
            "parameter binding for {:?} cannot render {:?}",
            self.view_ref,
            state.view_ref()
        );
        self.registry.render_input(state)
    }

    pub(crate) fn validate_instance(&self, state: &ParameterState) -> Result<()> {
        anyhow::ensure!(
            state.view_ref() == self.view_ref,
            "parameter binding for {:?} cannot validate {:?}",
            self.view_ref,
            state.view_ref()
        );
        self.registry.validate_instance(state)
    }
}

impl ParameterState {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn view_ref(&self) -> &str {
        &self.view_ref
    }

    pub(crate) fn raw_input(&self) -> &str {
        &self.raw_input
    }

    #[cfg(test)]
    pub(crate) fn input_rejected(&self) -> bool {
        self.input_rejected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Value {
        serde_json::json!({
            "workflows": { "trans": { "views": { "default": {
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
    fn parameter_bindings_copy_only_the_view_local_schema() {
        let registry = Arc::new(ParameterRegistry::compile(&config()).unwrap());
        let first = registry.parameter_binding("trans:default").unwrap();
        let second = registry.parameter_binding("trans:default").unwrap();
        assert_eq!(first.schema().plain, second.schema().plain);
        assert_eq!(first.schema().fields.len(), second.schema().fields.len());
        assert_eq!(first.view_ref, second.view_ref);
    }

    #[test]
    fn object_query_uses_defaults_and_materializes_values() {
        let registry = ParameterRegistry::compile(&config()).unwrap();
        let mut state = registry.instantiate("trans:default").unwrap();
        registry
            .update_input(&mut state, "ja en 'good morning'")
            .unwrap();
        assert_eq!(
            registry.parameter_values(&state).unwrap(),
            serde_json::json!({"source":"ja", "target":"en", "text":"good morning"})
        );
    }

    #[test]
    fn object_query_can_be_initialized_directly() {
        let registry = ParameterRegistry::compile(&config()).unwrap();
        let mut state = registry.instantiate("trans:default").unwrap();
        registry
            .update_value(
                &mut state,
                &serde_json::json!({"source":"en", "target":"zh"}),
            )
            .unwrap();
        assert_eq!(registry.render_input(&state).unwrap(), "en zh ''");
        assert_eq!(registry.parameter_values(&state).unwrap()["text"], "");
    }

    #[test]
    fn cli_parses_untyped_values_using_their_declared_types() {
        let registry = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"core": {"views": {"default": {"query": {
                "type":"object", "count":{"type":"integer", "default":1},
                "enabled":{"type":"boolean", "default":false}
            }}}}}
        }))
        .unwrap();
        let state = registry
            .bind_cli("core:default", &["--count=5".into(), "--enabled".into()])
            .unwrap();
        assert_eq!(
            registry.parameter_values(&state).unwrap(),
            serde_json::json!({"count":5,"enabled":true})
        );
    }

    #[test]
    fn plain_query_is_still_implicit() {
        let config = serde_json::json!({"workflows":{"core":{"views":{"default":{}}}}});
        let registry = ParameterRegistry::compile(&config).unwrap();
        let mut state = registry.instantiate("core:default").unwrap();
        registry.update_input(&mut state, "needle").unwrap();
        assert_eq!(registry.parameter_values(&state).unwrap(), "needle");
    }

    #[test]
    fn unknown_view_references_are_rejected_at_the_parameter_boundary() {
        let registry = Arc::new(
            ParameterRegistry::compile(&serde_json::json!({
                "workflows": {"core": {"views": {"default": {}}}}
            }))
            .unwrap(),
        );
        let error = registry
            .parameter_binding("missing:default")
            .expect_err("unknown views must not receive an implicit schema");
        assert!(error.to_string().contains("missing:default"));
        let error = registry
            .instantiate("missing:default")
            .expect_err("unknown views must not instantiate parameter state");
        assert!(error.to_string().contains("missing:default"));
    }

    #[test]
    fn sanitizing_plain_empty_values_is_a_noop_for_configured_queries() {
        let registry = Arc::new(
            ParameterRegistry::compile(&serde_json::json!({
                "workflows": {"core": {"views": {"default": {
                    "query": {"type": "string"}
                }}}}
            }))
            .unwrap(),
        );
        let mut state = registry.instantiate("core:default").unwrap();
        assert!(
            !registry
                .parameter_binding("core:default")
                .unwrap()
                .sanitize_typed_values(&mut state)
                .unwrap()
        );
        assert_eq!(state.revision(), 0);
        assert_eq!(state.raw_input(), "");
        assert!(!state.values.contains_key("query"));
    }

    #[test]
    fn sanitizing_terminal_controls_updates_typed_values_once() {
        let registry = Arc::new(
            ParameterRegistry::compile(&serde_json::json!({
                "workflows": {"core": {"views": {"default": {
                    "query": {"type": "string"}
                }}}}
            }))
            .unwrap(),
        );
        let binding = registry.parameter_binding("core:default").unwrap();
        let mut state = registry.instantiate("core:default").unwrap();
        let value = "before\u{1b}[31mred\u{1b}[0m";
        state
            .values
            .insert("query".to_string(), Value::String(value.to_string()));
        state.raw_input = value.to_string();

        assert!(binding.sanitize_typed_values(&mut state).unwrap());
        assert_eq!(state.revision(), 1);
        assert_eq!(state.raw_input(), "beforered");
        assert_eq!(state.values["query"], "beforered");

        assert!(!binding.sanitize_typed_values(&mut state).unwrap());
        assert_eq!(state.revision(), 1);
    }

    #[test]
    fn sanitizing_nested_objects_preserves_keys_and_entries() {
        let registry = Arc::new(
            ParameterRegistry::compile(&serde_json::json!({
                "workflows": {"data": {"views": {"default": {
                    "query": {
                        "type": "object",
                        "payload": {"type": "object", "default": {}}
                    }
                }}}}
            }))
            .unwrap(),
        );
        let binding = registry.parameter_binding("data:default").unwrap();
        let mut state = registry.instantiate("data:default").unwrap();
        let input = serde_json::json!({
            "payload": {
                "a\u{1b}[31m": "first\u{1b}[32m",
                "a": "second\u{1b}[0m",
                "nested": {"key\u{1b}[34m": "third\u{1b}[0m"}
            }
        });

        assert!(
            binding
                .update_sanitized_initial_value(&mut state, &input)
                .unwrap()
        );
        let payload = registry.parameter_values(&state).unwrap()["payload"]
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(payload.len(), 3);
        assert_eq!(payload["a\u{1b}[31m"], "first");
        assert_eq!(payload["a"], "second");
        assert_eq!(payload["nested"]["key\u{1b}[34m"], "third");
    }

    #[test]
    fn instance_validation_checks_required_fields_outside_input_order() {
        let registry = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"apps": {"views": {"default": {"query": {
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
    fn initial_input_allows_missing_required_fields_but_user_input_does_not() {
        let registry = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"apps": {"views": {"default": {"query": {
                "type": "object",
                "input_order": ["visible"],
                "visible": {"type": "string", "default": ""},
                "token": {"type": "string"}
            }}}}}
        }))
        .unwrap();
        let mut state = registry.instantiate("apps:default").unwrap();

        assert!(!registry.update_initial_input(&mut state, "").unwrap());
        assert_eq!(registry.render_input(&state).unwrap(), "");

        let error = registry
            .update_input(&mut state, "")
            .expect_err("user input must reject a missing required field");
        assert!(error.to_string().contains("--token is required"));
    }

    #[test]
    fn feed_binding_rejects_nonempty_input_without_ordered_fields() {
        let registry = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"apps": {"views": {"default": {"query": {
                "type": "object",
                "input_order": [],
                "token": {"type": "string", "default": "fixed"}
            }}}}}
        }))
        .unwrap();
        let mut state = registry.instantiate("apps:default").unwrap();

        let error = registry
            .update_input(&mut state, "needle")
            .expect_err("normal user input needs an ordered query field");
        assert!(error.to_string().contains("input_order is empty"));
        assert_eq!(registry.parameter_values(&state).unwrap()["token"], "fixed");
    }

    #[test]
    fn nullable_and_json_fields_keep_the_canonical_input_projection() {
        let registry = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"data": {"views": {"default": {"query": {
                "type": "object",
                "input_order": ["maybe", "items", "object"],
                "maybe": {"type": "string", "nullable": true},
                "items": {"type": "array<string>", "default": ["one", "two"]},
                "object": {"type": "object", "default": {}}
            }}}}}
        }))
        .unwrap();
        let state = registry.instantiate("data:default").unwrap();
        assert_eq!(registry.render_input(&state).unwrap(), "'' one,two {}");

        let state = registry
            .bind_cli("data:default", &["--items=one,two".into()])
            .unwrap();
        assert_eq!(
            registry.parameter_values(&state).unwrap()["items"],
            serde_json::json!(["one", "two"])
        );
        assert_eq!(registry.render_input(&state).unwrap(), "'' one,two {}");
    }

    #[test]
    fn interactive_input_rejects_array_and_object_fields_with_json_error() {
        let registry = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"data": {"views": {"default": {"query": {
                "type": "object",
                "input_order": ["items"],
                "items": {"type": "array<string>", "default": []}
            }}}}}
        }))
        .unwrap();
        let mut state = registry.instantiate("data:default").unwrap();
        let error = registry
            .update_input(&mut state, "one,two")
            .expect_err("interactive array input must remain JSON-only");
        assert!(format!("{error:#}").contains("array<string> input requires JSON"));

        let registry = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"data": {"views": {"default": {"query": {
                "type": "object",
                "input_order": ["object"],
                "object": {"type": "object", "default": {}}
            }}}}}
        }))
        .unwrap();
        let mut state = registry.instantiate("data:default").unwrap();
        let error = registry
            .update_input(&mut state, "{}")
            .expect_err("interactive object input must remain JSON-only");
        assert!(format!("{error:#}").contains("object input requires JSON"));
    }

    #[test]
    fn nullable_empty_input_token_remains_an_empty_string() {
        let registry = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"data": {"views": {"default": {"query": {
                "type": "object",
                "input_order": ["maybe", "other"],
                "maybe": {"type": "string", "nullable": true},
                "other": {"type": "string", "default": ""}
            }}}}}
        }))
        .unwrap();
        let mut state = registry.instantiate("data:default").unwrap();
        registry.update_input(&mut state, "'' other").unwrap();
        assert_eq!(registry.parameter_values(&state).unwrap()["maybe"], "");
    }

    #[test]
    fn typed_json_input_still_accepts_array_and_object_fields() {
        let registry = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"data": {"views": {"default": {"query": {
                "type": "object",
                "items": {"type": "array<string>"},
                "object": {"type": "object"}
            }}}}}
        }))
        .unwrap();
        let state = registry
            .bind_cli(
                "data:default",
                &[
                    "--items:=[\"one\",\"two\"]".into(),
                    "--object:={\"key\":\"value\"}".into(),
                ],
            )
            .unwrap();
        assert_eq!(
            registry.parameter_values(&state).unwrap(),
            serde_json::json!({"items":["one", "two"], "object":{"key":"value"}})
        );

        let mut state = registry.instantiate("data:default").unwrap();
        registry
            .update_value(
                &mut state,
                &serde_json::json!({"items":["one"], "object":{"key":"value"}}),
            )
            .unwrap();
        assert_eq!(
            registry.parameter_values(&state).unwrap(),
            serde_json::json!({"items":["one"], "object":{"key":"value"}})
        );
    }

    #[test]
    fn state_from_snapshot_rejects_non_string_plain_values() {
        let registry = Arc::new(
            ParameterRegistry::compile(&serde_json::json!({
                "workflows": {"core": {"views": {"default": {}}}}
            }))
            .unwrap(),
        );
        let binding = registry.parameter_binding("core:default").unwrap();

        for value in [
            serde_json::json!(7),
            serde_json::json!(["value"]),
            serde_json::json!({"query": "value"}),
        ] {
            let snapshot = ParameterSnapshot::from_parts(
                value,
                "value".to_string(),
                InputSourceIdentity::default(),
                0,
            );
            let error = binding
                .state_from_snapshot(&snapshot, false)
                .expect_err("plain snapshots must contain strings");
            assert!(error.to_string().contains("must contain a string"));
        }
    }

    #[test]
    fn state_from_snapshot_preserves_structured_typed_values_without_text_parsing() {
        let registry = Arc::new(
            ParameterRegistry::compile(&serde_json::json!({
                "workflows": {"data": {"views": {"default": {"query": {
                    "type": "object",
                    "items": {"type": "array<string>"},
                    "metadata": {"type": "object"}
                }}}}}
            }))
            .unwrap(),
        );
        let binding = registry.parameter_binding("data:default").unwrap();
        let values = serde_json::json!({
            "items": ["item\u{1b}[31m"],
            "metadata": {"label": "value\u{1b}[0m"}
        });
        let snapshot = ParameterSnapshot::from_parts(
            values.clone(),
            "item {}".to_string(),
            InputSourceIdentity::default(),
            4,
        );

        let state = binding
            .state_from_snapshot(&snapshot, false)
            .expect("structured values in an initialization snapshot are valid");
        assert_eq!(binding.parameter_values(&state).unwrap(), values);
        assert_eq!(state.raw_input(), "item {}");
        assert_eq!(state.revision(), 4);
    }

    #[test]
    fn initial_parameter_snapshot_allows_missing_required_fields() {
        let registry = Arc::new(
            ParameterRegistry::compile(&serde_json::json!({
                "workflows": {"apps": {"views": {"default": {"query": {
                    "type": "object",
                    "input_order": ["visible"],
                    "visible": {"type": "string", "default": ""},
                    "token": {"type": "string"}
                }}}}}
            }))
            .unwrap(),
        );
        let binding = registry.parameter_binding("apps:default").unwrap();
        let snapshot = ParameterSnapshot::from_parts(
            serde_json::json!({"visible": ""}),
            "".to_string(),
            InputSourceIdentity::default(),
            0,
        );

        let state = binding
            .state_from_snapshot(&snapshot, false)
            .expect("initial snapshot may omit required fields");
        assert_eq!(
            registry.parameter_values(&state).unwrap(),
            serde_json::json!({"visible": ""})
        );
    }

    #[test]
    fn parameter_snapshot_carries_mount_identity_and_committed_raw_input() {
        let registry = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"data": {"views": {"default": {}}}}
        }))
        .unwrap();
        let state = registry.instantiate("data:default").unwrap();
        let source = InputSourceIdentity {
            frame: crate::input::ViewMountId(7),
            generation: 2,
        };
        let snapshot = ParameterSnapshot::from_parts(
            registry.parameter_values(&state).unwrap(),
            state.raw_input().to_string(),
            source,
            state.revision(),
        );
        assert_eq!(snapshot.source(), source);
        assert_eq!(snapshot.raw_input(), "");
        assert_eq!(snapshot.revision(), 0);
    }
}
