use anyhow::{Context, Result, bail};
use jsonpath_rfc9535::JsonPath;
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) fn evaluate(args: Vec<Value>, named_args: BTreeMap<String, Value>) -> Result<Value> {
    if !named_args.is_empty() || args.len() != 2 {
        bail!("path expects a value and a JSONPath expression")
    }
    let mut args = args.into_iter();
    let source = args.next().expect("path source exists");
    let expression = args.next().expect("path expression exists");
    let expression = expression
        .as_str()
        .context("path requires a string JSONPath expression")?;
    apply_path(source, expression)
}

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
