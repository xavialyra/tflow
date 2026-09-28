use crate::workflow::config::{Workflow, parse_atomic_workflow_package};
use anyhow::Result;

pub(crate) const COMMANDS_WORKFLOW_TOML: &str =
    include_str!("../../../assets/builtin/workflows/__commands/workflow.toml");
pub(crate) const PARAMETERS_WORKFLOW_TOML: &str =
    include_str!("../../../assets/builtin/workflows/__parameters/workflow.toml");

pub(crate) fn builtin_commands_workflow() -> Result<Workflow> {
    let (_, wf) = parse_atomic_workflow_package(
        COMMANDS_WORKFLOW_TOML,
        "assets/builtin/workflows/__commands/workflow.toml",
        "__commands",
    )?;
    Ok(wf)
}

pub(crate) fn builtin_parameters_workflow() -> Result<Workflow> {
    let (_, wf) = parse_atomic_workflow_package(
        PARAMETERS_WORKFLOW_TOML,
        "assets/builtin/workflows/__parameters/workflow.toml",
        "__parameters",
    )?;
    Ok(wf)
}
