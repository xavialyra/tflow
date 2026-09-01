mod keymap;
mod render;
mod session;

use self::keymap::{CaptureAction, CaptureKeymap};
pub(crate) use self::render::CaptureRenderer;
use self::session::CaptureSession;
use super::{
    ActionId, BackgroundOutcome, EngineActionInput, EngineDecision, EngineEmission, EngineNotice,
    EngineRuntime, EngineValidationContext, EvaluatedEngineConfig, InputBindingFactoryContext,
    RenderModel, RendererFactoryContext, RuntimeFactoryContext, ViewContextPublication,
    evaluate_field, evaluate_optional_string, require_field, validate_fields,
};
use crate::command::ResolvedInputAction;
use crate::config::{
    Defaults, ENGINE_CAPTURE, ResolvedScriptSource, ScriptSourceSpec, View, toml_to_json,
};
use crate::execution::{ensure_script_success, run_script};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

pub(super) fn definition() -> crate::engine::EngineDefinition {
    crate::engine::EngineDefinition::new(ENGINE_CAPTURE, "capture")
        .with_current_fields(&["value"])
        .with_factory_fields(crate::engine::FactoryFieldPlan {
            runtime: &["title", "output"],
            binding: &[],
            deferred_runtime_errors: &["title", "output"],
            binding_defaults: Some(&["defaults", "capture", "bindings"]),
        })
        .with_input_policy(crate::engine::InputPolicy {
            strategy: crate::input::InputStrategy::Decoded,
            buffer_target: None,
            focus: crate::engine::InputFocus::Unfocused,
        })
        .with_mount_policy(crate::engine::MountPolicy {
            launcher_input_timeout: Some(80),
            terminal_eof: crate::engine::TerminalEofPolicy::Exit,
            ..crate::engine::MountPolicy::default()
        })
        .with_actions([
            crate::engine::ActionSpec::unit("capture.copy"),
            crate::engine::ActionSpec::unit("capture.back"),
        ])
}

fn reject_picker_sources(name: &str, view: &View) -> Result<()> {
    if view.selected_items().is_some() || !view.selected_feeds().is_empty() {
        bail!(
            "view {:?} using engine {:?} cannot provide picker items",
            name,
            view.selected_engine_type()
        );
    }
    Ok(())
}

pub(super) fn validate_config(context: EngineValidationContext<'_>) -> Result<()> {
    let name = context.view_ref;
    let view = context.view;
    reject_picker_sources(name, view)?;
    validate_fields(name, view, &["output", "title"])?;
    require_field(name, view, "output")?;
    if let Some(title) = view.engine_field("title")
        && !title.is_str()
    {
        anyhow::bail!("view {:?} capture title must be a string or template", name);
    }
    let output = view
        .engine_field("output")
        .expect("required capture output was checked");
    if !output.is_str() {
        let source = ScriptSourceSpec::parse(output)
            .with_context(|| format!("view {:?} capture output", name))?;
        source
            .validate_capture_source()
            .with_context(|| format!("view {:?} capture output", name))?;
        let root = context
            .script_root
            .with_context(|| format!("view {:?} has no plugin root", name))?;
        source
            .validate_target(root)
            .with_context(|| format!("view {:?} has invalid capture source target", name))?;
    }
    Ok(())
}

pub(super) fn validate_defaults(defaults: &Defaults) -> Result<()> {
    let bindings = defaults
        .capture
        .bindings
        .as_ref()
        .map(toml_to_json)
        .transpose()?;
    CaptureKeymap::validate_values(bindings.as_ref(), None).context("capture bindings")
}

pub(super) fn validate_keymap(name: &str, view: &View) -> Result<()> {
    let keymap = view.keymap.as_ref().map(toml_to_json).transpose()?;
    CaptureKeymap::validate_values(None, keymap.as_ref())
        .with_context(|| format!("view {:?} capture keymap", name))
}

#[derive(Clone)]
struct PendingCaptureScript {
    root: PathBuf,
    source: ResolvedScriptSource,
    args: Vec<String>,
}

