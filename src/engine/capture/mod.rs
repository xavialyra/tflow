mod bindings;
pub(crate) mod document;
pub(crate) mod image_decode;
pub(crate) mod image_path;
pub(crate) mod image_protocol;
mod protocol;
mod render;
mod session;

pub(crate) use protocol::{CaptureProtocolConfig, create_protocol_view};

use self::bindings::{CaptureAction, CaptureBindings};
pub(crate) use self::render::CaptureRenderer;
pub(crate) use self::session::CaptureSession;
use super::{
    BackgroundOutcome, EngineActionInput, EngineDecision, EngineEmission, EngineNotice,
    EngineRuntime, EngineValidationContext, ProjectedEngineConfig, RenderModel,
    RendererFactoryContext, RuntimeFactoryContext, ViewContextPublication, require_field,
    validate_fields,
};
use crate::workflow::config::{
    Defaults, ResolvedScriptSource, View, parse_script_source, toml_to_json,
};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn definition() -> crate::engine::EngineDefinition {
    crate::engine::EngineDefinition::new()
        .with_binding_defaults(Some(&["capture", "bindings"]))
        .with_actions([
            crate::engine::ActionSpec::unit("capture.copy"),
            crate::engine::ActionSpec::unit("capture.back"),
            crate::engine::ActionSpec::unit("capture.scroll_up"),
            crate::engine::ActionSpec::unit("capture.scroll_down"),
            crate::engine::ActionSpec::unit("capture.page_up"),
            crate::engine::ActionSpec::unit("capture.page_down"),
        ])
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
    if let toml::Value::Table(fields) = output {
        if fields.contains_key("file") || fields.contains_key("script") {
            parse_script_source(output, context.script_root)
                .context("capture output script is invalid")?;
        } else if fields.contains_key("content") || fields.contains_key("output") {
            validate_declared_content(output)?;
        } else {
            bail!(
                "view {:?} capture output table must define file, script, or content",
                name
            );
        }
    } else if !output.is_str() {
        bail!(
            "view {:?} capture output must be a string or script object",
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
    CaptureBindings::validate_defaults(bindings.as_ref()).context("capture bindings")
}

pub(super) fn validate_bindings(_name: &str, _view: &View) -> Result<()> {
    Ok(())
}

/// Resolves a bare Capture action name into `("capture.<action>", label)`.
pub(crate) fn engine_action(name: &str) -> Option<(String, &'static str)> {
    crate::input::bindings::binding_action_spec::<self::bindings::CaptureAction>(name)
}

fn validate_declared_content(value: &toml::Value) -> Result<()> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StaticContent {
        content: Option<toml::Value>,
        output: Option<toml::Value>,
    }
    let parsed: StaticContent = value
        .clone()
        .try_into()
        .context("declared capture output table contains unknown fields")?;
    if let Some(content) = parsed.content.or(parsed.output) {
        if content.is_str() {
            return Ok(());
        }
        if let Ok(json_val) = toml_to_json(&content)
            && document::parse(json_val).is_ok()
        {
            return Ok(());
        }
    }
    bail!("declared capture output table must define content or output as a string or document");
}

#[derive(Clone)]
struct PendingCaptureScript {
    view_ref: String,
    root: Option<PathBuf>,
    source: ResolvedScriptSource,
    parameters: serde_json::Value,
    launch_input: serde_json::Value,
    companion_data: Option<crate::view::companion::CompanionData>,
}

enum PreparedCaptureOutput {
    Text(String),
    Document(self::document::Document),
    Script {
        root: Option<PathBuf>,
        source: ResolvedScriptSource,
    },
}

pub(super) fn create_view(
    context: RuntimeFactoryContext,
) -> Result<Box<dyn crate::engine::EngineRuntime>> {
    let prepared = prepare_output(&context.config, context.config.workflow_root.as_deref());
    let workflow_root = context.config.workflow_root.clone();
    let (session, status, success, pending_script, initial_doc) = match prepared {
        Ok(PreparedCaptureOutput::Text(output)) => (
            CaptureSession::from_text(&output),
            String::new(),
            true,
            None,
            None,
        ),
        Ok(PreparedCaptureOutput::Document(doc)) => {
            let doc_clone = doc.clone();
            (
                CaptureSession::from_document(doc),
                String::new(),
                true,
                None,
                Some(doc_clone),
            )
        }
        Ok(PreparedCaptureOutput::Script { root, source }) => {
            let view_ref = context.identity.view_ref.clone();
            let launch_input = if !context.parameters.raw_input().is_empty() {
                Value::String(context.parameters.raw_input().to_string())
            } else {
                context.config.launch_input.clone()
            };
            (
                CaptureSession::from_text(""),
                "starting".to_string(),
                false,
                Some(PendingCaptureScript {
                    view_ref,
                    root,
                    source,
                    parameters: context.parameters.values().clone(),
                    launch_input,
                    companion_data: None,
                }),
                None,
            )
        }
        Err(error) => (
            CaptureSession::from_text(&error.to_string()),
            "failed".to_string(),
            false,
            None,
            None,
        ),
    };
    let mut view = CaptureView {
        view_ref: context.identity.view_ref,
        session,
        status,
        success,
        reported: false,
        pending_script,
        script_task: None,
        script_completion: None,
        workflow_root,
    };
    if let Some(doc) = initial_doc {
        view.start_document_images(&doc);
    }
    Ok(Box::new(view))
}

pub(super) fn create_renderer(
    _context: RendererFactoryContext,
) -> Result<Box<dyn crate::engine::ViewRenderer>> {
    Ok(Box::new(CaptureRenderer::new()))
}

fn prepare_output(
    config: &ProjectedEngineConfig,
    workflow_root: Option<&Path>,
) -> Result<PreparedCaptureOutput> {
    let capture = config
        .as_capture()
        .context("capture engine requires capture config")?;
    if let Some(text) = capture.static_text() {
        return Ok(PreparedCaptureOutput::Text(text.to_string()));
    }
    if let Some(fields) = capture.output.as_table() {
        if fields.contains_key("file") || fields.contains_key("script") {
            let source = parse_script_source(&capture.output, workflow_root)?;
            return Ok(PreparedCaptureOutput::Script {
                root: workflow_root.map(Path::to_path_buf),
                source,
            });
        }
        if let Some(val) = fields.get("content").or_else(|| fields.get("output")) {
            let json_val = toml_to_json(val)?;
            if let Some(doc) = document::parse(json_val)? {
                return Ok(PreparedCaptureOutput::Document(doc));
            }
        }
    }
    bail!("capture output must be a string, document, or script object")
}

struct CaptureScriptOutcome {
    result: Result<Value>,
    managed_child_reaped: bool,
}

fn capture_script_request(plan: &PendingCaptureScript) -> Value {
    let (parameters, input, state) = match &plan.companion_data {
        Some(data) => (&data.parameters, &data.input, &data.engine_state),
        None => (&plan.parameters, &plan.launch_input, &Value::Null),
    };
    crate::protocol::capture_request(
        parameters,
        input,
        crate::workflow::config::ENGINE_CAPTURE,
        state,
    )
}

fn run_capture_script(
    plan: &PendingCaptureScript,
    cancellation: &crate::lifecycle::CancellationObserver,
) -> CaptureScriptOutcome {
    let request = capture_script_request(plan);
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
    Completed(Value),
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
    script_task: Option<crate::task::TaskHandle<Value>>,
    script_completion: Option<CaptureCompletion>,
    workflow_root: Option<PathBuf>,
}

#[derive(Clone)]
pub(super) enum CaptureRenderContent {
    Text(std::sync::Arc<[ratatui::text::Line<'static>]>),
    Document {
        document: self::document::Document,
        images: Vec<self::document::DocumentImageState>,
    },
}

#[derive(Clone)]
struct CaptureRenderModel {
    content: CaptureRenderContent,
    #[cfg_attr(not(test), allow(dead_code))]
    lines: std::sync::Arc<[ratatui::text::Line<'static>]>,
    scroll_offset: usize,
    total_height: usize,
    status: String,
}

impl CaptureView {
    fn start_document_images(&mut self, doc: &document::Document) {
        let mut paths = Vec::new();
        doc.images(&mut paths);
        self.session.set_images(
            paths
                .iter()
                .map(|path| document::DocumentImageState {
                    image: Some(std::sync::Arc::new(image_protocol::ImageSource::file(
                        image_path::resolve(self.workflow_root.as_deref(), path),
                    ))),
                    error: None,
                })
                .collect(),
        );
    }

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
        let (output_val, status, success) = match completion {
            CaptureCompletion::Completed(val) => (val, String::new(), true),
            CaptureCompletion::Failed(error) => (Value::String(error), "failed".to_string(), false),
            CaptureCompletion::Cancelled => (
                Value::String("capture script was cancelled".to_string()),
                "failed".to_string(),
                false,
            ),
        };
        if success {
            if let Some(text) = output_val.as_str() {
                self.session = CaptureSession::from_text(text);
            } else if let Ok(Some(doc)) = document::parse(output_val.clone()) {
                self.session = CaptureSession::from_document(doc.clone());
                self.start_document_images(&doc);
            } else {
                self.session = CaptureSession::from_text(&output_val.to_string());
            }
        } else {
            self.session = CaptureSession::from_text(output_val.as_str().unwrap_or("failed"));
        }
        self.status = status.clone();
        self.success = success;
        self.reported = true;
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
            "capture.scroll_up" => {
                self.session.scroll_up(1);
                EngineDecision::Invalidate
            }
            "capture.scroll_down" => {
                self.session.scroll_down(1);
                EngineDecision::Invalidate
            }
            "capture.page_up" => {
                let step = self.session.viewport_height().saturating_sub(1).max(1);
                self.session.scroll_up(step);
                EngineDecision::Invalidate
            }
            "capture.page_down" => {
                let step = self.session.viewport_height().saturating_sub(1).max(1);
                self.session.scroll_down(step);
                EngineDecision::Invalidate
            }
            action => bail!("unknown capture action {:?}", action),
        };
        Ok(EngineEmission::decision(decision))
    }

    fn tick(&mut self, tick: crate::engine::EngineTick) -> Result<EngineEmission> {
        if tick.content_size.0 > 0 && tick.content_size.1 > 0 {
            self.session
                .set_viewport_size(tick.content_size.0 as usize, tick.content_size.1 as usize);
        } else if tick.content_size.1 > 0 {
            self.session
                .set_viewport_height(tick.content_size.1 as usize);
        }

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

    fn update_companion_data(&mut self, data: &crate::view::companion::CompanionData) {
        if let Some(ref mut plan) = self.pending_script {
            plan.companion_data = Some(data.clone());
        }
        self.script_task = None;
        self.script_completion = None;
        self.reported = false;
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
        let content = match self.session.body() {
            session::CaptureBody::Text { lines } => {
                CaptureRenderContent::Text(std::sync::Arc::clone(lines))
            }
            session::CaptureBody::Document { document, images } => CaptureRenderContent::Document {
                document: document.clone(),
                images: images.clone(),
            },
        };
        RenderModel::new(
            "capture",
            CaptureRenderModel {
                content,
                lines: self.session.shared_lines(),
                scroll_offset: self.session.scroll_offset(),
                total_height: self.session.total_height(),
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

    fn request_plan(input: Value) -> PendingCaptureScript {
        PendingCaptureScript {
            view_ref: "test:capture".into(),
            root: None,
            source: crate::workflow::config::parse_script_source(
                &toml::toml! { script = "printf '{}'" }.into(),
                None,
            )
            .unwrap(),
            parameters: serde_json::json!({"target": "original"}),
            launch_input: input,
            companion_data: None,
        }
    }

    #[test]
    fn ordinary_capture_preserves_json_strings_and_reserved_looking_fields() {
        let object =
            serde_json::json!({"parameters": {"business": true}, "text": "hello", "value": 3});
        for input in [object.clone(), Value::String(object.to_string())] {
            let plan = request_plan(input.clone());
            let request = capture_script_request(&plan);
            assert_eq!(request["context"]["input"], input);
            assert_eq!(request["context"]["parameters"], plan.parameters);
            assert_eq!(request["context"]["engine"]["state"], Value::Null);
        }
    }

    #[test]
    fn companion_capture_uses_explicit_source_without_removing_business_fields() {
        let mut plan = request_plan(Value::Null);
        let data = crate::view::companion::CompanionData {
            parameters: serde_json::json!({"source": true}),
            input: serde_json::json!({"parameters": "business data", "text": "not an item"}),
            engine_state: serde_json::json!({"values": {"name": "demo"}}),
        };
        plan.companion_data = Some(data.clone());
        let request = capture_script_request(&plan);
        assert_eq!(request["context"]["input"], data.input);
        assert_eq!(request["context"]["parameters"], data.parameters);
        assert_eq!(request["context"]["engine"]["state"], data.engine_state);
    }

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
                typed: Some(crate::engine::TypedEngineConfig::Capture(
                    crate::engine::CaptureConfig {
                        output: toml::toml! { file = "capture.sh" }.into(),
                    },
                )),
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
        assert_eq!(
            model.lines.as_ref(),
            &[ratatui::text::Line::raw("captured")]
        );
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
