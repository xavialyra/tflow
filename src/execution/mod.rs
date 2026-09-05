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
    MANAGED_ENVIRONMENT, PreparedProcess, ProcessGroupGuard, clear_managed_environment,
};
pub(crate) use runner::run_bounded_command_with_stdin;
pub(crate) use script::{
    ensure_script_success, read_script, resolve_argv, run_resolved_script, run_script,
    validate_max_output_bytes, validate_script_target,
};

