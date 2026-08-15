use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
enum QueryType {
    String,
    Integer,
    Number,
    Boolean,
    Array(Box<QueryType>),
    Object,
}

impl QueryType {
    fn parse(source: &str) -> Result<Self> {
        match source {
            "string" => Ok(Self::String),
            "integer" => Ok(Self::Integer),
            "number" => Ok(Self::Number),
            "boolean" => Ok(Self::Boolean),
            "object" => Ok(Self::Object),
            _ if source.starts_with("array<") && source.ends_with('>') => {
                let item = &source[6..source.len() - 1];
                if item.is_empty() {
                    bail!("query array type requires an item type");
                }
                Ok(Self::Array(Box::new(Self::parse(item)?)))
            }
            _ => bail!("unsupported query type {:?}", source),
        }
    }

    fn accepts(&self, value: &Value, nullable: bool) -> bool {
        if value.is_null() {
            return nullable;
        }
        match self {
            Self::String => value.is_string(),
            Self::Integer => value.as_i64().is_some() || value.as_u64().is_some(),
            Self::Number => value.is_number(),
            Self::Boolean => value.is_boolean(),
            Self::Array(item_type) => value
                .as_array()
                .is_some_and(|values| values.iter().all(|value| item_type.accepts(value, false))),
            Self::Object => value.is_object(),
        }
    }

    fn description(&self) -> String {
        match self {
            Self::String => "string".to_string(),
            Self::Integer => "integer".to_string(),
            Self::Number => "number".to_string(),
            Self::Boolean => "boolean".to_string(),
            Self::Array(item) => format!("array<{}>", item.description()),
            Self::Object => "object".to_string(),
        }
    }

    fn parse_text(&self, source: &str) -> Result<Value> {
        match self {
            Self::String => Ok(Value::String(source.to_string())),
            Self::Integer => source
                .parse::<i64>()
                .map(Value::from)
                .with_context(|| format!("{:?} is not an integer", source)),
            Self::Number => source
                .parse::<serde_json::Number>()
                .map(Value::Number)
                .with_context(|| format!("{:?} is not a number", source)),
            Self::Boolean => source
                .parse::<bool>()
                .map(Value::Bool)
                .with_context(|| format!("{:?} is not a boolean", source)),
            Self::Array(_) | Self::Object => {
                bail!("{} input requires JSON", self.description())
            }
        }
    }

    fn parse_cli(&self, source: &str) -> Result<Value> {
        let Self::Array(item_type) = self else {
            return self.parse_text(source);
        };
        if source.is_empty() {
            return Ok(Value::Array(Vec::new()));
        }
        source
            .split(',')
            .enumerate()
            .map(|(index, item)| {
                let item = item.trim();
                if item.is_empty() {
                    bail!("array element {index} is empty");
                }
                item_type.parse_text(item).with_context(|| {
                    format!("array element {index} must be {}", item_type.description())
                })
            })
            .collect::<Result<Vec<_>>>()
            .map(Value::Array)
    }
}

#[derive(Debug, Clone)]
struct QueryField {
    value_type: QueryType,
    default: Value,
    required: bool,
    nullable: bool,
}

impl QueryField {
    fn validate(&self, value: &Value) -> Result<()> {
        if self.value_type.accepts(value, self.nullable) {
            return Ok(());
        }
        bail!("expected {}", self.value_type.description())
    }
}

#[derive(Debug, Clone)]
struct QuerySchema {
    plain: bool,
    fields: BTreeMap<String, QueryField>,
    input_order: Vec<String>,
}

#[derive(Debug, Clone)]
struct ViewQuerySchema {
    query: QuerySchema,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct StateRegistry {
    views: BTreeMap<String, ViewQuerySchema>,
}

impl StateRegistry {
    pub(crate) fn compile(config: &Value) -> Result<Self> {
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

fn compile_query(view: &Value) -> Result<QuerySchema> {
    let Some(query) = view.get("query") else {
        return Ok(QuerySchema {
            plain: true,
            fields: BTreeMap::new(),
            input_order: Vec::new(),
        });
    };
    let query = query.as_object().context("view query must be an object")?;
    let query_type = query
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("string");
    if query_type == "string" {
        if query.keys().any(|key| key != "type") {
            bail!("string query may only declare type");
        }
        return Ok(QuerySchema {
            plain: true,
            fields: BTreeMap::new(),
            input_order: Vec::new(),
        });
    }
    if query_type != "object" {
        bail!("view query type must be \"object\" or \"string\"");
    }
    let input_order = parse_input_order(query.get("input_order"))?;
    let mut fields = BTreeMap::new();
    for (name, value) in query {
        if matches!(name.as_str(), "type" | "input_order") {
            continue;
        }
        fields.insert(name.clone(), compile_field(name, value)?);
    }
    let mut seen = BTreeSet::new();
    for name in &input_order {
        if !seen.insert(name.clone()) {
            bail!("query input_order contains duplicate field {:?}", name);
        }
        if !fields.contains_key(name) {
            bail!("query input_order references unknown field {:?}", name);
        }
    }
    Ok(QuerySchema {
        plain: false,
        fields,
        input_order,
    })
}

fn compile_field(name: &str, value: &Value) -> Result<QueryField> {
    let field = value
        .as_object()
        .with_context(|| format!("query field {:?} must be an object", name))?;
    for key in field.keys() {
        if !matches!(key.as_str(), "type" | "default" | "nullable") {
            bail!("query field {:?} has unknown property {:?}", name, key);
        }
    }
    let value_type = field
        .get("type")
        .and_then(Value::as_str)
        .with_context(|| format!("query field {:?} requires a type", name))?;
    let value_type =
        QueryType::parse(value_type).with_context(|| format!("invalid query field {:?}", name))?;
    let nullable = field
        .get("nullable")
        .map(|value| value.as_bool().context("query nullable must be boolean"))
        .transpose()?
        .unwrap_or(false);
    let default = field.get("default").cloned().unwrap_or(Value::Null);
    let nullable = nullable || default.is_null() && field.contains_key("default");
    let required = !field.contains_key("default") && !nullable;
    let definition = QueryField {
        value_type,
        default,
        required,
        nullable,
    };
    if !definition.required {
        definition
            .validate(&definition.default)
            .with_context(|| format!("invalid default for query field {:?}", name))?;
    }
    Ok(definition)
}

fn validate_required(schema: &QuerySchema, values: &BTreeMap<String, Value>) -> Result<()> {
    for (name, field) in &schema.fields {
        if field.required && values.get(name).is_none_or(Value::is_null) {
            bail!("query parameter --{} is required", name);
        }
    }
    Ok(())
}

fn render_input_value(value: &Value) -> String {
    match value {
        Value::Null => "''".to_string(),
        Value::String(value) => shell_words::quote(value).into_owned(),
        Value::Array(values) => values
            .iter()
            .map(|value| match value {
                Value::String(value) => value.clone(),
                value => value.to_string(),
            })
            .collect::<Vec<_>>()
            .join(","),
        value => value.to_string(),
    }
}

fn parse_input_order(value: Option<&Value>) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    value
        .as_array()
        .context("query input_order must be an array of field names")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .context("query input_order entries must be strings")
        })
        .collect()
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
