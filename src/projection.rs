use anyhow::{Context, Result, bail};
use jsonpath_rfc9535::JsonPath;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct DataRef {
    pub provider: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(rename = "match", default)]
    pub matcher: Option<String>,
}

impl DataRef {
    pub fn validate(&self) -> Result<()> {
        if self.provider.trim().is_empty() {
            bail!("data request provider cannot be empty");
        }
        if self.provider == "script" && self.target.as_deref().is_none_or(str::is_empty) {
            bail!("script data request requires a target");
        }
        if let Some(matcher) = &self.matcher {
            JsonPath::parse(matcher)
                .with_context(|| format!("invalid JSONPath match {:?}", matcher))?;
        }
        Ok(())
    }

    pub fn project(&self, source: Value) -> Result<Value> {
        self.validate()?;
        apply_match(source, self.matcher.as_deref())
    }
}

pub(crate) fn apply_match(source: Value, matcher: Option<&str>) -> Result<Value> {
    let expression = matcher.unwrap_or("$");
    let path = JsonPath::parse(expression)
        .with_context(|| format!("invalid JSONPath match {:?}", expression))?;
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
    fn config_and_runtime_requests_apply_jsonpath() {
        let config = serde_json::json!({
            "aa": {
                "a": {"bb": 1},
                "b": {"bb": 2}
            }
        });
        let runtime = serde_json::json!({
            "view": {
                "current": {
                    "command": [{"id": "open"}]
                }
            }
        });
        let config_ref: DataRef = toml::from_str(
            r#"
            provider = "config"
            match = "$.aa.*.bb"
            "#,
        )
        .unwrap();
        assert_eq!(
            config_ref.project(config.clone()).unwrap(),
            serde_json::json!([1, 2])
        );

        let runtime_ref: DataRef = toml::from_str(
            r#"
            provider = "runtime"
            match = "$.view.current.command"
            "#,
        )
        .unwrap();
        assert_eq!(
            runtime_ref.project(runtime).unwrap(),
            serde_json::json!([{"id": "open"}])
        );
    }

    #[test]
    fn a_single_match_keeps_the_selected_value_shape() {
        let source = serde_json::json!({"items": [1, 2]});
        let data_ref = DataRef {
            provider: "config".to_string(),
            target: None,
            matcher: Some("$.items".to_string()),
        };
        assert_eq!(data_ref.project(source).unwrap(), serde_json::json!([1, 2]));
    }

    #[test]
    fn script_requests_require_a_target() {
        let data_ref = DataRef {
            provider: "script".to_string(),
            target: None,
            matcher: None,
        };
        let error = data_ref
            .project(Value::Null)
            .expect_err("script requests require a target");
        assert!(error.to_string().contains("requires a target"));
    }
}
