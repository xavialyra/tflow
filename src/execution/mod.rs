mod inline;
mod process;
mod runner;
mod script;

pub(crate) use inline::{
    parse_shebang, prepare_inline_script_command, verify_interpreter,
};
pub(crate) use process::{
    ForegroundTerminalReclaimError, PreparedProcess, ProcessGroupGuard, run_foreground_process,
};
pub(crate) use runner::{BoundedCommandOutcome, run_bounded_command_with_stdin_outcome};
pub(crate) use script::{
    ensure_script_success, run_resolved_script_with_stdin_outcome_with_limit,
    validate_script_target,
};
