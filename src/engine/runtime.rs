use super::api::ViewIdentity;
use crate::input::{EditorSnapshot, Key, ViewMountId};
use crate::terminal::ImagePicker;
use crate::ui::theme::ResolvedTheme;
use crate::workflow::command::ViewOutput;
use crate::workflow::config::CommandBindingVisibility;
use crate::workflow::parameter::ParameterSnapshot;
use anyhow::Result;
use ratatui::{Frame, layout::Rect};
use serde_json::Value;
use std::any::Any;
use std::fmt;
use std::sync::Arc;

/// A configured action identifier. Physical keys are resolved by InputContext
/// before an Engine sees this value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ActionId(String);

impl ActionId {
    pub(crate) fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ActionId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for ActionId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionInvocation {
    pub(crate) id: ActionId,
}

impl ActionInvocation {
    pub(crate) fn new(id: impl Into<ActionId>) -> Self {
        Self { id: id.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionSpec {
    pub(crate) id: ActionId,
}

impl ActionSpec {
    pub(crate) fn unit(id: impl Into<ActionId>) -> Self {
        Self { id: id.into() }
    }
}

#[derive(Clone)]
pub(crate) struct EngineActionInput {
    pub(crate) invocation: ActionInvocation,
    pub(crate) context: ViewContext,
}

/// The immutable runtime projection a mounted Engine is allowed to observe.
///
/// Session owns the complete runtime tree. Engines receive only the current
/// mounted view projection needed to publish their own runtime updates.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EngineRuntimeSnapshot {
    current: Value,
}

impl EngineRuntimeSnapshot {
    pub(crate) fn new(current: Value) -> Self {
        Self { current }
    }

    pub(crate) fn current(&self) -> &Value {
        &self.current
    }
}

impl Default for EngineRuntimeSnapshot {
    fn default() -> Self {
        Self::new(Value::Null)
    }
}

/// The mount-local identity of a committed Session-owned ViewContext.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ViewContextIdentity {
    pub(crate) mount_id: ViewMountId,
    pub(crate) context_revision: u64,
}

impl ViewContextIdentity {
    pub(crate) fn new(mount_id: ViewMountId, context_revision: u64) -> Self {
        Self {
            mount_id,
            context_revision,
        }
    }
}

/// A publication returned by an Engine for the mount-local `current` value.
/// It is deliberately separate from the global RuntimeStore projection.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ViewContextPublication {
    pub(crate) current: Value,
    pub(crate) ready: bool,
}

impl ViewContextPublication {
    pub(crate) fn new(current: Value) -> Self {
        Self {
            current,
            ready: false,
        }
    }

    pub(crate) fn with_ready(mut self, ready: bool) -> Self {
        self.ready = ready;
        self
    }

    pub(crate) fn current(&self) -> &Value {
        &self.current
    }
}

/// The values required to construct a canonical Session-owned ViewContext.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ViewContextParts {
    pub(crate) mount_id: ViewMountId,
    pub(crate) identity: ViewIdentity,
    pub(crate) input: EditorSnapshot,
    pub(crate) input_generation: u64,
    pub(crate) parameters: ParameterSnapshot,
    pub(crate) input_rejected: bool,
    pub(crate) runtime: EngineRuntimeSnapshot,
    pub(crate) current: Value,
    pub(crate) revision: u64,
}

/// The canonical Session-owned snapshot for one mounted View.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ViewContext {
    pub(crate) mount_id: ViewMountId,
    pub(crate) identity: ViewIdentity,
    pub(crate) input: EditorSnapshot,
    pub(crate) input_generation: u64,
    pub(crate) parameters: ParameterSnapshot,
    pub(crate) input_rejected: bool,
    pub(crate) runtime: EngineRuntimeSnapshot,
    pub(crate) current: Value,
    pub(crate) revision: u64,
}

impl ViewContext {
    /// Construct a minimal context for Engine unit tests.
    #[cfg(test)]
    pub(crate) fn for_test(
        identity: ViewIdentity,
        input: EditorSnapshot,
        parameters: ParameterSnapshot,
        input_rejected: bool,
        runtime: EngineRuntimeSnapshot,
    ) -> Self {
        let mount_id = parameters.source().frame;
        let current = runtime.current().clone();
        Self::from_parts(ViewContextParts {
            mount_id,
            identity,
            input: input.clone(),
            input_generation: input.revision,
            parameters,
            input_rejected,
            runtime,
            current,
            revision: 0,
        })
    }

    pub(crate) fn from_parts(parts: ViewContextParts) -> Self {
        Self {
            mount_id: parts.mount_id,
            identity: parts.identity,
            input: parts.input,
            input_generation: parts.input_generation,
            parameters: parts.parameters,
            input_rejected: parts.input_rejected,
            runtime: parts.runtime,
            current: parts.current,
            revision: parts.revision,
        }
    }

