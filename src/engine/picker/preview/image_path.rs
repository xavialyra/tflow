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

#[cfg(test)]
mod tests {
    use super::{expand_home, resolve};
    use std::path::Path;

    #[test]
    fn expands_current_user_home_paths() {
        let home = Path::new("/home/launcher");
        assert_eq!(
            expand_home(Some(home), "~/data/cover.png"),
            home.join("data/cover.png")
        );
        assert_eq!(
            expand_home(Some(home), "~other/cover.png"),
            Path::new("~other/cover.png")
        );
    }

    #[test]
    fn resolves_relative_paths_from_workflow_root() {
        assert_eq!(
            resolve(Some(Path::new("/workflows/images")), "cover.png"),
            Path::new("/workflows/images/cover.png")
        );
    }
}
