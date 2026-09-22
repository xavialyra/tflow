use super::schema::{
    ParameterSchema, ViewParameterSchema, compile_parameter_schema, validate_required,
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

    #[allow(dead_code)]
    pub(crate) fn bind_cli(&self, view_ref: &str, arguments: &[String]) -> Result<ParameterState> {
        let mut state = self.instantiate(view_ref)?;
        self.apply_cli(&mut state, arguments)?;
        Ok(state)
    }

    pub(crate) fn apply_cli(&self, state: &mut ParameterState, arguments: &[String]) -> Result<()> {
        let view_ref = &state.view_ref;
        let schema = &self.schema_for(view_ref, state)?.schema;
        if schema.plain {
            if arguments.is_empty() {
                return Ok(());
            }
            if arguments.len() > 1 {
                bail!(
                    "view {:?} accepts at most one positional query argument, received {}",
                    view_ref,
                    arguments.len()
                );
            }
            let argument = &arguments[0];
            if argument.starts_with("--") {
                bail!(
                    "view {:?} does not declare keyed query parameters",
                    view_ref
                );
            }
            state
                .values
                .insert("query".to_string(), Value::String(argument.clone()));
            state.revision = state.revision.wrapping_add(1);
            state.raw_input = argument.clone();
            return Ok(());
        }
        let mut assigned = BTreeSet::new();
        let mut positional_seen = false;
        for argument in arguments {
            if let Some(raw) = argument.strip_prefix("--") {
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
            } else {
                if positional_seen {
                    bail!(
                        "view {:?} accepts at most one positional query argument",
                        view_ref
                    );
                }
                positional_seen = true;
                let Some(input_field) = &schema.input else {
                    bail!(
                        "view {:?} does not declare an interactive input field and does not accept positional arguments",
                        view_ref
                    );
                };
                let field = schema.fields.get(input_field).with_context(|| {
                    format!("query input references unknown field {:?}", input_field)
                })?;
                if !assigned.insert(input_field.clone()) {
                    bail!(
                        "query parameter --{} may be specified only once",
                        input_field
                    );
                }
                let value = Value::String(argument.clone());
                field
                    .validate(&value)
                    .with_context(|| format!("invalid query parameter --{}", input_field))?;
                state.values.insert(input_field.clone(), value);
            }
        }
        validate_required(schema, &state.values)?;
        if !assigned.is_empty() {
            state.revision = state.revision.wrapping_add(1);
        }
        state.raw_input = self.render_input(state)?;
        Ok(())
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
        if let Some(input_field) = &schema.input {
            return Ok(state
                .values
                .get(input_field)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string());
        }
        Ok(String::new())
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
        let Some(input_field) = &schema.input else {
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
            bail!("query input cannot be parsed because input is not configured");
        };
        let field = schema
            .fields
            .get(input_field)
            .expect("query field disappeared");
        let value = Value::String(source.to_string());
        field.validate(&value)?;
        let mut values = state.values.clone();
        values.insert(input_field.clone(), value);
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
            let empty_or_null =
                snapshot.values.as_str().is_some_and(|s| s.is_empty()) || snapshot.values.is_null();
            if !empty_or_null {
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
        }
        // Initialization snapshots may intentionally omit required fields;
        // strict validation remains at user input and typed patch boundaries.
        state.raw_input = snapshot.raw_input.clone();
        state.revision = snapshot.revision;
        state.input_rejected = input_rejected;
        Ok(state)
    }

    #[allow(dead_code)]
    pub(crate) fn bind_cli(&self, arguments: &[String]) -> Result<ParameterState> {
        self.registry.bind_cli(&self.view_ref, arguments)
    }

    pub(crate) fn apply_cli(&self, state: &mut ParameterState, arguments: &[String]) -> Result<()> {
        anyhow::ensure!(
            state.view_ref() == self.view_ref,
            "parameter binding for {:?} cannot update {:?}",
            self.view_ref,
            state.view_ref()
        );
        self.registry.apply_cli(state, arguments)
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
                    "input": "text",
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
        registry.update_input(&mut state, "good morning").unwrap();
        assert_eq!(
            registry.parameter_values(&state).unwrap(),
            serde_json::json!({"source": null, "target": null, "text": "good morning"})
        );
    }

    #[test]
    fn object_query_can_be_initialized_directly() {
        let registry = ParameterRegistry::compile(&config()).unwrap();
        let mut state = registry.instantiate("trans:default").unwrap();
        registry
            .update_value(
                &mut state,
                &serde_json::json!({"source":"en", "target":"zh", "text":"hello"}),
            )
            .unwrap();
        assert_eq!(registry.render_input(&state).unwrap(), "hello");
        assert_eq!(registry.parameter_values(&state).unwrap()["text"], "hello");
        assert_eq!(registry.parameter_values(&state).unwrap()["source"], "en");
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
    fn instance_validation_checks_required_fields_outside_input() {
        let registry = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"apps": {"views": {"default": {"query": {
                "type": "object",
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
                "input": "visible",
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
    fn query_input_rejects_nonempty_input_when_input_field_not_configured() {
        let registry = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"apps": {"views": {"default": {"query": {
                "type": "object",
                "token": {"type": "string", "default": "fixed"}
            }}}}}
        }))
        .unwrap();
        let mut state = registry.instantiate("apps:default").unwrap();

        let error = registry
            .update_input(&mut state, "needle")
            .expect_err("user input requires configured input field");
        assert!(error.to_string().contains("input is not configured"));
        assert_eq!(registry.parameter_values(&state).unwrap()["token"], "fixed");
    }

    #[test]
    fn query_schema_rejects_non_string_input_field() {
        let error = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"data": {"views": {"default": {"query": {
                "type": "object",
                "input": "items",
                "items": {"type": "array<string>", "default": []}
            }}}}}
        }))
        .expect_err("non-string input field must be rejected");
        assert!(error.to_string().contains("must be of type string"));
    }

    #[test]
    fn query_schema_rejects_legacy_input_order() {
        let error = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"data": {"views": {"default": {"query": {
                "type": "object",
                "input_order": ["maybe"],
                "maybe": {"type": "string", "nullable": true}
            }}}}}
        }))
        .expect_err("legacy input_order must be rejected");
        assert!(
            error
                .to_string()
                .contains("input_order has been replaced by input")
        );
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
                    "input": "visible",
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

    #[test]
    fn cli_accepts_single_positional_query_for_plain_view() {
        let registry = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"calc": {"views": {"main": {}}}}
        }))
        .unwrap();
        let state = registry
            .bind_cli("calc:main", &["2+2".to_string()])
            .unwrap();
        assert_eq!(
            registry.parameter_values(&state).unwrap(),
            Value::String("2+2".to_string())
        );
        assert_eq!(state.raw_input(), "2+2");

        let err = registry
            .bind_cli("calc:main", &["2+2".to_string(), "extra".to_string()])
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("accepts at most one positional query argument")
        );
    }

    #[test]
    fn cli_accepts_positional_query_mapped_to_input_field() {
        let registry = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"apps": {"views": {"main": {
                "query": {
                    "type": "object",
                    "input": "search",
                    "search": {"type": "string", "default": ""},
                    "limit": {"type": "integer", "default": 10}
                }
            }}}}
        }))
        .unwrap();
        let state = registry
            .bind_cli(
                "apps:main",
                &["firefox".to_string(), "--limit=20".to_string()],
            )
            .unwrap();
        assert_eq!(
            registry.parameter_values(&state).unwrap(),
            serde_json::json!({"search": "firefox", "limit": 20})
        );
        assert_eq!(state.raw_input(), "firefox");

        // Reject duplicate assignment via positional and explicit flag
        let err = registry
            .bind_cli(
                "apps:main",
                &["firefox".to_string(), "--search=chrome".to_string()],
            )
            .unwrap_err();
        assert!(err.to_string().contains("may be specified only once"));
    }

    #[test]
    fn cli_rejects_positional_query_when_input_field_not_declared() {
        let registry = ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"form": {"views": {"main": {
                "query": {
                    "type": "object",
                    "name": {"type": "string", "default": ""}
                }
            }}}}
        }))
        .unwrap();
        let err = registry
            .bind_cli("form:main", &["positional".to_string()])
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("does not declare an interactive input field")
        );
    }
}
