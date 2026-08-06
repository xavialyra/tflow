use crate::config::Config;
use crate::discovery::{DiscoveryResult, discover};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

pub(super) struct DiscoveryRequest {
    pub(super) id: u64,
    pub(super) view: String,
    pub(super) input: String,
    pub(super) log_file: Option<PathBuf>,
}

pub(super) struct DiscoveryResponse {
    pub(super) id: u64,
    pub(super) view: String,
    pub(super) input: String,
    pub(super) active_rule: String,
    pub(super) query: String,
    pub(super) result: std::result::Result<DiscoveryResult, String>,
}

pub(crate) struct DiscoveryEvent {
    pub(crate) current: bool,
    pub(crate) view: String,
    pub(crate) errors: Vec<String>,
    pub(crate) failure: Option<String>,
    pub(crate) pending_command: Option<super::super::Key>,
}

pub(super) fn discovery_result_error(
    result: &std::result::Result<DiscoveryResult, String>,
) -> (Vec<String>, Option<String>) {
    match result {
        Ok(result) => (result.errors.clone(), None),
        Err(error) => (Vec::new(), Some(error.clone())),
    }
}

pub(super) fn discovery_worker(
    config: Config,
    requests: Receiver<DiscoveryRequest>,
    responses: Sender<DiscoveryResponse>,
) {
    while let Ok(mut request) = requests.recv() {
        while let Ok(next_request) = requests.try_recv() {
            request = next_request;
        }

        let (source_prefix, query) = config.resolve_view_prefix(&request.view, &request.input);
        let (rule_name, rule, rule_query) = config.resolve_rule(&query);
        let result = discover(
            &config,
            &request.view,
            rule_name,
            rule,
            &rule_query,
            source_prefix.as_deref(),
            request.log_file.as_deref(),
        )
        .map_err(|error| error.to_string());
        let response = DiscoveryResponse {
            id: request.id,
            view: request.view,
            input: request.input,
            active_rule: rule_name.to_string(),
            query: rule_query,
            result,
        };
        if responses.send(response).is_err() {
            break;
        }
    }
}
