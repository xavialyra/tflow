use crate::engine::{
    EngineRegistry, PickerProtocolConfig, PrefixBackspace, PreviewDocumentCache,
    create_capture_protocol_view, create_embedded_protocol_view, create_picker_protocol_view,
    picker_mount_data,
};
use crate::input::{InputSourceIdentity, ViewMountId};
use crate::lifecycle::CancellationToken;
use crate::protocol::contracts::ViewInstanceId;
use crate::ui::theme::ResolvedTheme;
use crate::view::{NavigationRequest, View, ViewFactory, ViewServices};
use crate::workflow::config::CompiledConfig;
use crate::workflow::parameter::ParameterSnapshot;
use anyhow::{Context, Result, bail};
use serde_json::Value;

/// Factory used by the default protocol composition root. It owns only
/// immutable configuration and creates a fresh protocol View for each Router
/// instance; no separate AppSession state is consulted.
pub(crate) struct ProtocolViewFactory {
    config: std::sync::Arc<CompiledConfig>,
    invocation: std::sync::Arc<crate::workflow::InvocationContext>,
    theme: ResolvedTheme,
    cancellation: CancellationToken,
    engines: EngineRegistry,
    /// One cache per session so a Picker remounted by navigation or `replace`
    /// reuses the document another instance already rendered.
    preview_cache: PreviewDocumentCache,
}

impl ProtocolViewFactory {
    pub(crate) fn new(
        config: std::sync::Arc<CompiledConfig>,
        invocation: std::sync::Arc<crate::workflow::InvocationContext>,
        theme: ResolvedTheme,
        cancellation: CancellationToken,
        engines: EngineRegistry,
    ) -> Self {
        Self {
            config,
            invocation,
            theme,
            cancellation,
            engines,
            preview_cache: PreviewDocumentCache::default(),
        }
    }

    fn parameters(
        &self,
        request: &NavigationRequest,
        instance: ViewInstanceId,
    ) -> Result<ParameterSnapshot> {
        let raw = if let Some(seed) = &request.input {
            seed.text.clone()
        } else {
            let mut state = self.config.instantiate_parameters(&request.target)?;
            self.config
                .update_sanitized_initial_parameter_values(&mut state, &request.query.values)?;
            self.config.render_parameter_input(&state)?
        };
        Ok(ParameterSnapshot::from_parts(
            request.query.values.clone(),
            raw,
            InputSourceIdentity {
                frame: ViewMountId(instance.0),
                generation: 0,
            },
            0,
        ))
    }

    fn runtime_snapshot(&self, target: &str, parameters: &ParameterSnapshot) -> Result<Value> {
        let raw = parameters.raw_input();
        let commands =
            crate::workflow::command::collect_available_commands(&self.config, target, true)?
                .into_values()
                .collect::<Vec<_>>();
        Ok(serde_json::json!({
            "view": {"current": {
                "ref": target,
                "state_revision": 0,
                "input": raw,
                "raw_input": raw,
                "query": raw,
                "cursor": raw.len(),
                "buffer_revision": 0,
                "selected_item": Value::Null,
                "items": [],
                "command": commands,
                "command_owner": target,
            }},
            "session": {"input": {
                "raw": raw,
                "params": raw,
                "cursor": raw.len(),
                "revision": 0,
            }}
        }))
    }

    fn prepared(
        &self,
        target: &str,
        instance: ViewInstanceId,
        request: &NavigationRequest,
    ) -> Result<(
        crate::engine::ProjectedEngineConfig,
        crate::engine::ProjectedBindingConfig,
        Value,
        ParameterSnapshot,
    )> {
        let definition = self.engines.definition(&self.config, target)?;
        let parameters = self.parameters(request, instance)?;
        let runtime = self.runtime_snapshot(target, &parameters)?;
        Ok((
            crate::engine::project_engine_config(
                &self.config,
                target,
                &definition,
                self.invocation.input_value().clone(),
            )?,
            crate::engine::project_binding_config(&self.config, target, &definition)?,
            runtime,
            parameters,
        ))
    }

    fn picker_services(
        &self,
        target: &str,
        instance: ViewInstanceId,
    ) -> Result<crate::engine::PickerViewServices> {
        let mut services = picker_mount_data(
            &self.config,
            self.invocation.input_value(),
            target,
            crate::task::MountTaskLease::new(crate::input::ViewMountId(instance.0)),
        )?;
        services.set_preview_cache(self.preview_cache.clone());
        Ok(services)
    }
}

impl ViewFactory for ProtocolViewFactory {
    fn create(
        &self,
        request: &NavigationRequest,
        instance: ViewInstanceId,
        services: &ViewServices<'_>,
    ) -> Result<Box<dyn View>> {
        let target = &request.target;
        let engine_type = self.config.engine(target)?.to_string();
        let (engine, bindings, runtime_snapshot, parameters) =
            self.prepared(target, instance, request)?;
        match engine_type.as_str() {
            crate::workflow::config::ENGINE_PICKER => {
                let parameter_binding = self.config.parameter_binding(target)?;
                let left_prefix = match self.config.picker_left_prefix() {
                    None => None,
                    Some("$route") => Some(
                        services
                            .routes
                            .resolve(target)
                            .map(|location| location.label().to_string())
                            .unwrap_or_else(|| target.clone()),
                    ),
                    Some(literal) => Some(literal.to_string()),
                };
                let config = PickerProtocolConfig {
                    identity: crate::engine::ViewIdentity::new(
                        target,
                        crate::workflow::config::ENGINE_PICKER,
                    ),
                    engine,
                    bindings,
                    services: self.picker_services(target, instance)?,
                    parameter_binding,
                    theme: self.theme.clone(),
                    left_prefix,
                    prefix_backspace: self
                        .config
                        .picker_left_prefix_backspace()
                        .map(|value| match value {
                            crate::workflow::config::LeftPrefixBackspace::Parent => {
                                PrefixBackspace::Parent
                            }
                            crate::workflow::config::LeftPrefixBackspace::Root => {
                                PrefixBackspace::Root
                            }
                        }),
                    runtime_snapshot,
                    tasks: services
                        .host
                        .task_runtime()
                        .context("protocol host does not provide a TaskRuntime")?,
                };
                create_picker_protocol_view(config, request, instance)
            }
            crate::workflow::config::ENGINE_FORM => crate::engine::form::create_protocol_view(
                crate::engine::form::FormProtocolConfig {
                    engine,
                    runtime_snapshot,
                    raw_input: parameters.raw_input().to_string(),
                    theme: self.theme.clone(),
                    tasks: services
                        .host
                        .task_runtime()
                        .context("protocol host does not provide a TaskRuntime")?,
                },
                request,
                instance,
            ),
            crate::workflow::config::ENGINE_CAPTURE => create_capture_protocol_view(
                crate::engine::CaptureProtocolConfig::new(
                    target,
                    engine,
                    bindings,
                    self.cancellation.observer(),
                    runtime_snapshot,
                    self.theme.clone(),
                    services
                        .host
                        .task_runtime()
                        .context("protocol host does not provide a TaskRuntime")?,
                ),
                request,
                instance,
            ),
            crate::workflow::config::ENGINE_EMBEDDED => create_embedded_protocol_view(
                crate::engine::EmbeddedProtocolConfig::new(
                    target,
                    engine,
                    bindings,
                    self.cancellation.observer(),
                    runtime_snapshot,
                    parameters,
                    self.theme.clone(),
                ),
                request,
                instance,
            ),
            other => bail!("unsupported protocol View engine {:?}", other),
        }
    }
}
