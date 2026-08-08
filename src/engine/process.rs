use std::path::PathBuf;
use std::process::Command;

pub(crate) struct PreparedProcess {
    pub(crate) argv: Vec<String>,
    pub(crate) environment: Vec<(String, String)>,
    pub(crate) current_dir: Option<PathBuf>,
}

impl PreparedProcess {
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.argv[0]);
        command.args(&self.argv[1..]);
        if let Some(current_dir) = &self.current_dir {
            command.current_dir(current_dir);
        }
        for (key, value) in &self.environment {
            command.env(key, value);
        }
        command
    }
}
