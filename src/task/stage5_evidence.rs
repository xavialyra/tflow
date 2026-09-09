use super::{
    MountTaskLease, MountTaskStarter, RECENT_TERMINAL_LIMIT, TaskCancellationReason,
    TaskCompletion, TaskElapsed, TaskRuntime, TaskRuntimeMetricsSnapshot, TaskTags,
    TaskTerminalOutcome, TaskTerminalSample,
};
use crate::execution::run_bounded_command_with_stdin_outcome;
use crate::input::ViewMountId;
use crate::protocol::contracts::{TaskEvent, TaskId, TaskOutcome, ViewInstanceId};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const RUNS: usize = 5;
const ITERATIONS: usize = 40;
const TERMINALS_PER_ITERATION: usize = 5;
const WAIT: Duration = Duration::from_secs(5);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const OUTPUT_LIMIT: usize = 4096;
const SLOW_SCRIPT: &str = "printf ready > \"$1\"; sleep 30";
const SLOW_COMMAND: &str = "sh -c 'printf ready > \"$1\"; sleep 30' stage5 <ready-file>";
const METRICS: [&str; 4] = [
    "capture_queue_wait",
    "picker_cancellation_latency",
    "picker_cancellation_to_reap",
    "final_picker_runtime",
];

pub(super) fn run() {
    let started_utc = utc();
    let directory = artifact_directory(&started_utc);
    let profile = build_profile();
    let invocation = collector_invocation(profile);
    let environment = environment();
    let mut pooled = BTreeMap::<String, Vec<u64>>::new();
    let mut measurements_by_run = Vec::<BTreeMap<String, Vec<u64>>>::new();
    let mut runs = Vec::new();

    for run_index in 0..RUNS {
        let runtime = TaskRuntime::new();
        let mount = ViewMountId(10_000 + run_index as u64);
        let starter = MountTaskStarter::from_lease(&runtime, MountTaskLease::new(mount));
        let mut measurements = metric_map();

        for iteration in 0..ITERATIONS {
            let before = runtime.metrics_snapshot().recent_terminal.len();
            let base = (iteration * TERMINALS_PER_ITERATION) as u64;
            let generation = iteration as u64 + 1;
            let ready = directory.join(format!("ready-{run_index}-{iteration}"));
            let slow_ready = ready.clone();
            let mut slow = starter
                .for_task(TaskId(base), generation)
                .spawn_latest_with_test_snapshot_tagged(
                    "picker-items",
                    json!({"run": run_index, "iteration": iteration, "role": "slow-picker"}),
                    TaskTags::new("picker", "items"),
                    move |context| {
                        let mut command = Command::new("sh");
                        command
                            .arg("-c")
                            .arg(SLOW_SCRIPT)
                            .arg("stage5")
                            .arg(&slow_ready);
                        let outcome = run_bounded_command_with_stdin_outcome(
                            command,
                            None,
                            COMMAND_TIMEOUT,
                            OUTPUT_LIMIT,
                            OUTPUT_LIMIT,
                            &context.cancellation,
                        );
                        if outcome.managed_child_reaped() {
                            context.mark_process_reaped();
                        }
                        outcome
                            .into_result()
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    },
                );
            wait_for_file(&ready);
            assert!(
                runtime
                    .metrics_snapshot()
                    .current_active
                    .is_some_and(|active| {
                        active.tags == TaskTags::new("picker", "items")
                            && active.started_at.is_some()
                    })
            );

            let mut capture = starter
                .for_task(TaskId(base + 1), generation)
                .spawn_latest_with_test_snapshot_tagged(
                    "capture-script",
                    json!({"run": run_index, "iteration": iteration, "role": "capture"}),
                    TaskTags::new("capture", "script"),
                    |_| Ok(()),
                );
            let mut first = replacement(
                &starter,
                base + 2,
                generation,
                run_index,
                iteration,
                "first",
            );
            let mut second = replacement(
                &starter,
                base + 3,
                generation,
                run_index,
                iteration,
                "second",
            );
            let mut final_picker = replacement(
                &starter,
                base + 4,
                generation,
                run_index,
                iteration,
                "final",
            );

            assert!(matches!(
                receive(&mut slow, "slow Picker"),
                TaskCompletion::Cancelled
            ));
            assert!(matches!(
                receive(&mut capture, "Capture"),
                TaskCompletion::Completed(())
            ));
            assert!(matches!(
                receive(&mut first, "first pending Picker"),
                TaskCompletion::Cancelled
            ));
            assert!(matches!(
                receive(&mut second, "second pending Picker"),
                TaskCompletion::Cancelled
            ));
            assert!(matches!(
                receive(&mut final_picker, "final Picker"),
                TaskCompletion::Completed(())
            ));
            validate_events(
                wait_for_events(&runtime, TERMINALS_PER_ITERATION),
                mount,
                base,
                generation,
            );

            let snapshot = runtime.metrics_snapshot();
            let terminals = &snapshot.recent_terminal[before..];
            assert_eq!(terminals.len(), TERMINALS_PER_ITERATION);
            let slow_sample = terminals
                .iter()
                .find(|sample| {
                    sample.tags == TaskTags::new("picker", "items")
                        && sample.cancellation.as_ref().is_some_and(|cancellation| {
                            cancellation.reason
                                == TaskCancellationReason::LatestWinsActiveReplacement
                        })
                })
                .expect("slow Picker terminal sample missing");
            assert_eq!(slow_sample.outcome, TaskTerminalOutcome::Cancelled);
            let cancellation = slow_sample
                .cancellation
                .as_ref()
                .expect("slow Picker cancellation missing");
            let reaped = slow_sample
                .process_reaped_at
                .expect("slow Picker child was not reaped");
            assert!(
                cancellation.requested_at
                    >= slow_sample.started_at.expect("slow Picker start missing")
            );
            assert!(reaped >= cancellation.requested_at && slow_sample.terminal_at >= reaped);
            let pending = terminals
                .iter()
                .filter(|sample| {
                    sample.tags == TaskTags::new("picker", "items")
                        && sample.started_at.is_none()
                        && sample.outcome == TaskTerminalOutcome::Cancelled
                        && sample.cancellation.as_ref().is_some_and(|cancellation| {
                            cancellation.reason
                                == TaskCancellationReason::LatestWinsPendingReplacement
                        })
                })
                .count();
            assert_eq!(pending, 2);
            let capture_sample = terminals
                .iter()
                .find(|sample| sample.tags == TaskTags::new("capture", "script"))
                .expect("Capture terminal sample missing");
            assert_eq!(capture_sample.outcome, TaskTerminalOutcome::Completed);
            let capture_started = capture_sample.started_at.expect("Capture start missing");
            assert!(capture_started >= slow_sample.terminal_at);
            let final_sample = terminals
                .iter()
                .find(|sample| {
                    sample.tags == TaskTags::new("picker", "items")
                        && sample.outcome == TaskTerminalOutcome::Completed
                })
                .expect("final Picker terminal sample missing");
            let final_started = final_sample.started_at.expect("final Picker start missing");
            measurements
                .get_mut("capture_queue_wait")
                .unwrap()
                .push(delta(capture_started, capture_sample.submitted_at));
            measurements
                .get_mut("picker_cancellation_latency")
                .unwrap()
                .push(delta(slow_sample.terminal_at, cancellation.requested_at));
            measurements
                .get_mut("picker_cancellation_to_reap")
                .unwrap()
                .push(delta(reaped, cancellation.requested_at));
            measurements
                .get_mut("final_picker_runtime")
                .unwrap()
                .push(delta(final_sample.terminal_at, final_started));
        }

        runtime.shutdown_and_wait();
        let snapshot = runtime.metrics_snapshot();
        validate_run(&runtime, &snapshot);
        for name in METRICS {
            assert_eq!(measurements[name].len(), ITERATIONS);
            pooled
                .entry(name.to_string())
                .or_default()
                .extend_from_slice(&measurements[name]);
        }
        runs.push(json!({
            "run_index": run_index,
            "metrics_after_shutdown": metrics_json(&snapshot),
            "measurements_ns": measurements.iter().map(|(name, values)| (name, values.iter().map(ToString::to_string).collect::<Vec<_>>())).collect::<BTreeMap<_, _>>(),
            "statistics": measurements.iter().map(|(name, values)| (name, stats(values))).collect::<BTreeMap<_, _>>(),
            "terminal_samples": snapshot.recent_terminal.iter().map(terminal_json).collect::<Vec<_>>(),
        }));
        measurements_by_run.push(measurements);
    }

    let ended_utc = utc();
    let summaries = summaries(&pooled, &measurements_by_run);
    let raw = json!({
        "schema": "tlaunch.stage5.scheduler-evidence.v1",
        "collector": {
            "test": "stage_5_scheduler_evidence",
            "profile": profile,
            "invoke": invocation,
            "utc_started": started_utc,
            "utc_ended": ended_utc,
        },
        "environment": environment,
        "scenario": {
            "runs": RUNS, "iterations_per_run": ITERATIONS,
            "terminal_records_per_iteration": TERMINALS_PER_ITERATION,
            "retained_terminal_limit": RECENT_TERMINAL_LIMIT,
            "slow_picker_command": SLOW_COMMAND,
            "slow_picker_command_example": format!("sh -c 'printf ready > \\\"$1\\\"; sleep 30' stage5 {}", directory.join("ready-0-0").display()),
            "slow_command_timeout_ms": COMMAND_TIMEOUT.as_millis().to_string(),
            "process_output_limit_bytes": OUTPUT_LIMIT,
            "ready_signal": "slow shell writes a per-iteration ready file before sleeping",
            "lanes": {"picker": "picker-items", "capture": "capture-script"},
            "tags": {"picker": ["picker", "items"], "capture": ["capture", "script"]},
        },
        "runs": runs,
        "summary": summaries,
    });
    fs::write(
        directory.join("raw.json"),
        serde_json::to_vec_pretty(&raw).unwrap(),
    )
    .unwrap();
    fs::write(directory.join("summary.md"), markdown(&raw, &summaries)).unwrap();
    println!("Stage 5 scheduler evidence: {}", directory.display());
}

