//! Product identity and process-boundary names.
//!
//! The product name is compiled in once here so a rename never has to touch
//! the XDG directories, the caller environment, the child environment, or the
//! temporary-file prefixes in five different domains. Product-branded names
//! derive from [`PRODUCT`]; workflow-facing names stay generic.

/// Crate and binary name, and the basename of the XDG config, state, cache,
/// and runtime directories.
pub(crate) const PRODUCT: &str = env!("CARGO_PKG_NAME");

// Environment variables the host reads from its caller.
pub(crate) const ENV_SUITE: &str = "TFLOW_SUITE";
pub(crate) const ENV_SETTINGS: &str = "TFLOW_SETTINGS";
pub(crate) const ENV_BIN: &str = "TFLOW_BIN";

// Environment variables the host injects into workflow child processes.
pub(crate) const ENV_WORKFLOW_DIR: &str = "TFLOW_WORKFLOW_DIR";
pub(crate) const ENV_INPUT: &str = "TFLOW_INPUT";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branded_environment_variables_follow_the_product_name() {
        let prefix = PRODUCT.to_ascii_uppercase();
        for name in [
            ENV_SUITE,
            ENV_SETTINGS,
            ENV_BIN,
            ENV_WORKFLOW_DIR,
            ENV_INPUT,
        ] {
            assert!(
                name.starts_with(&prefix),
                "{name} must be prefixed with {prefix}"
            );
        }
    }
}
