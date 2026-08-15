use std::path::PathBuf;
use std::process::Command;

pub(crate) const MANAGED_ENVIRONMENT: &[&str] = &[
    "LAUNCHER_COMMAND",
    "LAUNCHER_INPUT",
    "LAUNCHER_ITEM",
    "LAUNCHER_ITEM_PLUGIN",
    "LAUNCHER_ITEM_VIEW_REF",
    "LAUNCHER_LOG_FILE",
    "LAUNCHER_METADATA",
    "LAUNCHER_PLUGIN",
    "LAUNCHER_PLUGIN_DIR",
    "LAUNCHER_QUERY",
    "LAUNCHER_VALUE",
    "LAUNCHER_VIEW",
    "LAUNCHER_VIEW_REF",
];

#[derive(Clone)]
pub(crate) struct PreparedProcess {
    pub(crate) argv: Vec<String>,
    pub(crate) environment: Vec<(String, String)>,
    pub(crate) current_dir: Option<PathBuf>,
}

impl PreparedProcess {
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.argv[0]);
        command.args(&self.argv[1..]);
        for key in MANAGED_ENVIRONMENT {
            command.env_remove(key);
        }
        if let Some(current_dir) = &self.current_dir {
            command.current_dir(current_dir);
        }
        for (key, value) in &self.environment {
            command.env(key, value);
        }
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn command_removes_unset_launcher_environment() {
        let prepared = PreparedProcess {
            argv: vec!["true".to_string()],
            environment: vec![("LAUNCHER_VIEW".to_string(), "core:default".to_string())],
            current_dir: None,
        };

        let command = prepared.command();
        let environment = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            environment["LAUNCHER_VIEW"].as_deref(),
            Some("core:default")
        );
        assert_eq!(environment["LAUNCHER_PLUGIN_DIR"], None);
        assert_eq!(environment["LAUNCHER_LOG_FILE"], None);
    }
}