fn replacement(
    starter: &MountTaskStarter,
    task: u64,
    generation: u64,
    run: usize,
    iteration: usize,
    role: &str,
) -> super::TaskHandle<()> {
    starter
        .for_task(TaskId(task), generation)
        .spawn_latest_with_test_snapshot_tagged(
            "picker-items",
            json!({"run": run, "iteration": iteration, "role": role}),
            TaskTags::new("picker", "items"),
            |_| Ok(()),
        )
}

fn receive<T>(handle: &mut super::TaskHandle<T>, label: &str) -> TaskCompletion<T> {
    let deadline = Instant::now() + WAIT;
    loop {
        match handle.try_recv() {
            Ok(value) => return value,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(1))
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                panic!("{label} exceeded bounded completion deadline")
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                panic!("{label} completion channel disconnected")
            }
        }
    }
}

fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if path.is_file() {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!(
        "slow managed shell did not signal readiness at {}",
        path.display()
    );
}

fn wait_for_events(runtime: &TaskRuntime, expected: usize) -> Vec<TaskEvent> {
    let deadline = Instant::now() + WAIT;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        events.extend(runtime.drain_events());
        if events.len() == expected {
            return events;
        }
        assert!(events.len() < expected, "too many correlated events");
        thread::sleep(Duration::from_millis(1));
    }
    panic!("received {} of {expected} correlated events", events.len());
}

