use crate::input::EditorBuffer;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum FieldType {
    #[default]
    String,
    Integer,
    Number,
    Boolean,
    Json,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Content {
    fields: Vec<Field>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Field {
    pub(super) name: String,
    pub(super) label: Option<String>,
    #[serde(default, rename = "type")]
    pub(super) kind: FieldType,
    #[serde(default)]
    pub(super) required: bool,
    #[serde(default)]
    value: Value,
}

#[derive(Debug)]
pub(super) struct Draft {
    pub(super) field: Field,
    pub(super) buffer: EditorBuffer,
    initial: String,
}

pub(super) fn parse_content(value: Value) -> Result<Vec<Draft>> {
    let content: Content = serde_json::from_value(value).context("invalid form content")?;
    let mut names = HashSet::new();
    content
        .fields
        .into_iter()
        .map(|field| {
            ensure!(
                !field.name.trim().is_empty(),
                "form field name must be nonempty"
            );
            ensure!(
                names.insert(field.name.clone()),
                "duplicate form field {:?}",
                field.name
            );
            let value = &field.value;
            ensure!(
                value.is_null()
                    || match field.kind {
                        FieldType::String => value.is_string(),
                        FieldType::Integer => value.is_i64() || value.is_u64(),
                        FieldType::Number => value.is_number(),
                        FieldType::Boolean => value.is_boolean(),
                        FieldType::Json => true,
                    },
                "form field {:?} initial value does not match its type",
                field.name
            );
            let initial = if value.is_null() {
                String::new()
            } else if field.kind == FieldType::String {
                value.as_str().unwrap().to_string()
            } else {
                value.to_string()
            };
            Ok(Draft {
                buffer: EditorBuffer::from_raw(&initial, initial.len()),
                field,
                initial,
            })
        })
        .collect()
}

impl Draft {
    pub(super) fn value(&self) -> std::result::Result<Value, String> {
        let raw = &self.buffer.raw;
        let value = if self.field.kind == FieldType::String {
            Value::String(raw.clone())
        } else if raw.trim().is_empty() {
            Value::Null
        } else {
            let parsed = serde_json::from_str::<Value>(raw);
            match (self.field.kind, parsed) {
                (FieldType::Json, Ok(value)) => value,
                (FieldType::Integer, Ok(value)) if value.is_i64() || value.is_u64() => value,
                (FieldType::Number, Ok(value)) if value.is_number() => value,
                (FieldType::Boolean, Ok(value)) if value.is_boolean() => value,
                (kind, _) => {
                    return Err(match kind {
                        FieldType::Integer => "Enter an integer",
                        FieldType::Number => "Enter a number",
                        FieldType::Boolean => "Enter true or false",
                        _ => "Enter valid JSON",
                    }
                    .to_string());
                }
            }
        };
        if self.field.required
            && (value.is_null() || value.as_str().is_some_and(|s| s.trim().is_empty()))
        {
            return Err("Required".to_string());
        }
        Ok(value)
    }
}

pub(super) fn state(drafts: &[Draft], focus: usize, ready: bool) -> Value {
    let mut values = Map::new();
    let mut texts = Map::new();
    let mut errors = Map::new();
    for draft in drafts {
        let name = &draft.field.name;
        texts.insert(name.clone(), Value::String(draft.buffer.raw.clone()));
        let value = match draft.value() {
            Ok(value) => value,
            Err(error) => {
                errors.insert(name.clone(), Value::String(error));
                Value::Null
            }
        };
        values.insert(name.clone(), value);
    }
    json!({
        "values": values,
        "drafts": texts,
        "valid": ready && errors.is_empty(),
        "errors": errors,
        "dirty": drafts.iter().any(|d| d.buffer.raw != d.initial),
        "focused": drafts.get(focus).map(|d| &d.field.name),
    })
}
