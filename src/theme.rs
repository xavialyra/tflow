use anyhow::{Context, Result, bail};
use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolvedTheme {
    pub(crate) base: Style,
    pub(crate) accent: Style,
    pub(crate) muted: Style,
    pub(crate) border: Style,
    pub(crate) surface: Style,
    pub(crate) surface_highlight: Style,
    pub(crate) highlight: Style,
    pub(crate) selected: Style,
    pub(crate) selected_muted: Style,
}

pub(crate) type Theme = ResolvedTheme;

impl Default for ResolvedTheme {
    fn default() -> Self {
        Self::terminal()
    }
}

impl ResolvedTheme {
    pub(crate) fn terminal() -> Self {
        let base = Style::new().fg(Color::Reset).bg(Color::Reset);
        let accent = base.patch(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD));
        let muted = base;
        let border = base;
        let surface = base;
        let highlight = accent;
        let selected = accent;
        let selected_muted = Style {
            fg: muted.fg,
            bg: selected.bg,
            add_modifier: selected.add_modifier,
            sub_modifier: selected.sub_modifier,
            ..Style::new()
        };

        Self {
            base,
            accent,
            muted,
            border,
            surface,
            surface_highlight: highlight,
            highlight,
            selected,
            selected_muted,
        }
    }

    fn apply_patches(&mut self, patches: &ThemeTokenPatches, source: &str) -> Result<()> {
        apply_style_patch(
            &mut self.base,
            patches.base.as_ref(),
            source,
            ThemeToken::Base,
        )?;
        apply_token_patch(
            &mut self.accent,
            patches.base.as_ref(),
            patches.accent.as_ref(),
            source,
            ThemeToken::Accent,
        )?;
        apply_token_patch(
            &mut self.muted,
            patches.base.as_ref(),
            patches.muted.as_ref(),
            source,
            ThemeToken::Muted,
        )?;
        apply_token_patch(
            &mut self.border,
            patches.base.as_ref(),
            patches.border.as_ref(),
            source,
            ThemeToken::Border,
        )?;
        apply_token_patch(
            &mut self.surface,
            patches.base.as_ref(),
            patches.surface.as_ref(),
            source,
            ThemeToken::Surface,
        )?;
        apply_token_patch(
            &mut self.surface_highlight,
            patches.base.as_ref(),
            patches.surface_highlight.as_ref(),
            source,
            ThemeToken::SurfaceHighlight,
        )?;
        apply_token_patch(
            &mut self.highlight,
            patches.base.as_ref(),
            patches.highlight.as_ref(),
            source,
            ThemeToken::Highlight,
        )?;
        apply_token_patch(
            &mut self.selected,
            patches.base.as_ref(),
            patches.selected.as_ref(),
            source,
            ThemeToken::Selected,
        )?;
        apply_token_patch(
            &mut self.selected_muted,
            patches.base.as_ref(),
            patches.selected_muted.as_ref(),
            source,
            ThemeToken::SelectedMuted,
        )?;
        Ok(())
    }

    fn apply_override(&mut self, override_value: &ThemeOverride) -> Result<()> {
        let style = match override_value.token {
            ThemeToken::Base => &mut self.base,
            ThemeToken::Accent => &mut self.accent,
            ThemeToken::Muted => &mut self.muted,
            ThemeToken::Border => &mut self.border,
            ThemeToken::Surface => &mut self.surface,
            ThemeToken::SurfaceHighlight => &mut self.surface_highlight,
            ThemeToken::Highlight => &mut self.highlight,
            ThemeToken::Selected => &mut self.selected,
            ThemeToken::SelectedMuted => &mut self.selected_muted,
        };
        match (&override_value.field, &override_value.value) {
            (StyleField::Foreground, StyleValue::Color(color))
            | (StyleField::Background, StyleValue::Color(color)) => {
                let slot = match override_value.field {
                    StyleField::Foreground => &mut style.fg,
                    StyleField::Background => &mut style.bg,
                    StyleField::Bold => unreachable!(),
                };
                *slot = Some(*color);
            }
            (StyleField::Bold, StyleValue::Bold(enabled)) => {
                *style = set_modifier(*style, Modifier::BOLD, Some(*enabled));
            }
            _ => bail!(
                "theme override {}.{} has an invalid value",
                override_value.token,
                override_value.field
            ),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum ThemeRef {
    Builtin { name: String },
    Named { name: String },
    File { path: PathBuf },
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawTheme {
    #[serde(default)]
    tokens: ThemeTokenPatches,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ThemeTokenPatches {
    #[serde(default)]
    base: Option<ThemeStylePatch>,
    #[serde(default)]
    accent: Option<ThemeStylePatch>,
    #[serde(default)]
    muted: Option<ThemeStylePatch>,
    #[serde(default)]
    border: Option<ThemeStylePatch>,
    #[serde(default)]
    surface: Option<ThemeStylePatch>,
    #[serde(default, rename = "surface-highlight", alias = "surface_highlight")]
    surface_highlight: Option<ThemeStylePatch>,
    #[serde(default)]
    highlight: Option<ThemeStylePatch>,
    #[serde(default)]
    selected: Option<ThemeStylePatch>,
    #[serde(default, rename = "selected-muted", alias = "selected_muted")]
    selected_muted: Option<ThemeStylePatch>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ThemeStylePatch {
    #[serde(default, alias = "fg")]
    foreground: Option<String>,
    #[serde(default, alias = "bg")]
    background: Option<String>,
    #[serde(default)]
    bold: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThemeToken {
    Base,
    Accent,
    Muted,
    Border,
    Surface,
    SurfaceHighlight,
    Highlight,
    Selected,
    SelectedMuted,
}

impl ThemeToken {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "base" => Some(Self::Base),
            "accent" => Some(Self::Accent),
            "muted" => Some(Self::Muted),
            "border" => Some(Self::Border),
            "surface" => Some(Self::Surface),
            "surface-highlight" | "surface_highlight" => Some(Self::SurfaceHighlight),
            "highlight" => Some(Self::Highlight),
            "selected" => Some(Self::Selected),
            "selected-muted" | "selected_muted" => Some(Self::SelectedMuted),
            _ => None,
        }
    }

    fn all_names() -> &'static str {
        "base, accent, muted, border, surface, surface-highlight, highlight, selected, or selected-muted"
    }
}

impl std::fmt::Display for ThemeToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Base => "base",
            Self::Accent => "accent",
            Self::Muted => "muted",
            Self::Border => "border",
            Self::Surface => "surface",
            Self::SurfaceHighlight => "surface-highlight",
            Self::Highlight => "highlight",
            Self::Selected => "selected",
            Self::SelectedMuted => "selected-muted",
        };
        formatter.write_str(name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StyleField {
    Foreground,
    Background,
    Bold,
}

impl StyleField {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "foreground" | "fg" => Some(Self::Foreground),
            "background" | "bg" => Some(Self::Background),
            "bold" => Some(Self::Bold),
            _ => None,
        }
    }
}

impl std::fmt::Display for StyleField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Foreground => "foreground",
            Self::Background => "background",
            Self::Bold => "bold",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StyleValue {
    Color(Color),
    Bold(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThemeOverride {
    token: ThemeToken,
    field: StyleField,
    value: StyleValue,
}

impl FromStr for ThemeOverride {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (key, raw_value) = value
            .split_once('=')
            .ok_or_else(|| format!("theme override {value:?} must use TOKEN.FIELD=VALUE"))?;
        let (raw_token, raw_field) = key.split_once('.').ok_or_else(|| {
            format!(
                "theme override {key:?} must use TOKEN.FIELD=VALUE; expected {}",
                ThemeToken::all_names()
            )
        })?;
        if raw_token.contains('.') {
            return Err(format!(
                "theme override key {key:?} contains too many separators"
            ));
        }
        let token = ThemeToken::parse(raw_token.trim()).ok_or_else(|| {
            format!(
                "theme override token {:?} is unsupported; expected {}",
                raw_token.trim(),
                ThemeToken::all_names()
            )
        })?;
        let field = StyleField::parse(raw_field.trim()).ok_or_else(|| {
            format!(
                "theme override field {:?} is unsupported; expected foreground, background, or bold",
                raw_field.trim()
            )
        })?;
        let raw_value = raw_value.trim();
        if raw_value.is_empty() {
            return Err(format!(
                "theme override {key:?} must have a non-empty value"
            ));
        }
        let value =
            match field {
                StyleField::Foreground | StyleField::Background => StyleValue::Color(
                    parse_color_value(raw_value)
                        .map_err(|error| format!("theme override {key:?} has {error}"))?,
                ),
                StyleField::Bold => StyleValue::Bold(raw_value.parse::<bool>().map_err(|_| {
                    format!("theme override {key:?} must set bold to true or false")
                })?),
            };
        Ok(Self {
            token,
            field,
            value,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ThemeLoadOptions {
    pub(crate) selector: Option<ThemeRef>,
    pub(crate) overrides: Vec<ThemeOverride>,
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
    configured: Option<&ThemeRef>,
    options: &ThemeLoadOptions,
) -> Result<ResolvedTheme> {
    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    let loader = ThemeLoader { config_dir };
    let mut theme = if let Some(selector) = options.selector.as_ref() {
        loader.resolve_reference(selector)?
    } else if let Some(configured) = configured {
        loader.resolve_reference(configured)?
    } else {
        ResolvedTheme::terminal()
    };
    for override_value in &options.overrides {
        theme.apply_override(override_value)?;
    }
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
            ThemeRef::File { path } => {
                let path = if path.is_absolute() {
                    path.clone()
                } else {
                    self.config_dir.join(path)
                };
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
        let mut theme = ResolvedTheme::terminal();
        theme.apply_patches(&raw.tokens, &canonical.display().to_string())?;
        Ok(theme)
    }
}

fn apply_token_patch(
    style: &mut Style,
    base_patch: Option<&ThemeStylePatch>,
    token_patch: Option<&ThemeStylePatch>,
    source: &str,
    token: ThemeToken,
) -> Result<()> {
    let Some(base_patch) = base_patch else {
        return apply_style_patch(style, token_patch, source, token);
    };
    let inherits_foreground = token_patch.is_none_or(|patch| patch.foreground.is_none());
    let inherits_background = token_patch.is_none_or(|patch| patch.background.is_none());
    let inherits_bold = token_patch.is_none_or(|patch| patch.bold.is_none());
    if inherits_foreground && let Some(value) = base_patch.foreground.as_deref() {
        style.fg = Some(parse_color(value, source, token, StyleField::Foreground)?);
    }
    if inherits_background && let Some(value) = base_patch.background.as_deref() {
        style.bg = Some(parse_color(value, source, token, StyleField::Background)?);
    }
    if inherits_bold {
        *style = set_modifier(*style, Modifier::BOLD, base_patch.bold);
    }
    apply_style_patch(style, token_patch, source, token)
}

fn apply_style_patch(
    style: &mut Style,
    patch: Option<&ThemeStylePatch>,
    source: &str,
    token: ThemeToken,
) -> Result<()> {
    let Some(patch) = patch else {
        return Ok(());
    };
    if let Some(value) = patch.foreground.as_deref() {
        style.fg = Some(parse_color(value, source, token, StyleField::Foreground)?);
    }
    if let Some(value) = patch.background.as_deref() {
        style.bg = Some(parse_color(value, source, token, StyleField::Background)?);
    }
    *style = set_modifier(*style, Modifier::BOLD, patch.bold);
    Ok(())
}

fn parse_color(value: &str, source: &str, token: ThemeToken, field: StyleField) -> Result<Color> {
    parse_color_value(value).map_err(|error| {
        anyhow::anyhow!(
            "{source} {token}.{field} has {error}; expected terminal, a basic ANSI color, or #RRGGBB"
        )
    })
}

fn parse_color_value(value: &str) -> Result<Color, String> {
    let value = value.trim();
    let color = match value.to_ascii_lowercase().as_str() {
        "terminal" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" => Some(Color::Gray),
        "white" => Some(Color::White),
        _ => None,
    }
    .or_else(|| parse_hex_color(value));
    color.ok_or_else(|| format!("unsupported color {value:?}"))
}

fn parse_hex_color(value: &str) -> Option<Color> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    Some(Color::Rgb(
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ))
}

fn set_modifier(style: Style, modifier: Modifier, enabled: Option<bool>) -> Style {
    match enabled {
        Some(true) => style.add_modifier(modifier),
        Some(false) => style.remove_modifier(modifier),
        None => style,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temporary_root() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "tui-launcher-theme-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn terminal_theme_contains_final_tokens() {
        let theme = ResolvedTheme::terminal();

        assert_eq!(theme.base.fg, Some(Color::Reset));
        assert_eq!(theme.base.bg, Some(Color::Reset));
        assert_eq!(theme.accent.fg, Some(Color::Cyan));
        assert!(theme.accent.add_modifier.contains(Modifier::BOLD));
        assert_eq!(theme.highlight.fg, Some(Color::Cyan));
        assert_eq!(theme.surface_highlight.fg, Some(Color::Cyan));
        assert_eq!(theme.selected.fg, Some(Color::Cyan));
        assert_eq!(theme.selected_muted.fg, Some(Color::Reset));
    }

    #[test]
    fn token_tables_accept_ansi_and_hex_colors() {
        let raw: RawTheme = toml::from_str(
            r##"
            [tokens.base]
            foreground = "terminal"
            background = "#102030"

            [tokens.accent]
            foreground = "magenta"

            [tokens.muted]
            foreground = "#204060"

            [tokens.highlight]
            foreground = "yellow"
            background = "blue"
            bold = true
            "##,
        )
        .unwrap();
        let mut theme = ResolvedTheme::terminal();
        theme.apply_patches(&raw.tokens, "test theme").unwrap();

        assert_eq!(theme.base.fg, Some(Color::Reset));
        assert_eq!(theme.base.bg, Some(Color::Rgb(16, 32, 48)));
        assert_eq!(theme.accent.fg, Some(Color::Magenta));
        assert_eq!(theme.accent.bg, Some(Color::Rgb(16, 32, 48)));
        assert_eq!(theme.muted.fg, Some(Color::Rgb(32, 64, 96)));
        assert_eq!(theme.muted.bg, Some(Color::Rgb(16, 32, 48)));
        assert_eq!(theme.highlight.fg, Some(Color::Yellow));
        assert_eq!(theme.highlight.bg, Some(Color::Blue));
        assert!(theme.highlight.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn token_values_must_use_tables() {
        assert!(toml::from_str::<RawTheme>("[tokens]\naccent = \"cyan\"\n").is_err());
        assert!(toml::from_str::<RawTheme>("[tokens.accent]\nreverse = true\n").is_err());
        assert!(
            toml::from_str::<RawTheme>("extends = { source = \"builtin\", name = \"terminal\" }\n")
                .is_err()
        );
    }

    #[test]
    fn explicit_refs_and_style_fields_keep_their_names_in_errors() {
        let ref_error =
            toml::from_str::<ThemeRef>("source = \"mystery\"\nname = \"x\"\n").unwrap_err();
        assert!(ref_error.to_string().contains("mystery"));

        let field_error =
            toml::from_str::<RawTheme>("[tokens.accent]\nforegroundd = \"red\"\n").unwrap_err();
        assert!(field_error.to_string().contains("foregroundd"));
    }

    #[test]
    fn removed_color_aliases_are_rejected() {
        for color in [
            "bright-blue",
            "light-blue",
            "dark-gray",
            "grey",
            "silver",
            "42",
            "default",
        ] {
            let raw: RawTheme =
                toml::from_str(&format!("[tokens.accent]\nforeground = {color:?}\n")).unwrap();
            let error = ResolvedTheme::terminal()
                .apply_patches(&raw.tokens, "test theme")
                .unwrap_err();
            assert!(error.to_string().contains("unsupported color"));
        }
    }

    #[test]
    fn configured_file_refs_are_relative_to_the_config_directory() {
        let root = temporary_root();
        fs::write(
            root.join("local.toml"),
            "[tokens.accent]\nforeground = \"blue\"\n",
        )
        .unwrap();
        let reference = ThemeRef::File {
            path: PathBuf::from("./local.toml"),
        };

        let theme = load(
            &root.join("config.toml"),
            Some(&reference),
            &ThemeLoadOptions::default(),
        )
        .unwrap();

        assert_eq!(theme.accent.fg, Some(Color::Blue));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn named_refs_resolve_below_the_config_directory() {
        let root = temporary_root();
        let themes = root.join("themes");
        fs::create_dir_all(&themes).unwrap();
        fs::write(
            themes.join("work.toml"),
            "[tokens.accent]\nforeground = \"yellow\"\n\n[tokens.muted]\nforeground = \"gray\"\n",
        )
        .unwrap();
        let reference = ThemeRef::Named {
            name: "work".to_string(),
        };

        let theme = load(
            &root.join("config.toml"),
            Some(&reference),
            &ThemeLoadOptions::default(),
        )
        .unwrap();

        assert_eq!(theme.accent.fg, Some(Color::Yellow));
        assert_eq!(theme.muted.fg, Some(Color::Gray));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cli_selector_replaces_configured_ref_and_applies_typed_overrides() {
        let configured = ThemeRef::Named {
            name: "missing-root-theme".to_string(),
        };
        let options = ThemeLoadOptions {
            selector: Some(ThemeRef::Builtin {
                name: "terminal".to_string(),
            }),
            overrides: vec![
                "accent.foreground=green".parse().unwrap(),
                "highlight.bold=false".parse().unwrap(),
            ],
        };

        let theme = load(Path::new("config.toml"), Some(&configured), &options).unwrap();

        assert_eq!(theme.accent.fg, Some(Color::Green));
        assert!(!theme.highlight.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn cli_overrides_are_strongly_typed() {
        let error = "accent.foreground=not-a-color"
            .parse::<ThemeOverride>()
            .unwrap_err();
        assert!(error.contains("unsupported color"));
        let error = "highlight.reverse=true"
            .parse::<ThemeOverride>()
            .unwrap_err();
        assert!(error.contains("foreground, background, or bold"));
        let override_value: ThemeOverride = "selected-muted.bg=cyan".parse().unwrap();
        assert_eq!(override_value.token, ThemeToken::SelectedMuted);
        assert_eq!(override_value.field, StyleField::Background);
    }
}