fn validate_events(events: Vec<TaskEvent>, mount: ViewMountId, base: u64, generation: u64) {
    let expected = BTreeMap::from([
        (TaskId(base), TaskOutcome::Cancelled),
        (TaskId(base + 1), TaskOutcome::Completed(Value::Null)),
        (TaskId(base + 2), TaskOutcome::Cancelled),
        (TaskId(base + 3), TaskOutcome::Cancelled),
        (TaskId(base + 4), TaskOutcome::Completed(Value::Null)),
    ]);
    for event in events {
        assert_eq!(event.instance, ViewInstanceId(mount.0));
        assert_eq!(event.generation, generation);
        assert_eq!(expected.get(&event.task), Some(&event.outcome));
    }
}

fn validate_run(runtime: &TaskRuntime, metrics: &TaskRuntimeMetricsSnapshot) {
    let terminal_count = ITERATIONS * TERMINALS_PER_ITERATION;
    assert!(terminal_count < RECENT_TERMINAL_LIMIT);
    assert_eq!(metrics.recent_terminal.len(), terminal_count);
    assert_eq!(metrics.submitted_total, terminal_count as u64);
    assert_eq!(metrics.started_total, (ITERATIONS * 3) as u64);
    assert_eq!(metrics.completed_total, (ITERATIONS * 2) as u64);
    assert_eq!(metrics.cancelled_total, (ITERATIONS * 3) as u64);
    assert_eq!(metrics.failed_total, 0);
    assert_eq!(metrics.panicked_total, 0);
    assert_eq!(metrics.queue_high_water, 2);
    assert_eq!(metrics.queue_depth, 0);
    assert_eq!(metrics.active_tasks, 0);
    assert!(!runtime.has_active_tasks() && !runtime.has_pending_events());
    assert_eq!(metrics.worker_started_total, 1);
    assert_eq!(metrics.worker_joined_total, 1);
}

