use anyhow::{Context, Result, bail};
use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolvedTheme {
    pub(crate) text: Style,
    pub(crate) muted_text: Style,
    pub(crate) chrome: ChromeTheme,
    pub(crate) picker: PickerTheme,
    pub(crate) preview: PreviewTheme,
    pub(crate) capture: CaptureTheme,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ChromeTheme {
    pub(crate) divider: Style,
    pub(crate) input_prefix: Style,
    pub(crate) footer: Style,
    pub(crate) footer_key: Style,
    pub(crate) error: Style,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PickerTheme {
    pub(crate) text: Style,
    pub(crate) muted: Style,
    pub(crate) selected: Style,
    pub(crate) selected_muted: Style,
    pub(crate) marker: Style,
    pub(crate) scrollbar: Style,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PreviewTheme {
    pub(crate) text: Style,
    pub(crate) error: Style,
    pub(crate) border: Style,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CaptureTheme {
    pub(crate) text: Style,
}

pub(crate) type Theme = ResolvedTheme;

impl Default for ResolvedTheme {
    fn default() -> Self {
        Self::terminal()
    }
}

impl ResolvedTheme {
    pub(crate) fn terminal() -> Self {
        Self::from_raw(&RawTheme::default(), "builtin terminal")
            .expect("builtin terminal theme must be valid")
    }

    fn from_raw(raw: &RawTheme, source: &str) -> Result<Self> {
        let palette = resolve_palette(&raw.palette, source)?;
        validate_bindings(&raw.bindings, source)?;
        let scheme = ResolvedScheme::resolve(&palette, &raw.scheme, source)?;
        let binding = |binding| resolve_binding(binding, &raw.bindings, &scheme, source);

        Ok(Self {
            text: binding(ThemeBinding::Text)?,
            muted_text: binding(ThemeBinding::MutedText)?,
            chrome: ChromeTheme {
                divider: binding(ThemeBinding::ChromeDivider)?,
                input_prefix: binding(ThemeBinding::ChromeInputPrefix)?,
                footer: binding(ThemeBinding::ChromeFooter)?,
                footer_key: binding(ThemeBinding::ChromeFooterKey)?,
                error: binding(ThemeBinding::ChromeError)?,
            },
            picker: PickerTheme {
                text: binding(ThemeBinding::PickerText)?,
                muted: binding(ThemeBinding::PickerMuted)?,
                selected: binding(ThemeBinding::PickerSelected)?,
                selected_muted: binding(ThemeBinding::PickerSelectedMuted)?,
                marker: binding(ThemeBinding::PickerMarker)?,
                scrollbar: binding(ThemeBinding::PickerScrollbar)?,
            },
            preview: PreviewTheme {
                text: binding(ThemeBinding::PreviewText)?,
                error: binding(ThemeBinding::PreviewError)?,
                border: binding(ThemeBinding::PreviewBorder)?,
            },
            capture: CaptureTheme {
                text: binding(ThemeBinding::CaptureText)?,
            },
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum ThemeRef {
    Builtin { name: String },
    Named { name: String },
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawTheme {
    #[serde(default)]
    palette: BTreeMap<String, String>,
    #[serde(default)]
    scheme: RawScheme,
    #[serde(default)]
    bindings: BTreeMap<String, RawBinding>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScheme {
    #[serde(default)]
    primary: Option<String>,
    #[serde(default, rename = "on-primary")]
    on_primary: Option<String>,
    #[serde(default, rename = "primary-container")]
    primary_container: Option<String>,
    #[serde(default, rename = "on-primary-container")]
    on_primary_container: Option<String>,
    #[serde(default)]
    surface: Option<String>,
    #[serde(default, rename = "surface-container")]
    surface_container: Option<String>,
    #[serde(default, rename = "on-surface")]
    on_surface: Option<String>,
    #[serde(default, rename = "on-surface-variant")]
    on_surface_variant: Option<String>,
    #[serde(default)]
    outline: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default, rename = "on-error")]
    on_error: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBinding {
    #[serde(default)]
    foreground: Option<String>,
    #[serde(default)]
    background: Option<String>,
    #[serde(default)]
    bold: Option<bool>,
    #[serde(default)]
    italic: Option<bool>,
    #[serde(default)]
    underline: Option<bool>,
    #[serde(default)]
    strikethrough: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
struct ResolvedScheme {
    primary: Color,
    on_primary: Color,
    primary_container: Color,
    on_primary_container: Color,
    surface: Color,
    surface_container: Color,
    on_surface: Color,
    on_surface_variant: Color,
    outline: Color,
    error: Color,
    on_error: Color,
}

impl ResolvedScheme {
    fn resolve(palette: &BTreeMap<String, Color>, raw: &RawScheme, source: &str) -> Result<Self> {
        let resolve = |role: SchemeRole, value: Option<&str>| match value {
            Some(value) => resolve_scheme_reference(value, palette, source, role),
            None => Ok(default_scheme_color(role)),
        };

        Ok(Self {
            primary: resolve(SchemeRole::Primary, raw.primary.as_deref())?,
            on_primary: resolve(SchemeRole::OnPrimary, raw.on_primary.as_deref())?,
            primary_container: resolve(
                SchemeRole::PrimaryContainer,
                raw.primary_container.as_deref(),
            )?,
            on_primary_container: resolve(
                SchemeRole::OnPrimaryContainer,
                raw.on_primary_container.as_deref(),
            )?,
            surface: resolve(SchemeRole::Surface, raw.surface.as_deref())?,
            surface_container: resolve(
                SchemeRole::SurfaceContainer,
                raw.surface_container.as_deref(),
            )?,
            on_surface: resolve(SchemeRole::OnSurface, raw.on_surface.as_deref())?,
            on_surface_variant: resolve(
                SchemeRole::OnSurfaceVariant,
                raw.on_surface_variant.as_deref(),
            )?,
            outline: resolve(SchemeRole::Outline, raw.outline.as_deref())?,
            error: resolve(SchemeRole::Error, raw.error.as_deref())?,
            on_error: resolve(SchemeRole::OnError, raw.on_error.as_deref())?,
        })
    }

    fn color(self, role: SchemeRole) -> Color {
        match role {
            SchemeRole::Primary => self.primary,
            SchemeRole::OnPrimary => self.on_primary,
            SchemeRole::PrimaryContainer => self.primary_container,
            SchemeRole::OnPrimaryContainer => self.on_primary_container,
            SchemeRole::Surface => self.surface,
            SchemeRole::SurfaceContainer => self.surface_container,
            SchemeRole::OnSurface => self.on_surface,
            SchemeRole::OnSurfaceVariant => self.on_surface_variant,
            SchemeRole::Outline => self.outline,
            SchemeRole::Error => self.error,
            SchemeRole::OnError => self.on_error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchemeRole {
    Primary,
    OnPrimary,
    PrimaryContainer,
    OnPrimaryContainer,
    Surface,
    SurfaceContainer,
    OnSurface,
    OnSurfaceVariant,
    Outline,
    Error,
    OnError,
}

impl SchemeRole {
    const ALL: &'static [Self] = &[
        Self::Primary,
        Self::OnPrimary,
        Self::PrimaryContainer,
        Self::OnPrimaryContainer,
        Self::Surface,
        Self::SurfaceContainer,
        Self::OnSurface,
        Self::OnSurfaceVariant,
        Self::Outline,
        Self::Error,
        Self::OnError,
    ];

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|role| role.name() == value)
    }

    fn all_names() -> String {
        Self::ALL
            .iter()
            .map(|role| role.name())
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn name(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::OnPrimary => "on-primary",
            Self::PrimaryContainer => "primary-container",
            Self::OnPrimaryContainer => "on-primary-container",
            Self::Surface => "surface",
            Self::SurfaceContainer => "surface-container",
            Self::OnSurface => "on-surface",
            Self::OnSurfaceVariant => "on-surface-variant",
            Self::Outline => "outline",
            Self::Error => "error",
            Self::OnError => "on-error",
        }
    }
}

impl std::fmt::Display for SchemeRole {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

fn default_scheme_color(role: SchemeRole) -> Color {
    match role {
        SchemeRole::Primary | SchemeRole::OnPrimaryContainer => Color::Cyan,
        SchemeRole::OnPrimary => Color::Black,
        SchemeRole::Error => Color::Red,
        SchemeRole::OnError => Color::White,
        SchemeRole::PrimaryContainer
        | SchemeRole::Surface
        | SchemeRole::SurfaceContainer
        | SchemeRole::OnSurface
        | SchemeRole::OnSurfaceVariant
        | SchemeRole::Outline => Color::Reset,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThemeBinding {
    Text,
    MutedText,
    ChromeDivider,
    ChromeInputPrefix,
    ChromeFooter,
    ChromeFooterKey,
    ChromeError,
    PickerText,
    PickerMuted,
    PickerSelected,
    PickerSelectedMuted,
    PickerMarker,
    PickerScrollbar,
    PreviewText,
    PreviewError,
    PreviewBorder,
    CaptureText,
}

impl ThemeBinding {
    const ALL: &'static [Self] = &[
        Self::Text,
        Self::MutedText,
        Self::ChromeDivider,
        Self::ChromeInputPrefix,
        Self::ChromeFooter,
        Self::ChromeFooterKey,
        Self::ChromeError,
        Self::PickerText,
        Self::PickerMuted,
        Self::PickerSelected,
        Self::PickerSelectedMuted,
        Self::PickerMarker,
        Self::PickerScrollbar,
        Self::PreviewText,
        Self::PreviewError,
        Self::PreviewBorder,
        Self::CaptureText,
    ];

    fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|binding| binding.name() == value)
    }

    fn all_names() -> String {
        Self::ALL
            .iter()
            .map(|binding| binding.name())
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::MutedText => "muted-text",
            Self::ChromeDivider => "chrome-divider",
            Self::ChromeInputPrefix => "chrome-input-prefix",
            Self::ChromeFooter => "chrome-footer",
            Self::ChromeFooterKey => "chrome-footer-key",
            Self::ChromeError => "chrome-error",
            Self::PickerText => "picker-text",
            Self::PickerMuted => "picker-muted",
            Self::PickerSelected => "picker-selected",
            Self::PickerSelectedMuted => "picker-selected-muted",
            Self::PickerMarker => "picker-marker",
            Self::PickerScrollbar => "picker-scrollbar",
            Self::PreviewText => "preview-text",
            Self::PreviewError => "preview-error",
            Self::PreviewBorder => "preview-border",
            Self::CaptureText => "capture-text",
        }
    }
}

impl std::fmt::Display for ThemeBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

#[derive(Debug, Clone, Copy)]
struct BindingDefault {
    foreground: SchemeRole,
    background: SchemeRole,
    bold: bool,
}

fn default_binding(binding: ThemeBinding) -> BindingDefault {
    match binding {
        ThemeBinding::Text => BindingDefault {
            foreground: SchemeRole::OnSurface,
            background: SchemeRole::Surface,
            bold: false,
        },
        ThemeBinding::MutedText => BindingDefault {
            foreground: SchemeRole::OnSurfaceVariant,
            background: SchemeRole::Surface,
            bold: false,
        },
        ThemeBinding::ChromeDivider => BindingDefault {
            foreground: SchemeRole::Outline,
            background: SchemeRole::Surface,
            bold: false,
        },
        ThemeBinding::ChromeInputPrefix => BindingDefault {
            foreground: SchemeRole::OnPrimaryContainer,
            background: SchemeRole::PrimaryContainer,
            bold: true,
        },
        ThemeBinding::ChromeFooter => BindingDefault {
            foreground: SchemeRole::OnSurfaceVariant,
            background: SchemeRole::SurfaceContainer,
            bold: false,
        },
        ThemeBinding::ChromeFooterKey => BindingDefault {
            foreground: SchemeRole::OnPrimaryContainer,
            background: SchemeRole::PrimaryContainer,
            bold: true,
        },
        ThemeBinding::ChromeError => BindingDefault {
            foreground: SchemeRole::OnError,
            background: SchemeRole::Error,
            bold: true,
        },
        ThemeBinding::PickerText => BindingDefault {
            foreground: SchemeRole::OnSurface,
            background: SchemeRole::Surface,
            bold: false,
        },
        ThemeBinding::PickerMuted => BindingDefault {
            foreground: SchemeRole::OnSurfaceVariant,
            background: SchemeRole::Surface,
            bold: false,
        },
        ThemeBinding::PickerSelected => BindingDefault {
            foreground: SchemeRole::OnPrimaryContainer,
            background: SchemeRole::PrimaryContainer,
            bold: true,
        },
        ThemeBinding::PickerSelectedMuted => BindingDefault {
            foreground: SchemeRole::OnPrimaryContainer,
            background: SchemeRole::PrimaryContainer,
            bold: false,
        },
        ThemeBinding::PickerMarker | ThemeBinding::PickerScrollbar => BindingDefault {
            foreground: SchemeRole::Primary,
            background: SchemeRole::Surface,
            bold: true,
        },
        ThemeBinding::PreviewText => BindingDefault {
            foreground: SchemeRole::OnSurface,
            background: SchemeRole::Surface,
            bold: false,
        },
        ThemeBinding::PreviewError => BindingDefault {
            foreground: SchemeRole::OnError,
            background: SchemeRole::Error,
            bold: false,
        },
        ThemeBinding::PreviewBorder => BindingDefault {
            foreground: SchemeRole::Outline,
            background: SchemeRole::Surface,
            bold: false,
        },
        ThemeBinding::CaptureText => BindingDefault {
            foreground: SchemeRole::OnSurface,
            background: SchemeRole::Surface,
            bold: false,
        },
    }
}

fn validate_bindings(bindings: &BTreeMap<String, RawBinding>, source: &str) -> Result<()> {
    for name in bindings.keys() {
        if ThemeBinding::parse(name).is_none() {
            bail!(
                "{source} binding {:?} is unsupported; expected {}",
                name,
                ThemeBinding::all_names()
            );
        }
    }
    Ok(())
}

fn resolve_binding(
    binding: ThemeBinding,
    raw_bindings: &BTreeMap<String, RawBinding>,
    scheme: &ResolvedScheme,
    source: &str,
) -> Result<Style> {
    let patch = raw_bindings.get(binding.name());
    let defaults = default_binding(binding);
    let foreground = match patch.and_then(|patch| patch.foreground.as_deref()) {
        Some(value) => resolve_binding_reference(value, scheme, source, binding, "foreground")?,
        None => scheme.color(defaults.foreground),
    };
    let background = match patch.and_then(|patch| patch.background.as_deref()) {
        Some(value) => resolve_binding_reference(value, scheme, source, binding, "background")?,
        None => scheme.color(defaults.background),
    };
    let mut style = set_modifier(
        Style::new().fg(foreground).bg(background),
        Modifier::BOLD,
        Some(patch.and_then(|patch| patch.bold).unwrap_or(defaults.bold)),
    );
    for (modifier, enabled) in [
        (Modifier::ITALIC, patch.and_then(|patch| patch.italic)),
        (
            Modifier::UNDERLINED,
            patch.and_then(|patch| patch.underline),
        ),
        (
            Modifier::CROSSED_OUT,
            patch.and_then(|patch| patch.strikethrough),
        ),
    ] {
        style = set_modifier(style, modifier, Some(enabled.unwrap_or(false)));
    }
    Ok(style)
}

fn resolve_binding_reference(
    value: &str,
    scheme: &ResolvedScheme,
    source: &str,
    binding: ThemeBinding,
    field: &str,
) -> Result<Color> {
    let value = value.trim();
    let Some(role_name) = value.strip_prefix("scheme:") else {
        bail!("{source} bindings.{binding}.{field} must reference a scheme role as scheme:ROLE");
    };
    let Some(role) = SchemeRole::parse(role_name) else {
        bail!(
            "{source} bindings.{binding}.{field} references unsupported scheme role {:?}; expected {}",
            role_name,
            SchemeRole::all_names()
        );
    };
    Ok(scheme.color(role))
}

fn resolve_scheme_reference(
    value: &str,
    palette: &BTreeMap<String, Color>,
    source: &str,
    role: SchemeRole,
) -> Result<Color> {
    let value = value.trim();
    if let Some(palette_name) = value.strip_prefix("palette:") {
        let Some(color) = palette.get(palette_name) else {
            bail!(
                "{source} scheme.{role} references unknown palette color {:?}",
                palette_name
            );
        };
        return Ok(*color);
    }
    if let Some(ansi_name) = value.strip_prefix("ansi:") {
        return parse_ansi_color(ansi_name).ok_or_else(|| {
            anyhow::anyhow!(
                "{source} scheme.{role} references unsupported ANSI color {:?}; expected {}",
                ansi_name,
                ansi_color_names()
            )
        });
    }
    bail!("{source} scheme.{role} must reference a color as palette:NAME or ansi:COLOR")
}

fn resolve_palette(
    overrides: &BTreeMap<String, String>,
    source: &str,
) -> Result<BTreeMap<String, Color>> {
    let mut palette = BTreeMap::new();
    for (name, value) in overrides {
        if name.trim().is_empty() {
            bail!("{source} palette names must not be empty");
        }
        let color = parse_color_value(value).map_err(|error| {
            anyhow::anyhow!(
                "{source} palette.{name} has {error}; expected a basic ANSI color or #RRGGBB"
            )
        })?;
        palette.insert(name.clone(), color);
    }
    Ok(palette)
}

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

fn ansi_color_names() -> &'static str {
    "black, red, green, yellow, blue, magenta, cyan, gray, or white"
}

fn parse_ansi_color(value: &str) -> Option<Color> {
    match value.to_ascii_lowercase().as_str() {
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
}

fn parse_color_value(value: &str) -> Result<Color, String> {
    let value = value.trim();
    parse_ansi_color(value)
        .or_else(|| parse_hex_color(value))
        .ok_or_else(|| format!("unsupported color {value:?}"))
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
        path::PathBuf,
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

    fn resolved_binding_style(theme: &ResolvedTheme, binding: ThemeBinding) -> Style {
        match binding {
            ThemeBinding::Text => theme.text,
            ThemeBinding::MutedText => theme.muted_text,
            ThemeBinding::ChromeDivider => theme.chrome.divider,
            ThemeBinding::ChromeInputPrefix => theme.chrome.input_prefix,
            ThemeBinding::ChromeFooter => theme.chrome.footer,
            ThemeBinding::ChromeFooterKey => theme.chrome.footer_key,
            ThemeBinding::ChromeError => theme.chrome.error,
            ThemeBinding::PickerText => theme.picker.text,
            ThemeBinding::PickerMuted => theme.picker.muted,
            ThemeBinding::PickerSelected => theme.picker.selected,
            ThemeBinding::PickerSelectedMuted => theme.picker.selected_muted,
            ThemeBinding::PickerMarker => theme.picker.marker,
            ThemeBinding::PickerScrollbar => theme.picker.scrollbar,
            ThemeBinding::PreviewText => theme.preview.text,
            ThemeBinding::PreviewError => theme.preview.error,
            ThemeBinding::PreviewBorder => theme.preview.border,
            ThemeBinding::CaptureText => theme.capture.text,
        }
    }

    #[test]
    fn scheme_role_names_cover_raw_and_resolved_fields() {
        let mut names = Vec::new();
        for &role in SchemeRole::ALL {
            let name = role.name();
            assert!(names.iter().all(|known| *known != name));
            names.push(name);
            assert_eq!(SchemeRole::parse(name), Some(role));

            let raw: RawTheme =
                toml::from_str(&format!("[scheme]\n{name} = \"ansi:magenta\"\n")).unwrap();
            let resolved =
                ResolvedScheme::resolve(&BTreeMap::new(), &raw.scheme, "test theme").unwrap();
            assert_eq!(resolved.color(role), Color::Magenta, "scheme role {name}");
        }
        assert_eq!(SchemeRole::all_names(), names.join(", "));
    }

    #[test]
    fn binding_names_cover_raw_and_resolved_fields() {
        let mut names = Vec::new();
        for &binding in ThemeBinding::ALL {
            let name = binding.name();
            assert!(names.iter().all(|known| *known != name));
            names.push(name);
            assert_eq!(ThemeBinding::parse(name), Some(binding));

            let raw: RawTheme =
                toml::from_str(&format!("[bindings.{name}]\nitalic = true\n")).unwrap();
            let theme = ResolvedTheme::from_raw(&raw, "test theme").unwrap();
            assert!(
                resolved_binding_style(&theme, binding)
                    .add_modifier
                    .contains(Modifier::ITALIC),
                "binding {name} did not resolve"
            );
        }
        assert_eq!(ThemeBinding::all_names(), names.join(", "));
    }

    #[test]
    fn terminal_theme_contains_default_bindings() {
        let theme = ResolvedTheme::terminal();

        assert_eq!(theme.text.fg, Some(Color::Reset));
        assert_eq!(theme.text.bg, Some(Color::Reset));
        assert_eq!(theme.chrome.input_prefix.fg, Some(Color::Cyan));
        assert_eq!(theme.chrome.input_prefix.bg, Some(Color::Reset));
        assert!(
            theme
                .chrome
                .input_prefix
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(theme.picker.selected.fg, Some(Color::Cyan));
        assert_eq!(theme.picker.selected.bg, Some(Color::Reset));
        assert_eq!(theme.picker.marker.fg, Some(Color::Cyan));
        assert_eq!(theme.chrome.error.fg, Some(Color::White));
        assert_eq!(theme.chrome.error.bg, Some(Color::Red));
        assert_eq!(theme.preview.text.fg, Some(Color::Reset));
        assert_eq!(theme.preview.error.fg, Some(Color::White));
        assert_eq!(theme.preview.error.bg, Some(Color::Red));
        assert_eq!(theme.preview.border.fg, Some(Color::Reset));
        assert_eq!(theme.capture.text.fg, Some(Color::Reset));
    }

    #[test]
    fn palette_scheme_and_binding_references_resolve() {
        let raw: RawTheme = toml::from_str(
            r##"
            [palette]
            brand = "#102030"
            paper = "#F2E9E1"
            ink = "#204060"

            [scheme]
            primary = "palette:brand"
            primary-container = "palette:paper"
            on-primary-container = "palette:ink"
            surface = "palette:paper"
            on-surface = "palette:ink"
            on-surface-variant = "palette:brand"
            outline = "palette:brand"

            [bindings.picker-selected]
            foreground = "scheme:on-primary-container"
            background = "scheme:primary-container"
            bold = true
            italic = true
            underline = true
            strikethrough = true

            [bindings.chrome-divider]
            foreground = "scheme:outline"

            [bindings.chrome-error]
            foreground = "scheme:on-error"
            background = "scheme:error"

            [bindings.preview-text]
            foreground = "scheme:on-surface-variant"

            [bindings.preview-error]
            foreground = "scheme:on-error"
            background = "scheme:error"

            [bindings.capture-text]
            foreground = "scheme:primary"
            "##,
        )
        .unwrap();
        let theme = ResolvedTheme::from_raw(&raw, "test theme").unwrap();

        assert_eq!(theme.picker.selected.fg, Some(Color::Rgb(32, 64, 96)));
        assert_eq!(theme.picker.selected.bg, Some(Color::Rgb(242, 233, 225)));
        assert!(theme.picker.selected.add_modifier.contains(Modifier::BOLD));
        assert!(
            theme
                .picker
                .selected
                .add_modifier
                .contains(Modifier::ITALIC)
        );
        assert!(
            theme
                .picker
                .selected
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
        assert!(
            theme
                .picker
                .selected
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
        assert_eq!(theme.chrome.divider.fg, Some(Color::Rgb(16, 32, 48)));
        assert_eq!(theme.chrome.divider.bg, Some(Color::Rgb(242, 233, 225)));
        assert_eq!(theme.text.fg, Some(Color::Rgb(32, 64, 96)));
        assert_eq!(theme.text.bg, Some(Color::Rgb(242, 233, 225)));
        assert_eq!(theme.chrome.error.fg, Some(Color::White));
        assert_eq!(theme.chrome.error.bg, Some(Color::Red));
        assert_eq!(theme.preview.text.fg, Some(Color::Rgb(16, 32, 48)));
        assert_eq!(theme.preview.error.fg, Some(Color::White));
        assert_eq!(theme.preview.error.bg, Some(Color::Red));
        assert_eq!(theme.capture.text.fg, Some(Color::Rgb(16, 32, 48)));
    }

    #[test]
    fn user_palette_names_do_not_shadow_ansi_or_builtin_scheme_colors() {
        let raw: RawTheme = toml::from_str(
            r##"
            [palette]
            cyan = "#102030"
            black = "#203040"
            red = "#304050"
            white = "#F0E0D0"

            [scheme]
            outline = "ansi:cyan"

            [bindings.text]
            foreground = "scheme:on-primary"
            "##,
        )
        .unwrap();
        let theme = ResolvedTheme::from_raw(&raw, "test theme").unwrap();

        assert_eq!(theme.picker.marker.fg, Some(Color::Cyan));
        assert_eq!(theme.picker.selected.fg, Some(Color::Cyan));
        assert_eq!(theme.text.fg, Some(Color::Black));
        assert_eq!(theme.chrome.error.bg, Some(Color::Red));
        assert_eq!(theme.chrome.error.fg, Some(Color::White));
        assert_eq!(theme.chrome.divider.fg, Some(Color::Cyan));
    }

    #[test]
    fn omitted_bindings_use_defaults_and_partial_bindings_are_supported() {
        let raw: RawTheme = toml::from_str(
            r##"
            [palette]
            quiet = "#696969"

            [scheme]
            on-surface-variant = "palette:quiet"

            [bindings.picker-marker]
            bold = false
            "##,
        )
        .unwrap();
        let theme = ResolvedTheme::from_raw(&raw, "test theme").unwrap();

        assert_eq!(theme.picker.marker.fg, Some(Color::Cyan));
        assert_eq!(theme.picker.marker.bg, Some(Color::Reset));
        assert!(!theme.picker.marker.add_modifier.contains(Modifier::BOLD));
        assert_eq!(theme.picker.muted.fg, Some(Color::Rgb(105, 105, 105)));
        assert!(
            !theme
                .picker
                .selected_muted
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(theme.chrome.footer_key.fg, Some(Color::Cyan));
    }

    #[test]
    fn theme_tables_are_strict_and_old_token_tables_are_rejected() {
        assert!(toml::from_str::<RawTheme>("[tokens]\naccent = \"cyan\"\n").is_err());
        assert!(toml::from_str::<RawTheme>("[bindings.text]\nreverse = true\n").is_err());
        assert!(toml::from_str::<RawTheme>("[scheme]\nprimaryy = \"palette:cyan\"\n").is_err());
        assert!(
            toml::from_str::<RawTheme>("extends = { source = \"builtin\", name = \"terminal\" }\n")
                .is_err()
        );

        let raw: RawTheme =
            toml::from_str("[bindings.unknown]\nforeground = \"scheme:primary\"\n").unwrap();
        let error = ResolvedTheme::from_raw(&raw, "test theme").unwrap_err();
        assert!(error.to_string().contains("unknown"));
    }

    #[test]
    fn explicit_refs_and_style_fields_keep_their_names_in_errors() {
        let ref_error =
            toml::from_str::<ThemeRef>("source = \"mystery\"\nname = \"x\"\n").unwrap_err();
        assert!(ref_error.to_string().contains("mystery"));

        let field_error =
            toml::from_str::<RawTheme>("[bindings.text]\nforegroundd = \"scheme:primary\"\n")
                .unwrap_err();
        assert!(field_error.to_string().contains("foregroundd"));
    }

    #[test]
    fn palette_values_must_be_supported_colors() {
        for color in [
            "bright-blue",
            "light-blue",
            "dark-gray",
            "grey",
            "silver",
            "42",
            "default",
            "terminal",
        ] {
            let raw: RawTheme =
                toml::from_str(&format!("[palette]\nprimary = {color:?}\n")).unwrap();
            let error = ResolvedTheme::from_raw(&raw, "test theme").unwrap_err();
            assert!(error.to_string().contains("unsupported color"));
        }
    }

    #[test]
    fn scheme_values_must_reference_known_palette_or_ansi_colors() {
        let raw: RawTheme = toml::from_str("[scheme]\nprimary = \"palette:missing\"\n").unwrap();
        let error = ResolvedTheme::from_raw(&raw, "test theme").unwrap_err();
        assert!(error.to_string().contains("unknown palette color"));

        let raw: RawTheme = toml::from_str("[scheme]\nprimary = \"#102030\"\n").unwrap();
        let error = ResolvedTheme::from_raw(&raw, "test theme").unwrap_err();
        assert!(error.to_string().contains("palette:NAME or ansi:COLOR"));

        let raw: RawTheme = toml::from_str("[scheme]\nprimary = \"palette:terminal\"\n").unwrap();
        let error = ResolvedTheme::from_raw(&raw, "test theme").unwrap_err();
        assert!(error.to_string().contains("unknown palette color"));

        let raw: RawTheme = toml::from_str("[scheme]\nprimary = \"ansi:bright-blue\"\n").unwrap();
        let error = ResolvedTheme::from_raw(&raw, "test theme").unwrap_err();
        assert!(error.to_string().contains("unsupported ANSI color"));

        let raw: RawTheme = toml::from_str("[scheme]\nprimary = \"ansi:magenta\"\n").unwrap();
        let theme = ResolvedTheme::from_raw(&raw, "test theme").unwrap();
        assert_eq!(theme.picker.marker.fg, Some(Color::Magenta));
    }

    #[test]
    fn configured_named_refs_are_relative_to_the_config_directory() {
        let root = temporary_root();
        fs::create_dir_all(root.join("themes")).unwrap();
        fs::write(
            root.join("themes/work.toml"),
            "[palette]\nbrand = \"blue\"\n\n[scheme]\nprimary = \"palette:brand\"\n",
        )
        .unwrap();

        let theme = load(
            &root.join("config.toml"),
            Some("work"),
            &ThemeLoadOptions::default(),
        )
        .unwrap();

        assert_eq!(theme.picker.marker.fg, Some(Color::Blue));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn named_refs_resolve_below_the_config_directory() {
        let root = temporary_root();
        let themes = root.join("themes");
        fs::create_dir_all(&themes).unwrap();
        fs::write(
            themes.join("work.toml"),
            "[palette]\nbrand = \"yellow\"\nquiet = \"gray\"\n\n[scheme]\nprimary = \"palette:brand\"\non-surface-variant = \"palette:quiet\"\n",
        )
        .unwrap();
        let theme = load(
            &root.join("config.toml"),
            Some("work"),
            &ThemeLoadOptions::default(),
        )
        .unwrap();

        assert_eq!(theme.picker.marker.fg, Some(Color::Yellow));
        assert_eq!(theme.picker.muted.fg, Some(Color::Gray));
        fs::remove_dir_all(root).unwrap();
    }
}
