use super::ViewContext;
use crate::config::{ConfigReadContext, ConfigScope};
use anyhow::{Context, Result};
use serde_json::Value;

pub(crate) fn field(context: &ViewContext<'_>, name: &str) -> Result<Option<Value>> {
    let runtime = context.runtime.read();
    let request = context.request.reference_value();
    context.config.get_with_references(
        ConfigReadContext {
            scope: ConfigScope::View(context.state),
            runtime: &runtime,
            input: &context.config.input_value,
            cancellation: Some(context.cancellation.clone()),
            binding_raw: None,
        },
        &[name],
        request.as_ref(),
        None,
    )
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
