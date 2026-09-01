mod api;
mod capture;
mod embedded;
mod evaluate;
mod picker;
mod registry;
mod runtime;

#[allow(unused_imports)]
pub(crate) use crate::command::{
    CommandContext, CommandExecution, CommandInvocation, CommandOwnerContext, InputActionBinding,
    InputEdit, InputFocus, InputRefreshPolicy, ViewEffect, ViewOutput, ViewOutputItem, ViewReturn,
};
#[cfg(test)]
pub(crate) use api::EngineRegistration;
pub(crate) use api::{
    EmbeddedResultConfig, EmbeddedResultFormat, EngineValidationContext, EvaluatedBindingConfig,
    EvaluatedEngineConfig, InputBindingFactoryContext, MountRuntimeData, RendererFactoryContext,
    RendererIdentity, RuntimeFactoryContext, ViewFactory, ViewIdentity,
};
pub(crate) use embedded::EmbeddedTerminal;
pub(crate) use evaluate::{
    binding_config as evaluate_binding_config, engine_config as evaluate_engine_config,
    field as evaluate_field, optional_string as evaluate_optional_string,
};
pub(crate) use registry::{EngineRegistry, require_field, validate_fields};
#[allow(unused_imports)]
pub(crate) use runtime::{
    ActionId, ActionInvocation, ActionSpec, BackgroundEngineTick, BackgroundOutcome, EffectRequest,
    EngineActionInput, EngineBufferTarget, EngineCommandBinding, EngineCommandInvocation,
    EngineCommandProjection, EngineDecision, EngineDefinition, EngineEmission,
    EngineNavigationRequest, EngineNotice, EngineRuntime, EngineRuntimeSnapshot, EngineTick,
    EngineTickMode, ExternalTickAction, ExternalTickResult, FactoryFieldPlan, InputPolicy,
    MountPolicy, QualifiedCommandId, RawInputReceiver, RenderContext, RenderModel, RuntimeUpdate,
    TerminalEofPolicy, ViewContext, ViewContextIdentity, ViewContextParts, ViewContextPublication,
    ViewRenderer,
};
