use crate::expression::parse_state_declaration;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct StateKey(Vec<String>);

impl StateKey {
    fn display(&self) -> String {
        self.0.join(".")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StateType {
    String,
    Integer,
    Number,
    Boolean,
    Array(Box<StateType>),
    Object,
}

impl StateType {
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
                    bail!("state array type requires an item type");
                }
                Ok(Self::Array(Box::new(Self::parse(item)?)))
            }
            _ => bail!("unsupported state type {:?}", source),
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

    fn parse_comma_value(&self, source: &str) -> Result<Value> {
        let Self::Array(item_type) = self else {
            return Ok(Value::String(source.to_string()));
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
}

#[derive(Debug, Clone)]
struct StateDefinition {
    value_type: StateType,
    default: Value,
    required: bool,
    nullable: bool,
}

impl StateDefinition {
    fn validate(&self, value: &Value) -> Result<()> {
        if self.value_type.accepts(value, self.nullable) {
            return Ok(());
        }
        bail!("expected {}", self.value_type.description())
    }
}

#[derive(Debug, Clone)]
struct QuerySchema {
    fields: BTreeMap<String, StateKey>,
    input_order: Vec<String>,
}

#[derive(Debug, Clone)]
struct ViewStateSchema {
    definitions: BTreeMap<StateKey, StateDefinition>,
    query: Option<QuerySchema>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct StateRegistry {
    views: BTreeMap<String, ViewStateSchema>,
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
                let mut definitions = BTreeMap::new();
                scan_definitions(view, &mut Vec::new(), &mut definitions)?;
                let query = compile_query(view, &definitions)?;
                views.insert(view_ref, ViewStateSchema { definitions, query });
            }
        }
        Ok(Self { views })
    }

    pub(crate) fn instantiate(&self, view_ref: &str) -> Result<StateInstance> {
        let schema = self
            .views
            .get(view_ref)
            .with_context(|| format!("view {:?} has no state schema", view_ref))?;
        Ok(StateInstance {
            view_ref: view_ref.to_string(),
            values: schema
                .definitions
                .iter()
                .map(|(key, definition)| (key.clone(), definition.default.clone()))
                .collect(),
            revision: 0,
        })
    }

    pub(crate) fn has_query(&self, view_ref: &str) -> bool {
        self.views
            .get(view_ref)
            .and_then(|schema| schema.query.as_ref())
            .is_some()
    }

