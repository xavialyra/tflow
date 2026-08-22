use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum QueryType {
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

    pub(super) fn parse_text(&self, source: &str) -> Result<Value> {
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

    pub(super) fn parse_cli(&self, source: &str) -> Result<Value> {
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
pub(super) struct QueryField {
    pub(super) value_type: QueryType,
    pub(super) default: Value,
    pub(super) required: bool,
    nullable: bool,
}

impl QueryField {
    pub(super) fn validate(&self, value: &Value) -> Result<()> {
        if self.value_type.accepts(value, self.nullable) {
            return Ok(());
        }
        bail!("expected {}", self.value_type.description())
    }
}

#[derive(Debug, Clone)]
pub(super) struct QuerySchema {
    pub(super) plain: bool,
    pub(super) fields: BTreeMap<String, QueryField>,
    pub(super) input_order: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ViewQuerySchema {
    pub(super) query: QuerySchema,
}

pub(super) fn compile_query(view: &Value) -> Result<QuerySchema> {
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

pub(super) fn validate_required(
    schema: &QuerySchema,
    values: &BTreeMap<String, Value>,
) -> Result<()> {
    for (name, field) in &schema.fields {
        if field.required && values.get(name).is_none_or(Value::is_null) {
            bail!("query parameter --{} is required", name);
        }
    }
    Ok(())
}

pub(super) fn render_input_value(value: &Value) -> String {
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
