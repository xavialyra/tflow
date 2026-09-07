//! Host-facing protocol modules.

mod command_adapter;
pub(crate) mod contracts;
mod session;

pub(crate) use command_adapter::{ProtocolCommandService, ViewCommandBindings};
pub(crate) use session::ProtocolSession;
