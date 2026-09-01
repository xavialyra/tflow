use crate::command::InputActionBinding;
use crate::config::{Config, View};
use crate::lifecycle::CancellationObserver;
use anyhow::Result;
use ratatui::{Frame, layout::Rect};
use serde_json::Value;
use std::any::Any;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbeddedResultFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EmbeddedResultConfig {
    pub(crate) format: EmbeddedResultFormat,
    pub(crate) required: bool,
    pub(crate) max_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ViewIdentity {
    pub(crate) view_ref: String,
    pub(crate) engine_type: String,
}

impl ViewIdentity {
    pub(crate) fn new(view_ref: impl Into<String>, engine_type: impl Into<String>) -> Self {
        Self {
            view_ref: view_ref.into(),
            engine_type: engine_type.into(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct EvaluatedEngineConfig {
    pub(crate) fields: BTreeMap<String, Value>,
    pub(crate) field_errors: BTreeMap<String, String>,
    pub(crate) plugin_root: Option<PathBuf>,
}

impl EvaluatedEngineConfig {
    pub(crate) fn field(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }

    pub(crate) fn field_error(&self, name: &str) -> Option<&str> {
        self.field_errors.get(name).map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RendererIdentity {
    pub(crate) view_ref: String,
    pub(crate) engine_type: String,
    pub(crate) renderer_type: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct EvaluatedBindingConfig {
    pub(crate) defaults: Option<Value>,
    pub(crate) view_keymap: Option<Value>,
    pub(crate) engine_fields: BTreeMap<String, Value>,
}

impl EvaluatedBindingConfig {
    pub(crate) fn engine_field(&self, name: &str) -> Option<&Value> {
        self.engine_fields.get(name)
    }
}

/// Engine-specific data prepared for one mount and consumed by that Engine's
/// runtime factory. The inert task lease used to build it cannot start work.

#[derive(Clone)]
pub(crate) struct MountRuntimeData(Arc<dyn Any + Send + Sync>);

impl MountRuntimeData {
    pub(crate) fn new<T: Any + Send + Sync>(value: T) -> Self {
        Self(Arc::new(value))
    }

    pub(crate) fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}

pub(crate) struct RuntimeFactoryContext {
    pub(crate) identity: ViewIdentity,
    pub(crate) config: EvaluatedEngineConfig,
    pub(crate) parameters: crate::parameter::ParameterSnapshot,
    pub(crate) cancellation: CancellationObserver,
    pub(crate) data: Option<MountRuntimeData>,
}

pub(crate) struct RendererFactoryContext {
    pub(crate) identity: RendererIdentity,
}

pub(crate) struct InputBindingFactoryContext {
    pub(crate) identity: ViewIdentity,
    pub(crate) bindings: EvaluatedBindingConfig,
}

struct NullRenderer {
    expected_kind: String,
}

impl crate::engine::ViewRenderer for NullRenderer {
    fn validate_model(&self, model: &crate::engine::RenderModel) -> anyhow::Result<()> {
        if model.kind() != self.expected_kind {
            anyhow::bail!(
                "null renderer/model pairing mismatch: expected {:?}, got {:?}",
                self.expected_kind,
                model.kind()
            );
        }
        Ok(())
    }

    fn render(
        &self,
        _model: &crate::engine::RenderModel,
        _context: &crate::engine::RenderContext,
        _frame: &mut Frame,
        _area: Rect,
    ) {
    }
}

pub(crate) trait ViewFactory {
    fn definition(
        &self,
        config: &Config,
        view_ref: &str,
    ) -> Result<crate::engine::EngineDefinition>;

    fn create_mount_data(
        &self,
        _config: &Config,
        _identity: &ViewIdentity,
        _task_lease: crate::task::MountTaskLease,
    ) -> Result<Option<MountRuntimeData>> {
        Ok(None)
    }

    fn create_view(
        &self,
        context: RuntimeFactoryContext,
    ) -> Result<Box<dyn crate::engine::EngineRuntime>>;

    fn create_renderer(
        &self,
        context: RendererFactoryContext,
    ) -> Result<Box<dyn crate::engine::ViewRenderer>> {
        Ok(Box::new(NullRenderer {
            expected_kind: context.identity.renderer_type,
        }))
    }

    fn create_input_bindings(
        &self,
        _context: InputBindingFactoryContext,
    ) -> Result<Vec<InputActionBinding>> {
        Ok(Vec::new())
    }
}

pub(crate) struct EngineValidationContext<'a> {
    pub(crate) view_ref: &'a str,
    pub(crate) view: &'a View,
    pub(crate) script_root: Option<&'a Path>,
}

#[cfg(test)]
pub(crate) type RuntimeFactory =
    Box<dyn Fn(RuntimeFactoryContext) -> Result<Box<dyn crate::engine::EngineRuntime>> + 'static>;
#[cfg(test)]
pub(crate) type RendererFactory =
    Box<dyn Fn(RendererFactoryContext) -> Result<Box<dyn crate::engine::ViewRenderer>> + 'static>;
#[cfg(test)]
pub(crate) type InputBindingFactory =
    Box<dyn Fn(InputBindingFactoryContext) -> Result<Vec<InputActionBinding>> + 'static>;

#[cfg(test)]
pub(crate) struct EngineRegistration {
    pub(crate) definition: crate::engine::EngineDefinition,
    pub(crate) create_runtime: RuntimeFactory,
    pub(crate) create_renderer: RendererFactory,
    pub(crate) create_bindings: InputBindingFactory,
}

#[cfg(test)]
impl EngineRegistration {
    pub(crate) fn new<F>(definition: crate::engine::EngineDefinition, create_runtime: F) -> Self
    where
        F: Fn(RuntimeFactoryContext) -> Result<Box<dyn crate::engine::EngineRuntime>> + 'static,
    {
        Self {
            definition,
            create_runtime: Box::new(create_runtime),
            create_renderer: Box::new(|context| {
                Ok(Box::new(NullRenderer {
                    expected_kind: context.identity.renderer_type,
                }))
            }),
            create_bindings: Box::new(|_| Ok(Vec::new())),
        }
    }

    pub(crate) fn with_renderer_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn(RendererFactoryContext) -> Result<Box<dyn crate::engine::ViewRenderer>> + 'static,
    {
        self.create_renderer = Box::new(factory);
        self
    }

    pub(crate) fn with_input_binding_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn(InputBindingFactoryContext) -> Result<Vec<InputActionBinding>> + 'static,
    {
        self.create_bindings = Box::new(factory);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_aliases_compile_against_the_narrow_contract() {
        let _runtime: RuntimeFactory = Box::new(|_context| anyhow::bail!("signature probe"));
        let _renderer: RendererFactory = Box::new(|context| {
            Ok(Box::new(NullRenderer {
                expected_kind: context.identity.renderer_type,
            }))
        });
        let _bindings: InputBindingFactory = Box::new(|_context| Ok(Vec::new()));

        fn accepts_view_factory(_: &dyn ViewFactory) {}
        let registry = crate::engine::EngineRegistry::new();
        accepts_view_factory(&registry);
    }
}
