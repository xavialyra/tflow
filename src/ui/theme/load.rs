use super::model::{RawTheme, ResolvedTheme, ThemeRef};
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub(crate) struct ThemeLoadOptions {
    pub(crate) selector: Option<ThemeRef>,
}

pub(crate) fn cli_named_theme(name: String) -> ThemeRef {
    if name.eq_ignore_ascii_case("terminal") {
        ThemeRef::Builtin { name }
    } else {
        ThemeRef::Named { name }
    }
}

pub(crate) fn load(
    config_path: &Path,
    configured: Option<&str>,
    options: &ThemeLoadOptions,
) -> Result<ResolvedTheme> {
    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    let loader = ThemeLoader { config_dir };
    let theme = if let Some(selector) = options.selector.as_ref() {
        loader.resolve_reference(selector)?
    } else if let Some(configured) = configured {
        loader.resolve_reference(&ThemeRef::Named {
            name: configured.to_string(),
        })?
    } else {
        ResolvedTheme::terminal()
    };
    Ok(theme)
}

struct ThemeLoader<'a> {
    config_dir: &'a Path,
}

impl ThemeLoader<'_> {
    fn resolve_reference(&self, reference: &ThemeRef) -> Result<ResolvedTheme> {
        match reference {
            ThemeRef::Builtin { name } => {
                if name.eq_ignore_ascii_case("terminal") {
                    Ok(ResolvedTheme::terminal())
                } else {
                    bail!("unknown builtin theme {:?}; expected terminal", name)
                }
            }
            ThemeRef::Named { name } => {
                let path = self.config_dir.join("themes").join(format!("{name}.toml"));
                self.load_file(&path)
            }
        }
    }

    fn load_file(&self, path: &Path) -> Result<ResolvedTheme> {
        let canonical = fs::canonicalize(path)
            .with_context(|| format!("could not resolve theme {}", path.display()))?;
        let source = fs::read_to_string(&canonical)
            .with_context(|| format!("could not read theme {}", canonical.display()))?;
        let raw: RawTheme = toml::from_str(&source)
            .with_context(|| format!("could not parse theme {}", canonical.display()))?;
        ResolvedTheme::from_raw(&raw, &canonical.display().to_string())
    }
}
