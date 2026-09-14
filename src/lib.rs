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

#[cfg(test)]
#[test]
#[ignore = "collects reproducible scheduler evidence and writes target/stage5 artifacts"]
fn stage_5_scheduler_evidence() {
    task::run_stage_5_scheduler_evidence();
}

pub fn run() -> anyhow::Result<i32> {
    app::run()
}
