use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParameterType {
    String,
    Integer,
    Number,
    Boolean,
    Array(Box<ParameterType>),
    Object,
}

impl ParameterType {
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
                .with_context(|| format!("{source:?} is not an integer")),
            Self::Number => source
                .parse::<serde_json::Number>()
                .map(Value::Number)
                .with_context(|| format!("{source:?} is not a number")),
            Self::Boolean => source
                .parse::<bool>()
                .map(Value::Bool)
                .with_context(|| format!("{source:?} is not a boolean")),
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
pub(crate) struct ParameterField {
    pub(crate) value_type: ParameterType,
    pub(crate) default: Value,
    pub(crate) required: bool,
    pub(crate) nullable: bool,
}

impl ParameterField {
    pub(super) fn validate(&self, value: &Value) -> Result<()> {
        if self.value_type.accepts(value, self.nullable) {
            return Ok(());
        }
        bail!("expected {}", self.value_type.description())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ParameterSchema {
    pub(crate) plain: bool,
    pub(crate) fields: BTreeMap<String, ParameterField>,
    pub(crate) input: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ViewParameterSchema {
    pub(super) schema: ParameterSchema,
}

pub(super) fn compile_parameter_schema(view: &Value) -> Result<ParameterSchema> {
    let Some(query) = view.get("query") else {
        return Ok(ParameterSchema {
            plain: true,
            fields: BTreeMap::new(),
            input: None,
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
        return Ok(ParameterSchema {
            plain: true,
            fields: BTreeMap::new(),
            input: None,
        });
    }
    if query_type != "object" {
        bail!("view query type must be \"object\" or \"string\"");
    }
    if query.contains_key("input_order") {
        bail!("query input_order has been replaced by input = \"<field>\"");
    }
    let input = match query.get("input") {
        Some(val) => {
            let field_name = val
                .as_str()
                .context("query input must be a string field name")?;
            Some(field_name.to_string())
        }
        None => None,
    };
    let mut fields = BTreeMap::new();
    for (name, value) in query {
        if matches!(name.as_str(), "type" | "input") {
            continue;
        }
        fields.insert(name.clone(), compile_field(name, value)?);
    }
    if let Some(input_field) = &input {
        let field = fields
            .get(input_field)
            .with_context(|| format!("query input references unknown field {:?}", input_field))?;
        if field.value_type != ParameterType::String {
            bail!(
                "query input field {:?} must be of type string, found {}",
                input_field,
                field.value_type.description()
            );
        }
    }
    Ok(ParameterSchema {
        plain: false,
        fields,
        input,
    })
}

fn compile_field(name: &str, value: &Value) -> Result<ParameterField> {
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
    let value_type = ParameterType::parse(value_type)
        .with_context(|| format!("invalid query field {:?}", name))?;
    let nullable = field
        .get("nullable")
        .map(|value| value.as_bool().context("query nullable must be boolean"))
        .transpose()?
        .unwrap_or(false);
    let default = field.get("default").cloned().unwrap_or(Value::Null);
    let nullable = nullable || default.is_null() && field.contains_key("default");
    let required = !field.contains_key("default") && !nullable;
    let definition = ParameterField {
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
    schema: &ParameterSchema,
    values: &BTreeMap<String, Value>,
) -> Result<()> {
    for (name, field) in &schema.fields {
        if field.required && values.get(name).is_none_or(Value::is_null) {
            bail!("query parameter --{} is required", name);
        }
    }
    Ok(())
}
