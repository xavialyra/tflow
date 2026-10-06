//! Host-facing protocol modules.

mod command_adapter;
pub(crate) mod contracts;
mod script;
mod session;

pub(crate) use script::{
    FeedbackLevel, ProtocolOperation, ProtocolOutcome, capture_request, command_request,
    form_request, items_request, parse_declared_operation, return_request,
    run_script_capture_response, run_script_form_response, run_script_items_response,
    run_script_response,
};

pub(crate) use command_adapter::ProtocolCommandService;
pub(crate) use session::ProtocolSession;
