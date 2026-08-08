use super::ViewContext;
use crate::expression::ExpressionMethods;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

pub(crate) fn field(context: &ViewContext<'_>, name: &str) -> Result<Option<Value>> {
    let script_root = context
        .config
        .plugin_root(&context.location.view_ref)
        .unwrap_or_else(|| Path::new("."));
    let runtime = context.runtime.read();
    let mut methods = ExpressionMethods::new(script_root);
    context
        .config
        .evaluate_engine_field(&context.location.view_ref, name, &runtime, &mut methods)
}

pub(crate) fn optional_string(context: &ViewContext<'_>, name: &str) -> Result<Option<String>> {
    field(context, name)?
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .with_context(|| format!("engine field {:?} must evaluate to a string", name))
        })
        .transpose()
}
