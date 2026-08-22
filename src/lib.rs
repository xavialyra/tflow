mod app;
mod diagnostics;
mod engine;
mod execution;
mod input;
mod lifecycle;
mod session;
mod terminal;
mod ui;
mod workflow;

// Keep the established crate paths while the implementation lives in the
// architectural domains above.
pub(crate) use ui::{chrome, theme};
pub(crate) use workflow::{
    command, config, expression, navigation as router, query as state, runtime,
};

pub fn run() -> anyhow::Result<i32> {
    app::run()
}
