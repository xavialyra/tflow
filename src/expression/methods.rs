use super::{MethodResolver, path, script};
use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub struct ExpressionMethods<'a> {
    script_root: &'a Path,
}

impl<'a> ExpressionMethods<'a> {
    pub fn new(script_root: &'a Path) -> Self {
        Self { script_root }
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
            "script" => script::evaluate(self.script_root, args, named_args),
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
        let references = TreeReferences {
            config: &config,
            runtime: &runtime,
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
