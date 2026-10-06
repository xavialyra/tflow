mod api;
pub(crate) mod capture;
mod embedded;
pub(crate) mod form;
pub(crate) mod picker;
mod projection;
mod registry;
mod runtime;

pub(crate) use api::{
    CaptureConfig, EmbeddedConfig, EmbeddedResultConfig, EmbeddedResultFormat,
    EngineValidationContext, FormConfig, PickerConfig, ProjectedBindingConfig,
    ProjectedEngineConfig, RendererFactoryContext, RuntimeFactoryContext, TypedEngineConfig,
    ViewIdentity,
};
pub(crate) use capture::{
    CaptureProtocolConfig, create_protocol_view as create_capture_protocol_view,
};
pub(crate) use embedded::{
    EmbeddedProtocolConfig, EmbeddedTerminal, create_protocol_view as create_embedded_protocol_view,
};
pub(crate) use picker::{
    PickerProtocolConfig, PickerViewServices, PrefixBackspace, SlotToken,
    create_protocol_view as create_picker_protocol_view, mount_data as picker_mount_data,
    run_items_script_raw,
};
pub(crate) use projection::{project_binding_config, project_engine_config};
pub(crate) use registry::{EngineRegistry, require_field, validate_fields};
pub(crate) use runtime::{
    ActionId, ActionInvocation, ActionSpec, BackgroundOutcome, EffectRequest, EngineActionInput,
    EngineDecision, EngineDefinition, EngineEmission, EngineNavigationRequest, EngineNotice,
    EngineRuntime, EngineRuntimeSnapshot, EngineTick, ExternalTickAction, ExternalTickResult,
    RawInputReceiver, RenderContext, RenderModel, RuntimeUpdate, ViewContext, ViewContextIdentity,
    ViewContextParts, ViewContextPublication, ViewRenderer,
};

/// Resolves a bare engine action name into its fully-qualified command id and
/// label, so the command index can address an engine action exactly like any
/// workflow command.
pub(crate) fn engine_action(engine_type: &str, name: &str) -> Option<(String, &'static str)> {
    match engine_type {
        crate::workflow::config::ENGINE_PICKER => picker::engine_action(name),
        crate::workflow::config::ENGINE_CAPTURE => capture::engine_action(name),
        crate::workflow::config::ENGINE_FORM => form::engine_action(name),
        crate::workflow::config::ENGINE_EMBEDDED => embedded::engine_action(name),
        _ => None,
    }
}

/// Parses an explicit engine-action address `@engine:<engine>.<action>` and
/// resolves it. A View binding uses this form to target an engine action; a
/// bare (unprefixed) name only ever resolves to a workflow command.
pub(crate) fn engine_action_from_address(address: &str) -> Option<(String, &'static str)> {
    let (engine, action) = address.strip_prefix("@engine:")?.split_once('.')?;
    engine_action(engine, action)
}

/// Resolves a registry id of the form `<engine>.<action>`, with no sigil.
///
/// This is the identity spelling, not an address: it answers "is this command
/// FQID already owned by an engine action?", which is what keeps the command
/// index unique.
pub(crate) fn engine_action_from_id(fqid: &str) -> Option<(String, &'static str)> {
    let (engine, action) = fqid.split_once('.')?;
    engine_action(engine, action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::config::{ENGINE_CAPTURE, ENGINE_EMBEDDED, ENGINE_FORM, ENGINE_PICKER};

    #[test]
    fn engine_action_namespaces_are_engine_specific() {
        assert_eq!(
            engine_action(ENGINE_PICKER, "select_next"),
            Some(("picker.select_next".to_string(), "Select Next"))
        );
        assert_eq!(
            engine_action(ENGINE_CAPTURE, "copy"),
            Some(("capture.copy".to_string(), "Copy"))
        );
        assert!(engine_action(ENGINE_FORM, "focus_next").is_some());
        assert!(engine_action(ENGINE_EMBEDDED, "cancel").is_some());

        // Each engine owns its namespace: a Picker action is not a Form action.
        assert!(engine_action(ENGINE_FORM, "select_next").is_none());
        assert!(engine_action(ENGINE_PICKER, "focus_next").is_none());
        assert!(engine_action(ENGINE_PICKER, "missing").is_none());
        assert!(engine_action("unknown", "select_next").is_none());
    }

    #[test]
    fn explicit_engine_address_resolves_only_with_the_engine_sigil() {
        assert_eq!(
            engine_action_from_address("@engine:picker.select_next"),
            Some(("picker.select_next".to_string(), "Select Next"))
        );
        // A bare name is never an engine address; that is a workflow command.
        assert!(engine_action_from_address("select_next").is_none());
        assert!(engine_action_from_address("@engine:picker").is_none());
        assert!(engine_action_from_address("@engine:picker.missing").is_none());
    }

    /// The identity spelling carries no sigil. It is what validation uses to ask
    /// whether a workflow command FQID is already owned by an engine action.
    #[test]
    fn an_action_id_resolves_without_a_sigil() {
        assert_eq!(
            engine_action_from_id("picker.select_next"),
            Some(("picker.select_next".to_string(), "Select Next"))
        );
        assert_eq!(
            engine_action_from_id("capture.copy"),
            Some(("capture.copy".to_string(), "Copy"))
        );

        // Only real actions of real engines are ids; anything else is a workflow
        // command's own namespace.
        assert!(engine_action_from_id("picker").is_none());
        assert!(engine_action_from_id("picker.missing").is_none());
        assert!(engine_action_from_id("core.page").is_none());
        assert!(engine_action_from_id("form.open").is_none());
        assert!(engine_action_from_id("@engine:picker.exit").is_none());
    }
}
