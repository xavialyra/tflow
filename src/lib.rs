mod app;
mod diagnostics;
mod engine;
mod execution;
mod input;
mod lifecycle;
mod protocol;
mod task;
mod terminal;
mod ui;
mod view;
mod workflow;

// Keep the established crate paths while the implementation lives in the
// architectural domains above.
pub(crate) use ui::{chrome, theme};
pub(crate) use workflow::{command, config, expression, navigation as router, parameter, runtime};

pub fn run() -> anyhow::Result<i32> {
    app::run()
}
