mod api;
mod capture;
mod embedded;
pub(crate) mod form;
mod picker;
mod projection;
mod registry;
mod runtime;

pub(crate) use api::{
    EmbeddedResultConfig, EmbeddedResultFormat, EngineValidationContext,
    InputBindingFactoryContext, ProjectedBindingConfig, ProjectedEngineConfig,
    RendererFactoryContext, RuntimeFactoryContext, ViewIdentity,
};
pub(crate) use capture::{
    CaptureProtocolConfig, create_protocol_view as create_capture_protocol_view,
};
pub(crate) use embedded::{
    EmbeddedProtocolConfig, EmbeddedTerminal, create_protocol_view as create_embedded_protocol_view,
};
pub(crate) use picker::{
    PickerProtocolConfig, PickerViewServices, SlotToken,
    create_protocol_view as create_picker_protocol_view, mount_data as picker_mount_data,
};
pub(crate) use projection::{project_binding_config, project_engine_config};
pub(crate) use registry::{EngineRegistry, require_field, validate_fields};
pub(crate) use runtime::{
    ActionId, ActionInvocation, ActionSpec, BackgroundOutcome, EffectRequest, EngineActionInput,
    EngineDecision, EngineDefinition, EngineEmission, EngineNavigationRequest, EngineNotice,
    EngineRuntime, EngineRuntimeSnapshot, EngineTick, ExternalTickAction, ExternalTickResult,
    FactoryFieldPlan, RawInputReceiver, RenderContext, RenderModel, RuntimeUpdate, ViewContext,
    ViewContextIdentity, ViewContextParts, ViewContextPublication, ViewRenderer,
};
