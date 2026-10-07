mod registry;
mod snapshot;

pub(crate) use registry::{
    BindingLayer, CommandEntry, CommandRegistry, UnbindRules, command_call_depth, execute_at_depth,
};
pub(crate) use snapshot::ChromeSnapshot;

pub(crate) fn host_action_label(_id: &str) -> Option<&'static str> {
    None
}
