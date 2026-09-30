use std::path::{Path, PathBuf};

pub(crate) fn resolve(workflow_root: Option<&Path>, path: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path = expand_home(home.as_deref(), path);
    if path.is_absolute() {
        path
    } else if let Some(root) = workflow_root {
        root.join(path)
    } else {
        path
    }
}

fn expand_home(home: Option<&Path>, path: &str) -> PathBuf {
    match path {
        "~" => home
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(path)),
        _ => path
            .strip_prefix("~/")
            .and_then(|suffix| home.map(|home| home.join(suffix)))
            .unwrap_or_else(|| PathBuf::from(path)),
    }
}