fn ns(duration: Duration) -> u64 {
    duration
        .as_nanos()
        .try_into()
        .expect("duration exceeds u64 nanoseconds")
}
fn delta(later: TaskElapsed, earlier: TaskElapsed) -> u64 {
    ns(later
        .0
        .checked_sub(earlier.0)
        .expect("timestamps not monotonic"))
}
fn outcome_name(outcome: TaskTerminalOutcome) -> &'static str {
    match outcome {
        TaskTerminalOutcome::Completed => "completed",
        TaskTerminalOutcome::Failed => "failed",
        TaskTerminalOutcome::Panicked => "panicked",
        TaskTerminalOutcome::Cancelled => "cancelled",
    }
}
fn reason_name(reason: TaskCancellationReason) -> &'static str {
    match reason {
        TaskCancellationReason::LatestWinsPendingReplacement => "latest_wins_pending_replacement",
        TaskCancellationReason::LatestWinsActiveReplacement => "latest_wins_active_replacement",
        TaskCancellationReason::MountClosed => "mount_closed",
        TaskCancellationReason::ExplicitHandle => "explicit_handle",
        TaskCancellationReason::DroppedHandle => "dropped_handle",
        TaskCancellationReason::RuntimeCancellation => "runtime_cancellation",
        TaskCancellationReason::ShutdownActive => "shutdown_active",
        TaskCancellationReason::ShutdownQueueDrain => "shutdown_queue_drain",
        TaskCancellationReason::SubmissionAfterShutdown => "submission_after_shutdown",
    }
}

fn terminal_json(sample: &TaskTerminalSample) -> Value {
    json!({"tags": {"engine": sample.tags.engine, "task_class": sample.tags.task_class}, "lane": sample.lane,
        "submitted_at_ns": ns(sample.submitted_at.0).to_string(), "started_at_ns": sample.started_at.map(|time| ns(time.0).to_string()),
        "cancellation": sample.cancellation.as_ref().map(|c| json!({"reason": reason_name(c.reason), "requested_at_ns": ns(c.requested_at.0).to_string()})),
        "terminal_at_ns": ns(sample.terminal_at.0).to_string(), "outcome": outcome_name(sample.outcome),
        "process_reaped_at_ns": sample.process_reaped_at.map(|time| ns(time.0).to_string())})
}

