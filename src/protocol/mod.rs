//! Host-facing protocol modules.

mod command_adapter;
pub(crate) mod contracts;
mod producer;
mod session;

pub(crate) use producer::{
    ProtocolOperation, capture_request, command_request, items_request, parse_declared_operation,
    return_request, run_script_capture_response, run_script_items_response, run_script_response,
};

pub(crate) use command_adapter::{ProtocolCommandService, ViewCommandBindings};
pub(crate) use session::ProtocolSession;