    pub(crate) fn identity(&self) -> ViewContextIdentity {
        ViewContextIdentity::new(self.mount_id(), self.revision())
    }

    pub(crate) fn mount_id(&self) -> ViewMountId {
        self.mount_id
    }

    pub(crate) fn view_identity(&self) -> &ViewIdentity {
        &self.identity
    }

    pub(crate) fn view_ref(&self) -> &str {
        &self.identity.view_ref
    }

    pub(crate) fn parameter_snapshot(&self) -> &ParameterSnapshot {
        &self.parameters
    }

    pub(crate) fn parameter_raw(&self) -> &str {
        self.parameters.raw_input()
    }

    pub(crate) fn input_snapshot(&self) -> &EditorSnapshot {
        &self.input
    }

    pub(crate) fn input_raw(&self) -> &str {
        &self.input.raw
    }

    pub(crate) fn input_rejected(&self) -> bool {
        self.input_rejected
    }

    pub(crate) fn runtime_snapshot(&self) -> &EngineRuntimeSnapshot {
        &self.runtime
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
}

#[derive(Clone)]
pub(crate) struct EngineTick {
    pub(crate) context: ViewContext,
    pub(crate) content_size: (u16, u16),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum EngineNotice {
    Info { view_ref: String, message: String },
    Error { view_ref: String, message: String },
    ClearError,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RuntimeUpdate {
    pub(crate) path: String,
    pub(crate) value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EngineNavigationRequest {
    pub(crate) target: String,
    pub(crate) parameters: Option<Value>,
    pub(crate) replace: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum EffectRequest {
    CopyToClipboard(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct QualifiedCommandId {
    pub(crate) owner: String,
    pub(crate) id: String,
}

impl QualifiedCommandId {
    pub(crate) fn new(owner: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            id: id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EngineCommandBinding {
    pub(crate) command: QualifiedCommandId,
    pub(crate) key: Key,
    pub(crate) label: Option<String>,
    pub(crate) enabled: bool,
    pub(crate) visibility: CommandBindingVisibility,
}

impl EngineCommandBinding {
    pub(crate) fn new(command: QualifiedCommandId, key: Key) -> Self {
        Self {
            command,
            key,
            label: None,
            enabled: true,
            visibility: CommandBindingVisibility::Always,
        }
    }
}

#[derive(Clone)]
pub(crate) struct EngineCommandProjection {
    pub(crate) based_on: ViewContextIdentity,
    pub(crate) bindings: Vec<EngineCommandBinding>,
}

impl EngineCommandProjection {
    pub(crate) fn new(based_on: ViewContextIdentity) -> Self {
        Self {
            based_on,
            bindings: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub(crate) enum EngineDecision {
    Continue,
    Invalidate,
    Return(ViewOutput),
    Navigate(EngineNavigationRequest),
    Execute(EffectRequest),
    Report(EngineNotice),
    RuntimeUpdate(RuntimeUpdate),
    Batch(Vec<EngineDecision>),
    Close,
    Exit,
}

impl From<EngineNavigationRequest> for EngineDecision {
    fn from(request: EngineNavigationRequest) -> Self {
        Self::Navigate(request)
    }
}

/// Immutable data handed to a ViewRenderer. The type-erased payload is owned
/// by the model and can only be inspected through a typed downcast.
#[derive(Clone)]
pub(crate) struct RenderModel {
    kind: &'static str,
    payload: Arc<dyn Any + Send + Sync>,
}

impl fmt::Debug for RenderModel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RenderModel")
            .field("kind", &self.kind)
            .finish()
    }
}

impl RenderModel {
    pub(crate) fn new<T>(kind: &'static str, payload: T) -> Self
    where
        T: Any + Send + Sync,
    {
        Self {
            kind,
            payload: Arc::new(payload),
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        self.kind
    }

    pub(crate) fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.payload.downcast_ref()
    }
}

#[derive(Clone)]
pub(crate) struct RenderContext {
    pub(crate) theme: ResolvedTheme,
    pub(crate) image_picker: Option<ImagePicker>,
}

pub(crate) trait ViewRenderer {
    fn validate_model(&self, model: &RenderModel) -> Result<()>;

    fn render(&self, model: &RenderModel, context: &RenderContext, frame: &mut Frame, area: Rect);

    fn chrome(&self, _model: &RenderModel) -> crate::ui::chrome::EngineChrome {
        crate::ui::chrome::EngineChrome::default()
    }
}

/// Raw bytes are a Host-to-runtime capability, not part of the common
/// Engine phase protocol. The receiver is owned by the mounted runtime.
pub(crate) trait RawInputReceiver {
    fn push_input(&mut self, bytes: &[u8]) -> Result<()>;
}

/// Host-visible output from one live Engine transition.
pub(crate) struct EngineEmission {
    decision: EngineDecision,
    publication: Option<ViewContextPublication>,
}

impl EngineEmission {
    pub(crate) fn decision(decision: EngineDecision) -> Self {
        Self {
            decision,
            publication: None,
        }
    }

    pub(crate) fn with_publication(mut self, publication: ViewContextPublication) -> Self {
        self.publication = Some(publication);
        self
    }

    pub(crate) fn publication(&self) -> Option<&ViewContextPublication> {
        self.publication.as_ref()
    }

    pub(crate) fn decision_ref(&self) -> &EngineDecision {
        &self.decision
    }
}

/// Restricted output from an inactive Engine. Background work may update the
/// mount-local publication and report notices, but cannot target Host runtime
/// state or request an active-view effect.
#[derive(Default)]
pub(crate) struct BackgroundOutcome {
    pub(crate) notices: Vec<EngineNotice>,
    pub(crate) publication: Option<ViewContextPublication>,
}

impl BackgroundOutcome {
    pub(crate) fn with_publication(mut self, publication: ViewContextPublication) -> Self {
        self.publication = Some(publication);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ExternalTickAction {
    Continue,
    Return(ViewOutput),
    Close,
    Fail(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExternalTickResult {
    pub(crate) action: ExternalTickAction,
    pub(crate) notice: Option<EngineNotice>,
}

impl ExternalTickResult {
    pub(crate) fn continue_without_notice() -> Self {
        Self {
            action: ExternalTickAction::Continue,
            notice: None,
        }
    }

    pub(crate) fn return_with(output: ViewOutput, notice: Option<EngineNotice>) -> Self {
        Self {
            action: ExternalTickAction::Return(output),
            notice,
        }
    }

    pub(crate) fn close_with(notice: Option<EngineNotice>) -> Self {
        Self {
            action: ExternalTickAction::Close,
            notice,
        }
    }

    pub(crate) fn fail_with(error: String, notice: Option<EngineNotice>) -> Self {
        Self {
            action: ExternalTickAction::Fail(error),
            notice,
        }
    }
}

/// Runtime contract for one mounted Engine. Calls mutate the live runtime;
/// Host preflight and effect application happen independently afterward.
pub(crate) trait EngineRuntime {
    fn action(&mut self, _input: EngineActionInput) -> Result<EngineEmission> {
        anyhow::bail!("engine runtime does not support actions")
    }

    fn activate(&mut self, _context: ViewContext) -> Result<EngineEmission> {
        Ok(EngineEmission::decision(EngineDecision::Continue))
    }

    fn restore_input(&mut self, _context: ViewContext) -> Result<EngineEmission> {
        Ok(EngineEmission::decision(EngineDecision::Continue))
    }

    fn input_committed(&mut self, _context: ViewContext) -> Result<EngineEmission> {
        Ok(EngineEmission::decision(EngineDecision::Continue))
    }

    fn input_ready(&mut self, _context: ViewContext) -> Result<EngineEmission> {
        Ok(EngineEmission::decision(EngineDecision::Continue))
    }

    fn input_rejected(&mut self, _expected: ViewContextIdentity) -> Result<EngineEmission> {
        Ok(EngineEmission::decision(EngineDecision::Continue))
    }

    fn tick(&mut self, _tick: EngineTick) -> Result<EngineEmission> {
        Ok(EngineEmission::decision(EngineDecision::Continue))
    }

    /// Start work that was made ready by the live Engine change. Session
    /// calls this only after publishing the corresponding Host runtime state.
    /// The starter is tied to that committed mount state. An `External`
    /// runtime must keep irreversible resource start in its explicit external
    /// phase instead of using this prepared-work hook.
    fn start_prepared_work(&mut self, _starter: &crate::task::MountTaskStarter) -> bool {
        false
    }

    /// Poll foreground Engine-owned work. A completion is consumed once and
    /// its Engine state is applied before Host processing begins.
    fn poll_work(&mut self) -> Result<Option<EngineEmission>> {
        Ok(None)
    }

    /// Poll inactive Engine-owned work through the restricted background
    /// channel. This cannot express RuntimeUpdate or a Host effect.
    fn poll_background_work(&mut self) -> Result<Option<BackgroundOutcome>> {
        Ok(None)
    }

    /// Drive an explicitly external tick. Session applies the restricted
    /// result directly and does not run generic preflight or rollback after it.
    fn drive_tick(&mut self, _tick: EngineTick) -> Result<ExternalTickResult> {
        anyhow::bail!("engine runtime does not support external ticks")
    }

    /// Confirm an external completion after Session successfully applies the
    /// restricted result. A failed Host transition leaves the completion
    /// pending so the next drive can retry it.
    fn commit_external_tick(&mut self) {}

    /// Drive an inactive external resource. This hook cannot publish an
    /// EngineDecision; the runtime must retain any completion for its next
    /// foreground external tick.
    fn drive_background_tick(&mut self) -> Result<()> {
        Ok(())
    }

    /// Release mount-owned resources. Session invokes this exactly once when
    /// the mount leaves the active stack; cleanup must not fail.
    fn deactivate(&mut self) {}

    fn command_projection(&self, context: &ViewContext) -> Result<EngineCommandProjection> {
        Ok(EngineCommandProjection::new(context.identity()))
    }

    /// Resolve the owner context for an Engine-owned command. The Host supplies
    /// the page owner directly and asks the Engine only for domain-owned owners.
    fn command_owner_context(
        &self,
        _context: &ViewContext,
        _owner: &str,
    ) -> Result<Option<crate::workflow::command::CommandOwnerContext>> {
        Ok(None)
    }

    /// Returns the mount-owned raw receiver, when this runtime supports raw
    /// input. Session discovers this once after the initial Parameters dispatch
    /// and assigns the mount's stable `ReceiverId`; availability and logical
    /// receiver identity must remain stable for the rest of the mount.
    fn raw_receiver(&mut self) -> Option<&mut dyn RawInputReceiver> {
        None
    }

    fn render_model(&self) -> RenderModel;
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FactoryFieldPlan {
    pub(crate) runtime: &'static [&'static str],
    pub(crate) binding: &'static [&'static str],
    pub(crate) binding_defaults: Option<&'static [&'static str]>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct EngineDefinition {
    pub(crate) actions: Vec<ActionSpec>,
    pub(crate) factory_fields: FactoryFieldPlan,
}

impl EngineDefinition {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn with_factory_fields(mut self, factory_fields: FactoryFieldPlan) -> Self {
        self.factory_fields = factory_fields;
        self
    }

    pub(crate) fn with_actions(mut self, actions: impl IntoIterator<Item = ActionSpec>) -> Self {
        self.actions.extend(actions);
        self
    }

    #[cfg(test)]
    pub(crate) fn action(&self, id: &ActionId) -> Option<&ActionSpec> {
        self.actions.iter().find(|spec| spec.id == *id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{EditorBuffer, InputSourceIdentity};
    use crate::workflow::parameter::ParameterSnapshot;

    fn test_context() -> ViewContext {
        let mount_id = ViewMountId(7);
        let source = InputSourceIdentity {
            frame: mount_id,
            generation: 3,
        };
        ViewContext::from_parts(ViewContextParts {
            mount_id,
            identity: ViewIdentity::new("core:default", "picker"),
            input: EditorBuffer::new("needle").snapshot(),
            input_generation: source.generation,
            parameters: ParameterSnapshot::from_parts(
                serde_json::json!({"query": "needle"}),
                "needle".to_string(),
                source,
                11,
            ),
            input_rejected: false,
            runtime: EngineRuntimeSnapshot::new(serde_json::json!({"page": true})),
            current: serde_json::json!({"item": serde_json::Value::Null}),
            revision: 4,
        })
    }

    struct TestRuntime {
        value: String,
    }

    impl EngineRuntime for TestRuntime {
        fn action(&mut self, input: EngineActionInput) -> Result<EngineEmission> {
            self.value = input.invocation.id.as_str().to_string();
            Ok(
                EngineEmission::decision(EngineDecision::Continue).with_publication(
                    ViewContextPublication::new(serde_json::json!({"value": self.value.clone()})),
                ),
            )
        }

        fn render_model(&self) -> RenderModel {
            RenderModel::new("test", ())
        }
    }

    #[test]
    fn runtime_action_mutates_live_state_before_returning() {
        let mut runtime = TestRuntime {
            value: "live".to_string(),
        };

        let emission = runtime
            .action(EngineActionInput {
                invocation: ActionInvocation::new("accepted"),
                context: test_context(),
            })
            .unwrap();

        assert_eq!(runtime.value, "accepted");
        assert!(matches!(emission.decision_ref(), EngineDecision::Continue));
        assert_eq!(
            emission.publication().unwrap().current(),
            &serde_json::json!({"value": "accepted"})
        );
    }

    #[test]
    fn engine_emission_keeps_its_publication() {
        let publication = ViewContextPublication::new(serde_json::json!({"ready": true}));
        let emission = EngineEmission::decision(EngineDecision::Continue)
            .with_publication(publication.clone());

        assert_eq!(emission.publication(), Some(&publication));
    }
}