    pub(crate) fn bind_cli(&self, view_ref: &str, arguments: &[String]) -> Result<StateInstance> {
        let mut state = self.instantiate(view_ref)?;
        let schema = self.schema_for(view_ref, &state)?;
        let Some(query) = schema.query.as_ref() else {
            if arguments.is_empty() {
                return Ok(state);
            }
            bail!("view {:?} does not declare query parameters", view_ref);
        };
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
            let key = query.fields.get(name).with_context(|| {
                format!(
                    "view {:?} does not declare query parameter --{}",
                    view_ref, name
                )
            })?;
            if !assigned.insert(key.clone()) {
                bail!("query parameter --{} may be specified only once", name);
            }
            let definition = schema
                .definitions
                .get(key)
                .expect("query state definition disappeared");
            let value = if typed {
                value
            } else {
                definition
                    .value_type
                    .parse_comma_value(value.as_str().expect("untyped CLI input is a string"))?
            };
            definition
                .validate(&value)
                .with_context(|| format!("invalid query parameter --{}", name))?;
            state.values.insert(key.clone(), value);
        }
        validate_required(query, &schema.definitions, &state.values)?;
        if !assigned.is_empty() {
            state.revision = state.revision.wrapping_add(1);
        }
        Ok(state)
    }

    pub(crate) fn render_input(&self, state: &StateInstance) -> Result<String> {
        let schema = self.schema_for(&state.view_ref, state)?;
        let Some(query) = schema.query.as_ref() else {
            return Ok(String::new());
        };
        if query.input_order.is_empty() {
            return Ok(String::new());
        }
        if query.input_order.len() == 1 {
            let key = query
                .fields
                .get(&query.input_order[0])
                .expect("validated query state disappeared");
            let definition = schema
                .definitions
                .get(key)
                .expect("query state definition disappeared");
            if definition.value_type == StateType::String {
                return Ok(state
                    .values
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string());
            }
        }
        Ok(query
            .input_order
            .iter()
            .map(|name| {
                let key = query
                    .fields
                    .get(name)
                    .expect("validated query state disappeared");
                let definition = schema
                    .definitions
                    .get(key)
                    .expect("query state definition disappeared");
                render_input_value(state.values.get(key).unwrap_or(&definition.default))
            })
            .collect::<Vec<_>>()
            .join(" "))
    }

    pub(crate) fn update_input(&self, state: &mut StateInstance, source: &str) -> Result<bool> {
        let schema = self.schema_for(&state.view_ref, state)?;
        let Some(query) = schema.query.as_ref() else {
            return Ok(false);
        };
        if query.input_order.is_empty() {
            return Ok(false);
        }
        let tokens = if query.input_order.len() == 1 {
            let key = query
                .fields
                .get(&query.input_order[0])
                .expect("validated query state disappeared");
            let definition = schema
                .definitions
                .get(key)
                .expect("query state definition disappeared");
            if definition.value_type == StateType::String {
                vec![source.to_string()]
            } else {
                shell_words::split(source).context("query input contains invalid quoting")?
            }
        } else {
            shell_words::split(source).context("query input contains invalid quoting")?
        };
        if tokens.len() > query.input_order.len() {
            bail!(
                "query input expected at most {} values, received {}",
                query.input_order.len(),
                tokens.len()
            );
        }
        let mut values = state.values.clone();
        for (index, name) in query.input_order.iter().enumerate() {
            let key = query
                .fields
                .get(name)
                .expect("validated query state disappeared");
            let definition = schema
                .definitions
                .get(key)
                .expect("query state definition disappeared");
            let value = match tokens.get(index) {
                Some(token) => definition
                    .value_type
                    .parse_text(token)
                    .with_context(|| format!("query input field {:?} is invalid", name))?,
                None if definition.required => {
                    bail!("query input is missing required field {:?}", name)
                }
                None => definition.default.clone(),
            };
            definition.validate(&value)?;
            values.insert(key.clone(), value);
        }
        if values == state.values {
            return Ok(false);
        }
        state.values = values;
        state.revision = state.revision.wrapping_add(1);
        Ok(true)
    }

    pub(crate) fn materialize(&self, config: &Value, state: &StateInstance) -> Result<Value> {
        if !self.views.contains_key(&state.view_ref) && state.values.is_empty() {
            return Ok(config.clone());
        }
        self.schema_for(&state.view_ref, state)?;
        let mut materialized = config.clone();
        for (key, value) in &state.values {
            set_path(&mut materialized, &key.0, value.clone());
        }
        Ok(materialized)
    }

    fn schema_for<'a>(
        &'a self,
        view_ref: &str,
        state: &StateInstance,
    ) -> Result<&'a ViewStateSchema> {
        if state.view_ref != view_ref {
            bail!(
                "state instance for {:?} cannot evaluate view {:?}",
                state.view_ref,
                view_ref
            );
        }
        self.views
            .get(view_ref)
            .with_context(|| format!("view {:?} has no state schema", view_ref))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct StateInstance {
    view_ref: String,
    values: BTreeMap<StateKey, Value>,
    revision: u64,
}