enum PreparedCaptureOutput {
    Text(String),
    Script {
        root: PathBuf,
        source: ResolvedScriptSource,
        args: Vec<String>,
    },
}

pub(super) fn create_view(
    context: RuntimeFactoryContext,
) -> Result<Box<dyn crate::engine::EngineRuntime>> {
    let default_title = context.identity.view_ref.clone();
    let evaluated = (|| {
        let title = evaluate_optional_string(&context.config, "title")?
            .unwrap_or_else(|| default_title.clone());
        let output = prepare_output(&context.config, context.config.plugin_root.as_deref())?;
        Ok::<_, anyhow::Error>((title, output))
    })();
    let (title, output, status, success, pending_script) = match evaluated {
        Ok((title, PreparedCaptureOutput::Text(output))) => (
            title.clone(),
            output.clone(),
            "finished successfully".to_string(),
            true,
            None,
        ),
        Ok((title, PreparedCaptureOutput::Script { root, source, args })) => (
            title,
            String::new(),
            "starting".to_string(),
            false,
            Some(PendingCaptureScript { root, source, args }),
        ),
        Err(error) => (
            default_title,
            error.to_string(),
            "failed".to_string(),
            false,
            None,
        ),
    };
    Ok(Box::new(CaptureView {
        view_ref: context.identity.view_ref,
        session: CaptureSession::new(&title, &output),
        status,
        success,
        reported: false,
        pending_script,
        script_task: None,
        script_completion: None,
    }))
}

pub(super) fn create_renderer(
    _context: RendererFactoryContext,
) -> Result<Box<dyn crate::engine::ViewRenderer>> {
    Ok(Box::new(CaptureRenderer))
}

pub(crate) fn create_input_bindings(
    context: InputBindingFactoryContext,
) -> Result<Vec<crate::command::InputActionBinding>> {
    let keymap =
        CaptureKeymap::from_values(context.bindings.defaults, context.bindings.view_keymap)?;
    Ok(keymap
        .bindings()
        .map(|(key, action)| crate::command::InputActionBinding {
            key,
            action: ResolvedInputAction::Engine(ActionId::new(match action {
                CaptureAction::Copy => "capture.copy",
                CaptureAction::Back => "capture.back",
            })),
            label: Some(action.label().to_string()),
            enabled: true,
        })
        .collect())
}

fn prepare_output(
    config: &EvaluatedEngineConfig,
    plugin_root: Option<&Path>,
) -> Result<PreparedCaptureOutput> {
    let output =
        evaluate_field(config, "output")?.context("capture engine requires an output field")?;
    if let Some(output) = output.as_str() {
        return Ok(PreparedCaptureOutput::Text(output.to_string()));
    }

    let source = ResolvedScriptSource::parse(&output)
        .context("capture output must evaluate to a string or script source")?;
    let root = plugin_root
        .context("capture source has no plugin root")?
        .to_path_buf();
    let args = source.script_args("capture script args")?;
    Ok(PreparedCaptureOutput::Script { root, source, args })
}

fn run_capture_script(
    plan: &PendingCaptureScript,
    cancellation: &crate::lifecycle::CancellationObserver,
) -> Result<String> {
    let output = run_script(
        &plan.root,
        &plan.source.file,
        &plan.args,
        plan.source.max_output_bytes,
        cancellation,
    )?;
    ensure_script_success(&output)?;
    if output.stdout.is_empty() {
        bail!("script produced no JSON output");
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("script {} did not produce valid JSON", plan.source.file))?;
    value
        .as_str()
        .map(str::to_string)
        .context("capture script source must produce a JSON string")
}

#[derive(Clone)]
enum CaptureCompletion {
    Completed(String),
    Failed(String),
    Cancelled,
}

struct CaptureView {
    view_ref: String,
    session: CaptureSession,
    status: String,
    success: bool,
    reported: bool,
    pending_script: Option<PendingCaptureScript>,
    script_task: Option<crate::task::TaskHandle<String>>,
    script_completion: Option<CaptureCompletion>,
}

#[derive(Debug, Clone)]
struct CaptureRenderModel {
    lines: std::sync::Arc<[String]>,
    status: String,
    title: String,
}

