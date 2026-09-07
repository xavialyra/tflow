mod inline;
mod process;
mod runner;
mod script;

#[allow(unused_imports)]
pub(crate) use inline::{
    Shebang, format_attributed_script, materialize_inline_script, parse_shebang,
    prepare_inline_script_command, scripts_cache_dir, verify_interpreter,
};
pub(crate) use process::{
    ForegroundTerminalReclaimError, MANAGED_ENVIRONMENT, PreparedProcess, ProcessGroupGuard,
    clear_managed_environment, run_foreground_process,
};
pub(crate) use runner::{
    BoundedCommandOutcome, run_bounded_command_with_stdin, run_bounded_command_with_stdin_outcome,
};
pub(crate) use script::{
    ensure_script_success, read_script, resolve_argv, run_resolved_script_with_outcome, run_script,
    validate_max_output_bytes, validate_script_target,
};