impl StateInstance {
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn view_ref(&self) -> &str {
        &self.view_ref
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub(crate) fn empty(view_ref: impl Into<String>) -> Self {
        Self {
            view_ref: view_ref.into(),
            values: BTreeMap::new(),
            revision: 0,
        }
    }
}

fn compile_query(
    view: &Value,
    definitions: &BTreeMap<StateKey, StateDefinition>,
) -> Result<Option<QuerySchema>> {
    let Some(query) = view.get("query") else {
        return Ok(None);
    };
    let query = query.as_object().context("view query must be an object")?;
    if query.get("type").and_then(Value::as_str) != Some("object") {
        bail!("view query type must be \"object\"");
    }
    let input_order = parse_input_order(query.get("input_order"))?;
    let mut fields = BTreeMap::new();
    for (name, _) in query {
        if matches!(name.as_str(), "type" | "input_order") {
            continue;
        }
        let key = StateKey(vec!["query".to_string(), name.clone()]);
        if definitions.contains_key(&key) {
            fields.insert(name.clone(), key);
        }
    }
    let mut seen = BTreeSet::new();
    for name in &input_order {
        if !seen.insert(name.clone()) {
            bail!("query input_order contains duplicate field {:?}", name);
        }
        if !fields.contains_key(name) {
            bail!("query input_order references unknown state {:?}", name);
        }
    }
    Ok(Some(QuerySchema {
        fields,
        input_order,
    }))
}

fn scan_definitions(
    value: &Value,
    path: &mut Vec<String>,
    definitions: &mut BTreeMap<StateKey, StateDefinition>,
) -> Result<()> {
    if let Value::String(source) = value
        && let Some(declaration) = parse_state_declaration(source)?
    {
        let value_type = StateType::parse(&declaration.type_name)?;
        let required = declaration.default.is_none();
        let default = declaration.default.unwrap_or(Value::Null);
        let nullable = default.is_null();
        let key = StateKey(path.clone());
        let definition = StateDefinition {
            value_type,
            default,
            required,
            nullable,
        };
        if !definition.required {
            definition
                .validate(&definition.default)
                .with_context(|| format!("invalid default for state {}", key.display()))?;
        }
        if definitions.insert(key.clone(), definition).is_some() {
            bail!("duplicate state definition {}", key.display());
        }
        return Ok(());
    }
    match value {
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                path.push(index.to_string());
                scan_definitions(value, path, definitions)?;
                path.pop();
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                path.push(key.clone());
                scan_definitions(value, path, definitions)?;
                path.pop();
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_required(
    query: &QuerySchema,
    definitions: &BTreeMap<StateKey, StateDefinition>,
    values: &BTreeMap<StateKey, Value>,
) -> Result<()> {
    for key in query.fields.values() {
        let definition = definitions
            .get(key)
            .expect("query state definition disappeared");
        if definition.required && values.get(key).is_none_or(Value::is_null) {
            bail!("query parameter --{} is required", key.0.last().unwrap());
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
        .context("query input_order must be an array of state names")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .context("query input_order entries must be strings")
        })
        .collect()
}

fn set_path(root: &mut Value, path: &[String], value: Value) {
    let mut current = root;
    for component in &path[..path.len().saturating_sub(1)] {
        current = match current {
            Value::Object(object) => object.get_mut(component),
            Value::Array(array) => component
                .parse::<usize>()
                .ok()
                .and_then(|index| array.get_mut(index)),
            _ => None,
        }
        .expect("compiled state path disappeared from View configuration");
    }
    let Some(last) = path.last() else {
        return;
    };
    match current {
        Value::Object(object) => {
            object.insert(last.clone(), value);
        }
        Value::Array(array) => {
            let index = last
                .parse::<usize>()
                .expect("compiled state array path is invalid");
            *array
                .get_mut(index)
                .expect("compiled state array index disappeared") = value;
        }
        _ => panic!("compiled state parent is not a container"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Value {
        serde_json::json!({
            "plugins": {
                "trans": {
                    "views": {
                        "default": {
                            "query": {
                                "type": "object",
                                "input_order": ["source", "target", "text"],
                                "source": "{{ state(\"string\", null) }}",
                                "target": "{{ state(\"string\", null) }}",
                                "text": "{{ state(\"string\", \"\") }}"
                            },
                            "presentation": ["{{ state(\"string\", \"default\") }}"]
                        }
                    }
                }
            }
        })
    }

    #[test]
    fn state_values_are_local_to_one_view_instance() {
        let config = config();
        let registry = StateRegistry::compile(&config).unwrap();
        let mut first = registry
            .bind_cli(
                "trans:default",
                &[
                    "--source=en".to_string(),
                    "--target=zh".to_string(),
                    "--text=hello".to_string(),
                ],
            )
            .unwrap();
        let second = registry.instantiate("trans:default").unwrap();

        registry
            .update_input(&mut first, "ja en 'good morning'")
            .unwrap();
        let first = registry
            .materialize(&config["plugins"]["trans"]["views"]["default"], &first)
            .unwrap();
        let second = registry
            .materialize(&config["plugins"]["trans"]["views"]["default"], &second)
            .unwrap();

        assert_eq!(first["query"]["text"], "good morning");
        assert_eq!(second["query"]["text"], "");
        assert_eq!(first["presentation"][0], "default");
    }

    #[test]
    fn cli_rejects_positionals_repeated_keys_and_wrong_types() {
        let registry = StateRegistry::compile(&config()).unwrap();
        assert!(
            registry
                .bind_cli("trans:default", &["hello".to_string()])
                .is_err()
        );
        assert!(
            registry
                .bind_cli(
                    "trans:default",
                    &["--source=en".to_string(), "--source=ja".to_string()]
                )
                .is_err()
        );
        assert!(
            registry
                .bind_cli("trans:default", &["--source:=1".to_string()])
                .is_err()
        );
    }
}
