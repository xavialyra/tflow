use super::{MethodResolver, path, script};
use crate::cancellation::CancellationToken;
use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub struct ExpressionMethods<'a> {
    script_root: &'a Path,
    cancellation: CancellationToken,
}

impl<'a> ExpressionMethods<'a> {
    #[cfg(test)]
    pub fn new(script_root: &'a Path) -> Self {
        Self::with_cancellation(script_root, CancellationToken::new())
    }

    pub fn with_cancellation(script_root: &'a Path, cancellation: CancellationToken) -> Self {
        Self {
            script_root,
            cancellation,
        }
    }
}

impl MethodResolver for ExpressionMethods<'_> {
    fn call_method(
        &mut self,
        name: &str,
        args: Vec<Value>,
        named_args: BTreeMap<String, Value>,
    ) -> Result<Value> {
        match name {
            "path" => path::evaluate(args, named_args),
            "script" => script::evaluate(self.script_root, &self.cancellation, args, named_args),
            _ => anyhow::bail!("unknown expression method {:?}", name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expression::{EvalContext, Template, TreeReferences};

    #[test]
    fn path_projects_an_expression_value() {
        let config = Value::Null;
        let runtime = serde_json::json!({"view": {"current": {"items": [1, 2]}}});
        let input = Value::Null;
        let references = TreeReferences {
            config: &config,
            this: &Value::Null,
            runtime: &runtime,
            input: &input,
            request: None,
            returned: None,
        };
        let mut methods = ExpressionMethods::new(Path::new("."));
        let mut context = EvalContext {
            references: &references,
            methods: &mut methods,
        };
        assert_eq!(
            Template::parse(r#"{{ path(runtime:view.current, "$.items") }}"#)
                .unwrap()
                .evaluate_value(&mut context)
                .unwrap(),
            serde_json::json!([1, 2])
        );
    }
}
