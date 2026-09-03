mod api;
mod capture;
mod embedded;
mod evaluate;
mod picker;
mod protocol_factory;
mod registry;
mod runtime;

pub(crate) use crate::command::ViewOutput;
pub(crate) use api::{
    EmbeddedResultConfig, EmbeddedResultFormat, EngineValidationContext, EvaluatedBindingConfig,
    EvaluatedEngineConfig, InputBindingFactoryContext, RendererFactoryContext,
    RuntimeFactoryContext, ViewIdentity,
};
pub(crate) use capture::{
    CaptureProtocolConfig, create_protocol_view as create_capture_protocol_view,
};
pub(crate) use embedded::{
    EmbeddedProtocolConfig, EmbeddedTerminal, create_protocol_view as create_embedded_protocol_view,
};
pub(crate) use evaluate::{
    binding_config as evaluate_binding_config, engine_config as evaluate_engine_config,
    field as evaluate_field, optional_string as evaluate_optional_string,
};
pub(crate) use picker::{
    PickerProtocolConfig, PickerViewServices, create_protocol_view as create_picker_protocol_view,
};
pub(crate) use protocol_factory::ProtocolViewFactory;
pub(crate) use registry::{EngineRegistry, require_field, validate_fields};
pub(crate) use runtime::{
    ActionId, ActionInvocation, ActionSpec, BackgroundOutcome, EffectRequest,
    EngineActionInput, EngineCommandBinding,
    EngineCommandProjection, EngineDecision, EngineDefinition, EngineEmission,
    EngineNavigationRequest, EngineNotice, EngineRuntime, EngineRuntimeSnapshot, EngineTick,
    ExternalTickAction, ExternalTickResult, FactoryFieldPlan,
    QualifiedCommandId, RawInputReceiver, RenderContext, RenderModel, RuntimeUpdate,
    ViewContext, ViewContextIdentity, ViewContextParts, ViewContextPublication,
    ViewRenderer,
};
