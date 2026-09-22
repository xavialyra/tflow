mod keymap;
mod protocol;
mod render;
mod session;

pub(crate) use protocol::{CaptureProtocolConfig, create_protocol_view};

use self::keymap::{CaptureAction, CaptureKeymap};
pub(crate) use self::render::CaptureRenderer;
use self::session::CaptureSession;
use super::{
    BackgroundOutcome, EngineActionInput, EngineDecision, EngineEmission, EngineNotice,
    EngineRuntime, EngineValidationContext, ProjectedEngineConfig, RenderModel,
    RendererFactoryContext, RuntimeFactoryContext, ViewContextPublication, require_field,
    validate_fields,
};
use crate::input::keymap::KeymapAction;
use crate::workflow::config::{
    Defaults, ProducerKind, ResolvedScriptSource, View, parse_producer_script_handler, toml_to_json,
};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn definition() -> crate::engine::EngineDefinition {
    crate::engine::EngineDefinition::new()
        .with_factory_fields(crate::engine::FactoryFieldPlan {
            runtime: &["output"],
            binding: &[],
            binding_defaults: Some(&["defaults", "capture", "bindings"]),
        })
        .with_actions([
            crate::engine::ActionSpec::unit("capture.copy"),
            crate::engine::ActionSpec::unit("capture.back"),
        ])
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputProducerConfig {
    producer: ProducerKind,
    handler: toml::Value,
}

fn reject_picker_sources(name: &str, view: &View) -> Result<()> {
    if view.selected_items().is_some() {
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
    validate_fields(name, view, &["output"])?;
    require_field(name, view, "output")?;
    let output = view
        .engine_field("output")
        .expect("required capture output was checked");
    if output
        .as_table()
        .is_some_and(|fields| fields.contains_key("producer"))
    {
        let provider: OutputProducerConfig = output
            .clone()
            .try_into()
            .context("capture output producer must define producer and handler")?;
        match provider.producer {
            ProducerKind::Declared => validate_declared_output(&provider.handler)?,
            ProducerKind::Script => {
                parse_producer_script_handler(&provider.handler, context.script_root)
                    .context("capture output script handler is invalid")?;
            }
        }
    } else if !output.is_str() {
        bail!(
            "view {:?} capture output must be a string or producer object",
            name
        );
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

pub(super) fn validate_keymap(_name: &str, _view: &View) -> Result<()> {
    Ok(())
}

pub(crate) fn is_capture_action(name: &str) -> bool {
    self::keymap::CaptureAction::parse(name).is_some()
}

fn validate_declared_output(value: &toml::Value) -> Result<()> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Handler {
        output: String,
    }
    let handler: Handler = value
        .clone()
        .try_into()
        .context("declared capture output handler must define output")?;
    let _ = handler.output;
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredOutputHandler {
    output: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectedOutputProducer {
    producer: ProducerKind,
    handler: serde_json::Value,
}

#[derive(Clone)]
struct PendingCaptureScript {
    view_ref: String,
    root: Option<PathBuf>,
    source: ResolvedScriptSource,
    parameters: serde_json::Value,
    launch_input: serde_json::Value,
}

enum PreparedCaptureOutput {
    Text(String),
    Script {
        root: Option<PathBuf>,
        source: ResolvedScriptSource,
    },
}

pub(super) fn create_view(
    context: RuntimeFactoryContext,
) -> Result<Box<dyn crate::engine::EngineRuntime>> {
    let prepared = prepare_output(&context.config, context.config.workflow_root.as_deref());
    let (output, status, success, pending_script) = match prepared {
        Ok(PreparedCaptureOutput::Text(output)) => (output, String::new(), true, None),
        Ok(PreparedCaptureOutput::Script { root, source }) => {
            let view_ref = context.identity.view_ref.clone();
            (
                String::new(),
                "starting".to_string(),
                false,
                Some(PendingCaptureScript {
                    view_ref,
                    root,
                    source,
                    parameters: context.parameters.values().clone(),
                    launch_input: context.config.launch_input.clone(),
                }),
            )
        }
        Err(error) => (error.to_string(), "failed".to_string(), false, None),
    };
    Ok(Box::new(CaptureView {
        view_ref: context.identity.view_ref,
        session: CaptureSession::new(&output),
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

fn prepare_output(
    config: &ProjectedEngineConfig,
    workflow_root: Option<&Path>,
) -> Result<PreparedCaptureOutput> {
    let output = config
        .field("output")
        .context("capture engine requires an output field")?;
    if output
        .as_object()
        .is_some_and(|fields| fields.contains_key("producer"))
    {
        let provider: ProjectedOutputProducer = serde_json::from_value(output.clone())
            .context("capture output producer must define producer and handler")?;
        return match provider.producer {
            ProducerKind::Declared => {
                let handler: DeclaredOutputHandler = serde_json::from_value(provider.handler)
                    .context("declared capture output handler must define output")?;
                Ok(PreparedCaptureOutput::Text(handler.output))
            }
            ProducerKind::Script => {
                let handler = toml::Value::try_from(provider.handler)
                    .context("capture output script handler could not be converted to TOML")?;
                let source = parse_producer_script_handler(&handler, workflow_root)?;
                Ok(PreparedCaptureOutput::Script {
                    root: workflow_root.map(Path::to_path_buf),
                    source,
                })
            }
        };
    }
    output
        .as_str()
        .map(|output| PreparedCaptureOutput::Text(output.to_string()))
        .context("capture output must be a string or producer object")
}

struct CaptureScriptOutcome {
    result: Result<String>,
    managed_child_reaped: bool,
}

fn run_capture_script(
    plan: &PendingCaptureScript,
    cancellation: &crate::lifecycle::CancellationObserver,
) -> CaptureScriptOutcome {
    let request = crate::protocol::capture_request(
        &plan.parameters,
        &plan.launch_input,
        crate::workflow::config::ENGINE_CAPTURE,
        &Value::Null,
    );
    let response = crate::protocol::run_script_capture_response(
        &plan.view_ref,
        &format!("[views.{}.output]", plan.view_ref),
        plan.root.as_deref(),
        &plan.source,
        &request,
        cancellation,
    );
    CaptureScriptOutcome {
        result: response.result,
        managed_child_reaped: response.managed_child_reaped,
    }
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
        let (output, status, success) = match completion {
            CaptureCompletion::Completed(output) => (output, String::new(), true),
            CaptureCompletion::Failed(error) => (error, "failed".to_string(), false),
            CaptureCompletion::Cancelled => (
                "capture script was cancelled".to_string(),
                "failed".to_string(),
                false,
            ),
        };
        self.session = CaptureSession::new(&output);
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
        (
            notice,
            ViewContextPublication::new(current).with_ready(true),
        )
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
            .with_publication(ViewContextPublication::new(current).with_ready(true)))
    }

    fn start_prepared_work(&mut self, starter: &crate::task::MountTaskStarter) -> bool {
        if self.script_task.is_some() || self.script_completion.is_some() {
            return false;
        }
        let Some(plan) = self.pending_script.clone() else {
            return false;
        };
        self.script_completion = None;
        self.script_task = Some(starter.spawn_latest_tagged(
            "capture-script",
            crate::task::TaskTags::new("capture", "script"),
            move |context| {
                let outcome = run_capture_script(&plan, &context.cancellation.observer());
                if outcome.managed_child_reaped {
                    context.mark_process_reaped();
                }
                outcome.result.map_err(|error| error.to_string())
            },
        ));
        true
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
        let root = std::env::temp_dir().join(format!("tflow-capture-start-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let script = root.join("capture.sh");
        let marker = root.join("started");
        let release = root.join("release");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\ncat >/dev/null\nprintf started > '{}'\nwhile [ ! -f '{}' ]; do sleep 0.01; done\nprintf '{{\"version\":1,\"output\":\"captured\"}}\\n'\n",
                marker.display(),
                release.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let cancellation = crate::lifecycle::CancellationToken::new();
        let context = RuntimeFactoryContext {
            identity: crate::engine::ViewIdentity::new(
                "core:capture",
                crate::workflow::config::ENGINE_CAPTURE,
            ),
            config: ProjectedEngineConfig {
                fields: [(
                    "output".to_string(),
                    serde_json::json!({
                        "producer": "script",
                        "handler": {"file": "capture.sh"}
                    }),
                )]
                .into_iter()
                .collect(),
                workflow_root: Some(root.clone()),
                ..ProjectedEngineConfig::default()
            },
            parameters: crate::workflow::parameter::ParameterSnapshot::from_parts(
                serde_json::Value::Null,
                String::new(),
                crate::input::InputSourceIdentity {
                    frame: crate::input::ViewMountId(1),
                    generation: 0,
                },
                0,
            ),
            cancellation: cancellation.observer(),
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
        runtime.start_prepared_work(&starter);
        runtime.deactivate();
        runtime.start_prepared_work(&starter);

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
        assert!(
            tasks
                .metrics_snapshot()
                .recent_terminal
                .last()
                .unwrap()
                .process_reaped_at
                .is_some()
        );
        tasks.shutdown_and_wait();
        fs::remove_dir_all(root).unwrap();
    }
}