impl CaptureView {
    fn receive_completion(&mut self) -> Result<Option<CaptureCompletion>> {
        if let Some(completion) = self.script_completion.take() {
            return Ok(Some(completion));
        }
        let Some(task) = &mut self.script_task else {
            return Ok(None);
        };
        let completion = match task.try_recv() {
            Ok(crate::task::TaskCompletion::Completed(output)) => {
                CaptureCompletion::Completed(output)
            }
            Ok(crate::task::TaskCompletion::Failed(error)) => CaptureCompletion::Failed(error),
            Ok(crate::task::TaskCompletion::Cancelled) => CaptureCompletion::Cancelled,
            Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => CaptureCompletion::Failed(
                "capture script stopped before producing a result".to_string(),
            ),
        };
        Ok(Some(completion))
    }

    fn apply_completion(
        &mut self,
        completion: CaptureCompletion,
    ) -> (EngineNotice, ViewContextPublication) {
        let title = self.session.title();
        let (session, status, success) = match completion {
            CaptureCompletion::Completed(output) => (
                CaptureSession::new(title, &output),
                "finished successfully".to_string(),
                true,
            ),
            CaptureCompletion::Failed(error) => (
                CaptureSession::new(title, &error),
                "failed".to_string(),
                false,
            ),
            CaptureCompletion::Cancelled => (
                CaptureSession::new(title, "capture script was cancelled"),
                "failed".to_string(),
                false,
            ),
        };
        self.session = session;
        self.status = status.clone();
        self.success = success;
        self.reported = true;
        self.pending_script = None;
        self.script_task = None;
        self.script_completion = None;
        let notice = if success {
            EngineNotice::Info {
                view_ref: self.view_ref.clone(),
                message: status,
            }
        } else {
            EngineNotice::Error {
                view_ref: self.view_ref.clone(),
                message: status,
            }
        };
        let current = if success {
            serde_json::json!({"value": self.session.output()})
        } else {
            serde_json::Value::Null
        };
        (notice, ViewContextPublication::new(current))
    }
}

impl EngineRuntime for CaptureView {
    fn action(&mut self, input: EngineActionInput) -> Result<EngineEmission> {
        let decision = match input.invocation.id.as_str() {
            "capture.copy" if self.success => EngineDecision::Execute(
                crate::engine::EffectRequest::CopyToClipboard(self.session.output().to_string()),
            ),
            "capture.copy" => EngineDecision::Continue,
            "capture.back" => EngineDecision::Close,
            action => bail!("unknown capture action {:?}", action),
        };
        Ok(EngineEmission::decision(decision))
    }

    fn parameters(
        &mut self,
        _parameters: crate::parameter::ParameterSnapshot,
        _expected: crate::engine::ViewContextIdentity,
    ) -> Result<EngineEmission> {
        Ok(EngineEmission::decision(EngineDecision::Invalidate))
    }

    fn tick(&mut self, _tick: crate::engine::EngineTick) -> Result<EngineEmission> {
        if self.pending_script.is_some()
            || self.script_task.is_some()
            || self.script_completion.is_some()
            || self.reported
        {
            return Ok(EngineEmission::decision(EngineDecision::Continue));
        }
        self.reported = true;
        let current = if self.success {
            serde_json::json!({"value": self.session.output()})
        } else {
            serde_json::Value::Null
        };
        let decision = if self.success {
            EngineDecision::Report(EngineNotice::Info {
                view_ref: self.view_ref.clone(),
                message: self.status.clone(),
            })
        } else {
            EngineDecision::Report(EngineNotice::Error {
                view_ref: self.view_ref.clone(),
                message: self.status.clone(),
            })
        };
        Ok(EngineEmission::decision(decision)
            .with_publication(ViewContextPublication::new(current)))
    }

    fn tick_mode(&self) -> crate::engine::EngineTickMode {
        crate::engine::EngineTickMode::Prepared
    }

