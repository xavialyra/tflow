mod app;
mod command;
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

pub fn run() -> anyhow::Result<i32> {
    app::run()
}
