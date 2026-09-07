use crate::workflow::parameter::ParameterState;
use anyhow::{Result, ensure};
use serde_json::Value;

/// Immutable, per-launch workflow state.
///
/// This is intentionally separate from `CompiledConfig`: several independent
/// launches may share one compiled configuration while retaining different root
/// targets, captured stdin, and initial parameter values.
#[derive(Debug, Clone)]
pub(crate) struct InvocationContext {
    root_view: String,
    input: Value,
    root_parameters: ParameterState,
}

impl InvocationContext {
    pub(crate) fn new(
        root_view: String,
        input: Value,
        root_parameters: ParameterState,
    ) -> Result<Self> {
        ensure!(
            root_parameters.view_ref() == root_view,
            "invocation parameters belong to {:?}, not root view {:?}",
            root_parameters.view_ref(),
            root_view
        );
        Ok(Self {
            root_view,
            input,
            root_parameters,
        })
    }

    pub(crate) fn root_view(&self) -> &str {
        &self.root_view
    }

    pub(crate) fn input_value(&self) -> &Value {
        &self.input
    }

    pub(crate) fn root_parameters(&self) -> &ParameterState {
        &self.root_parameters
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contexts_sharing_compiled_configuration_remain_independent() {
        let config = crate::workflow::config::load_test_fixture().unwrap();
        let first = InvocationContext::new(
            "core:default".to_string(),
            serde_json::json!({"stdin": {"path": "/tmp/first"}}),
            config.instantiate_parameters("core:default").unwrap(),
        )
        .unwrap();
        let second = InvocationContext::new(
            "dmenu:main".to_string(),
            serde_json::json!({"stdin": {"path": "/tmp/second"}}),
            config.instantiate_parameters("dmenu:main").unwrap(),
        )
        .unwrap();

        assert_eq!(first.root_view(), "core:default");
        assert_eq!(second.root_view(), "dmenu:main");
        assert_eq!(first.input_value()["stdin"]["path"], "/tmp/first");
        assert_eq!(second.input_value()["stdin"]["path"], "/tmp/second");
        assert_eq!(first.root_parameters().view_ref(), "core:default");
        assert_eq!(second.root_parameters().view_ref(), "dmenu:main");
    }
}