    fn start_prepared_work(
        &mut self,
        starter: &crate::task::MountTaskStarter,
        runtime_snapshot: &serde_json::Value,
    ) {
        if self.script_task.is_some() || self.script_completion.is_some() {
            return;
        }
        let Some(plan) = self.pending_script.clone() else {
            return;
        };
        self.script_completion = None;
        self.script_task = Some(starter.spawn_latest_with_snapshot(
            "capture-script",
            runtime_snapshot.clone(),
            move |context| {
                run_capture_script(&plan, &context.cancellation.observer())
                    .map_err(|error| error.to_string())
            },
        ));
    }

    fn poll_work(&mut self) -> Result<Option<EngineEmission>> {
        let Some(completion) = self.receive_completion()? else {
            return Ok(None);
        };
        let (notice, publication) = self.apply_completion(completion);
        Ok(Some(
            EngineEmission::decision(EngineDecision::Report(notice)).with_publication(publication),
        ))
    }

    fn poll_background_work(&mut self) -> Result<Option<BackgroundOutcome>> {
        let Some(completion) = self.receive_completion()? else {
            return Ok(None);
        };
        let (notice, publication) = self.apply_completion(completion);
        Ok(Some(BackgroundOutcome {
            notices: vec![notice],
            publication: Some(publication),
        }))
    }

    fn deactivate(&mut self) {
        self.script_task = None;
        self.script_completion = None;
    }

    fn render_model(&self) -> RenderModel {
        RenderModel::new(
            "capture",
            CaptureRenderModel {
                lines: self.session.shared_lines(),
                status: self.status.clone(),
                title: self.session.title().to_string(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn script_starts_as_prepared_work_and_restarts_after_reactivation() {
        let root =
            std::env::temp_dir().join(format!("tui-launcher-capture-start-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let script = root.join("capture.sh");
        let marker = root.join("started");
        let release = root.join("release");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf started > '{}'\nwhile [ ! -f '{}' ]; do sleep 0.01; done\nprintf '\"captured\"\\n'\n",
                marker.display(),
                release.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let cancellation = crate::lifecycle::CancellationToken::new();
        let context = RuntimeFactoryContext {
            identity: crate::engine::ViewIdentity::new("core:capture", ENGINE_CAPTURE),
            config: EvaluatedEngineConfig {
                fields: [(
                    "output".to_string(),
                    serde_json::json!({
                        "source": "script",
                        "file": "capture.sh"
                    }),
                )]
                .into_iter()
                .collect(),
                plugin_root: Some(root.clone()),
                ..EvaluatedEngineConfig::default()
            },
            parameters: crate::parameter::ParameterSnapshot::from_parts(
                serde_json::Value::Null,
                String::new(),
                crate::input::InputSourceIdentity {
                    frame: crate::input::ViewMountId(1),
                    generation: 0,
                },
                0,
            ),
            cancellation: cancellation.observer(),
            data: None,
        };

        let mut runtime = create_view(context).unwrap();
        assert!(
            !marker.exists(),
            "capture script ran during mount preparation"
        );

        let tasks = crate::task::TaskRuntime::new();
        let starter = crate::task::MountTaskStarter::from_lease(
            &tasks,
            crate::task::MountTaskLease::new(crate::input::ViewMountId(1)),
        );
        runtime.start_prepared_work(&starter, &serde_json::Value::Null);
        runtime.deactivate();
        runtime.start_prepared_work(&starter, &serde_json::Value::Null);

        fs::write(&release, "release").unwrap();
        let mut emission = None;
        for _ in 0..200 {
            if let Some(next) = runtime.poll_work().unwrap() {
                emission = Some(next);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let emission = emission.expect("capture script should complete");
        assert!(matches!(emission.decision_ref(), EngineDecision::Report(_)));
        assert_eq!(fs::read_to_string(&marker).unwrap(), "started");
        assert!(runtime.poll_work().unwrap().is_none());

        let model = runtime.render_model();
        let model = model.downcast_ref::<CaptureRenderModel>().unwrap();
        assert_eq!(model.lines.as_ref(), &["captured"]);
        tasks.shutdown_and_wait();
        fs::remove_dir_all(root).unwrap();
    }
}