fn metrics_json(metrics: &TaskRuntimeMetricsSnapshot) -> Value {
    json!({"now_ns": ns(metrics.now.0).to_string(), "queue_depth": metrics.queue_depth, "active_tasks": metrics.active_tasks,
        "queue_high_water": metrics.queue_high_water, "submitted_total": metrics.submitted_total, "started_total": metrics.started_total,
        "completed_total": metrics.completed_total, "failed_total": metrics.failed_total, "panicked_total": metrics.panicked_total,
        "cancelled_total": metrics.cancelled_total, "submission_after_shutdown_total": metrics.submission_after_shutdown_total,
        "worker_started_total": metrics.worker_started_total, "worker_joined_total": metrics.worker_joined_total,
        "recent_terminal_count": metrics.recent_terminal.len()})
}

fn metric_map() -> BTreeMap<String, Vec<u64>> {
    METRICS
        .into_iter()
        .map(|name| (name.to_string(), Vec::with_capacity(ITERATIONS)))
        .collect()
}
fn rank(values: &[u64], percentile: usize) -> u64 {
    assert!(!values.is_empty());
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted[((percentile * sorted.len()).div_ceil(100).max(1)) - 1]
}
fn stats(values: &[u64]) -> Value {
    json!({"count": values.len(), "p50_ns": rank(values, 50).to_string(), "p95_ns": rank(values, 95).to_string(), "max_ns": values.iter().max().unwrap().to_string()})
}

fn summaries(
    pooled: &BTreeMap<String, Vec<u64>>,
    by_run: &[BTreeMap<String, Vec<u64>>],
) -> BTreeMap<&'static str, Value> {
    METRICS.into_iter().map(|name| {
        let p50 = by_run.iter().map(|run| rank(&run[name], 50)).collect::<Vec<_>>();
        let p95 = by_run.iter().map(|run| rank(&run[name], 95)).collect::<Vec<_>>();
        let max = by_run.iter().map(|run| *run[name].iter().max().unwrap()).collect::<Vec<_>>();
        (name, json!({"pooled": stats(&pooled[name]), "median_of_run_p50_ns": rank(&p50, 50).to_string(), "median_of_run_p95_ns": rank(&p95, 50).to_string(), "median_of_run_max_ns": rank(&max, 50).to_string()}))
    }).collect()
}

fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
        .map(|text| text.trim().to_string())
}
fn utc() -> String {
    command_stdout("date", &["-u", "+%Y%m%d-%H%M%SZ"]).unwrap_or_else(|| "unknown-utc".to_string())
}
fn artifact_directory(timestamp: &str) -> PathBuf {
    let root = std::env::current_dir().unwrap().join("target/stage5");
    fs::create_dir_all(&root).unwrap();
    for suffix in 0..10_000_u32 {
        let name = if suffix == 0 {
            format!("scheduler-{timestamp}")
        } else {
            format!("scheduler-{timestamp}-{suffix}")
        };
        let path = root.join(name);
        match fs::create_dir(&path) {
            Ok(()) => return path,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!("could not create {}: {error}", path.display()),
        }
    }
    panic!("could not allocate non-overwriting Stage 5 artifact directory")
}
fn build_profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

fn collector_invocation(profile: &str) -> &'static str {
    match profile {
        "release" => {
            "cargo test --release --lib stage_5_scheduler_evidence -- --ignored --exact --nocapture"
        }
        _ => "cargo test --lib stage_5_scheduler_evidence -- --ignored --exact --nocapture",
    }
}

