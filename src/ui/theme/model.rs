use super::binding::{ThemeBinding, resolve_binding, validate_bindings};
use super::color::{ResolvedScheme, resolve_palette};
use anyhow::Result;
use ratatui::style::Style;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolvedTheme {
    pub(crate) text: Style,
    #[cfg(test)]
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

    pub(super) fn from_raw(raw: &RawTheme, source: &str) -> Result<Self> {
        let palette = resolve_palette(&raw.palette, source)?;
        validate_bindings(&raw.bindings, source)?;
        let scheme = ResolvedScheme::resolve(&palette, &raw.scheme, source)?;
        let binding = |binding| resolve_binding(binding, &raw.bindings, &scheme, source);

        Ok(Self {
            text: binding(ThemeBinding::Text)?,
            #[cfg(test)]
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
pub(super) struct RawTheme {
    #[serde(default)]
    pub(super) palette: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) scheme: RawScheme,
    #[serde(default)]
    pub(super) bindings: BTreeMap<String, RawBinding>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawScheme {
    #[serde(default)]
    pub(super) primary: Option<String>,
    #[serde(default, rename = "on-primary")]
    pub(super) on_primary: Option<String>,
    #[serde(default, rename = "primary-container")]
    pub(super) primary_container: Option<String>,
    #[serde(default, rename = "on-primary-container")]
    pub(super) on_primary_container: Option<String>,
    #[serde(default)]
    pub(super) surface: Option<String>,
    #[serde(default, rename = "surface-container")]
    pub(super) surface_container: Option<String>,
    #[serde(default, rename = "on-surface")]
    pub(super) on_surface: Option<String>,
    #[serde(default, rename = "on-surface-variant")]
    pub(super) on_surface_variant: Option<String>,
    #[serde(default)]
    pub(super) outline: Option<String>,
    #[serde(default)]
    pub(super) error: Option<String>,
    #[serde(default, rename = "on-error")]
    pub(super) on_error: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawBinding {
    #[serde(default)]
    pub(super) foreground: Option<String>,
    #[serde(default)]
    pub(super) background: Option<String>,
    #[serde(default)]
    pub(super) bold: Option<bool>,
    #[serde(default)]
    pub(super) italic: Option<bool>,
    #[serde(default)]
    pub(super) underline: Option<bool>,
    #[serde(default)]
    pub(super) strikethrough: Option<bool>,
}
