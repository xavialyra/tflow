mod registry;
mod snapshot;

pub(crate) use registry::{
    BindingLayer, CommandEntry, CommandRegistry, UnbindRules, command_call_depth, execute_at_depth,
};
pub(crate) use snapshot::ChromeSnapshot;

pub(crate) const OPEN_COMPANION: &str = "host.open_companion";

pub(crate) fn host_action_label(id: &str) -> Option<&'static str> {
    (id == OPEN_COMPANION).then_some("Open Companion")
}
