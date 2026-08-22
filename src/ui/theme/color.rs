use super::model::RawScheme;
use anyhow::{Result, bail};
use ratatui::style::Color;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub(super) struct ResolvedScheme {
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
    pub(super) fn resolve(
        palette: &BTreeMap<String, Color>,
        raw: &RawScheme,
        source: &str,
    ) -> Result<Self> {
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

    pub(super) fn color(self, role: SchemeRole) -> Color {
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
pub(super) enum SchemeRole {
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
    pub(super) const ALL: &'static [Self] = &[
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

    pub(super) fn parse(value: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|role| role.name() == value)
    }

    pub(super) fn all_names() -> String {
        Self::ALL
            .iter()
            .map(|role| role.name())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(super) fn name(self) -> &'static str {
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

pub(super) fn resolve_palette(
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

pub(super) fn ansi_color_names() -> &'static str {
    "black, red, green, yellow, blue, magenta, cyan, gray, or white"
}

pub(super) fn parse_ansi_color(value: &str) -> Option<Color> {
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
