use anyhow::{Context, Result};
use jsonpath_rfc9535::JsonPath;
use serde_json::Value;

pub(crate) fn apply_path(source: Value, expression: &str) -> Result<Value> {
    let path = JsonPath::parse(expression)
        .with_context(|| format!("invalid JSONPath expression {:?}", expression))?;
    let values = path.query_values(&source);
    match values.as_slice() {
        [] => Ok(Value::Null),
        [value] => Ok((*value).clone()),
        values => Ok(Value::Array(
            values.iter().map(|value| (*value).clone()).collect(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_selects_values_from_an_object() {
        let source = serde_json::json!({
            "aa": {
                "a": {"bb": 1},
                "b": {"bb": 2}
            }
        });
        assert_eq!(
            apply_path(source, "$.aa.*.bb").unwrap(),
            serde_json::json!([1, 2])
        );
    }

    #[test]
    fn path_keeps_a_single_match_value_shape() {
        let source = serde_json::json!({"items": [1, 2]});
        assert_eq!(
            apply_path(source, "$.items").unwrap(),
            serde_json::json!([1, 2])
        );
    }

    #[test]
    fn path_returns_null_when_nothing_matches() {
        let source = serde_json::json!({"items": [1, 2]});
        assert_eq!(apply_path(source, "$.missing").unwrap(), Value::Null);
    }
}
