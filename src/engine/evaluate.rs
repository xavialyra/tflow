use super::ViewContext;
use crate::config::ConfigSource;
use crate::expression::EvaluationStage;
use anyhow::{Context, Result};
use serde_json::Value;

pub(crate) fn field(context: &ViewContext<'_>, name: &str) -> Result<Option<Value>> {
    context.config.get(
        ConfigSource::View(context.state.view_ref()),
        &context.evaluation,
        EvaluationStage::Operation,
        &[name],
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