fn environment() -> Value {
    json!({"rustc": command_stdout("rustc", &["--version"]), "os": std::env::consts::OS, "kernel": command_stdout("uname", &["-srmo"]), "arch": std::env::consts::ARCH, "cpu_count": std::thread::available_parallelism().ok().map(|n| n.get()), "git_commit": command_stdout("git", &["rev-parse", "HEAD"]), "git_dirty": command_stdout("git", &["status", "--porcelain"]).is_some_and(|s| !s.is_empty()), "cargo_lock_sha256": command_stdout("sha256sum", &["Cargo.lock"]).and_then(|s| s.split_whitespace().next().map(str::to_string))})
}
fn millis(nanos: u64) -> String {
    format!("{:.3}", nanos as f64 / 1_000_000.0)
}
fn markdown(raw: &Value, summaries: &BTreeMap<&str, Value>) -> String {
    let environment = &raw["environment"];
    let collector = &raw["collector"];
    let threshold = 250_000_000_u64;
    let mut text = format!(
        "# Stage 5 Scheduler Evidence\n\n- Schema: `tlaunch.stage5.scheduler-evidence.v1`\n- UTC start: `{}`\n- UTC end: `{}`\n- Build profile: `{}`\n- Invoke: `{}`\n- Slow managed command: `{SLOW_COMMAND}`\n- Workload: {RUNS} independent runs x {ITERATIONS} iterations x {TERMINALS_PER_ITERATION} terminal records = {}; each run retains {} records below the {}-record limit.\n\n## Environment\n\n- Git commit: `{}`\n- Git dirty: `{}`\n- Rust: `{}`\n- OS/kernel: `{}`\n- Architecture: `{}`\n- CPU count: `{}`\n- Cargo.lock SHA-256: `{}`\n\n## Pooled And Median-Of-Run Results\n\n| Metric | Pooled p50 | Pooled p95 | Pooled max | Median run p50 | Median run p95 | Median run max |\n| --- | ---: | ---: | ---: | ---: | ---: | ---: |\n",
        collector["utc_started"],
        collector["utc_ended"],
        collector["profile"],
        collector["invoke"],
        RUNS * ITERATIONS * TERMINALS_PER_ITERATION,
        ITERATIONS * TERMINALS_PER_ITERATION,
        RECENT_TERMINAL_LIMIT,
        environment["git_commit"],
        environment["git_dirty"],
        environment["rustc"],
        environment["kernel"],
        environment["arch"],
        environment["cpu_count"],
        environment["cargo_lock_sha256"]
    );
    for name in METRICS {
        let pooled = &summaries[name]["pooled"];
        text.push_str(&format!(
            "| {name} | {} ms | {} ms | {} ms | {} ms | {} ms | {} ms |\n",
            millis(pooled["p50_ns"].as_str().unwrap().parse().unwrap()),
            millis(pooled["p95_ns"].as_str().unwrap().parse().unwrap()),
            millis(pooled["max_ns"].as_str().unwrap().parse().unwrap()),
            millis(
                summaries[name]["median_of_run_p50_ns"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap()
            ),
            millis(
                summaries[name]["median_of_run_p95_ns"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap()
            ),
            millis(
                summaries[name]["median_of_run_max_ns"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap()
            )
        ));
    }
    text.push_str("\n## 250 ms P95 Targets\n\n| Metric | Pooled p95 | Target | Result |\n| --- | ---: | ---: | --- |\n");
    for name in METRICS {
        let p95: u64 = summaries[name]["pooled"]["p95_ns"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        text.push_str(&format!(
            "| {name} | {} ms | 250.000 ms | {} |\n",
            millis(p95),
            if p95 <= threshold { "PASS" } else { "FAIL" }
        ));
    }
    text.push_str("\n`raw.json` contains every retained terminal record with runtime-local timestamp values represented as nanosecond strings, all per-iteration measurements, and nearest-rank summaries. This collection does not itself make a scheduler-policy decision.\n");
    text
}
