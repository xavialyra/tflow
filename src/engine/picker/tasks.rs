use super::items::{ItemsRequest, ItemsResponse, load_items_for_page};
use crate::config::Config;
use crate::task::{TaskHandle, TaskRuntime};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct PickerItemsScheduler {
    runtime: TaskRuntime,
    lane: String,
}

impl PickerItemsScheduler {
    pub(crate) fn new(runtime: TaskRuntime, view_ref: &str) -> Self {
        Self {
            runtime,
            lane: format!("picker-items:{view_ref}"),
        }
    }

    pub(crate) fn submit_items(
        &self,
        config: &Arc<Config>,
        request: ItemsRequest,
    ) -> TaskHandle<ItemsResponse> {
        let ItemsRequest {
            view,
            generation,
            input,
            binding_raw,
            page_state,
        } = request;
        let query = binding_raw.clone();
        let config = Arc::clone(config);
        self.runtime
            .spawn_latest(self.lane.clone(), move |context| {
                let result = load_items_for_page(
                    &config,
                    &view,
                    &page_state,
                    &binding_raw,
                    &context.runtime,
                    &context.cancellation,
                )
                .map_err(|error| error.to_string());
                Ok(ItemsResponse {
                    view,
                    generation,
                    input,
                    query,
                    result,
                })
            })
    }
}
