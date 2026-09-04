use super::binding::validate_bindings;
use super::color::{ResolvedScheme, resolve_palette};
use anyhow::Result;
use ratatui::style::{Color, Style};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RawStyleBinding {
    #[serde(default)]
    pub foreground: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub bold: Option<bool>,
    #[serde(default)]
    pub italic: Option<bool>,
    #[serde(default)]
    pub underline: Option<bool>,
    #[serde(default)]
    pub strikethrough: Option<bool>,
    #[serde(default)]
    pub dim: Option<bool>,
    #[serde(default)]
    pub reversed: Option<bool>,
    #[serde(default)]
    pub selected: Option<Box<RawStyleBinding>>,
}

impl RawStyleBinding {
    pub fn merge_with(&self, base: &RawStyleBinding) -> RawStyleBinding {
        RawStyleBinding {
            foreground: self.foreground.clone().or_else(|| base.foreground.clone()),
            background: self.background.clone().or_else(|| base.background.clone()),
            bold: self.bold.or(base.bold),
            italic: self.italic.or(base.italic),
            underline: self.underline.or(base.underline),
            strikethrough: self.strikethrough.or(base.strikethrough),
            dim: self.dim.or(base.dim),
            reversed: self.reversed.or(base.reversed),
            selected: match (&self.selected, &base.selected) {
                (Some(s), Some(b)) => Some(Box::new(s.merge_with(b))),
                (Some(s), None) => Some(s.clone()),
                (None, Some(b)) => Some(b.clone()),
                (None, None) => None,
            },
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPluginThemeOverride {
    #[serde(default)]
    pub(super) styles: BTreeMap<String, RawStyleBinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolvedCustomStyle {
    pub(crate) normal: Style,
    pub(crate) selected: Option<Style>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedTheme {
    pub(crate) text: Style,
    #[cfg(test)]
    pub(crate) muted_text: Style,
    pub(crate) chrome: ChromeTheme,
    pub(crate) picker: PickerTheme,
    pub(crate) preview: PreviewTheme,
    pub(crate) capture: CaptureTheme,
    pub(super) scheme: ResolvedScheme,
    pub(super) palette: Arc<BTreeMap<String, Color>>,
    pub(super) raw_theme_overrides: Arc<BTreeMap<(String, String), RawStyleBinding>>,
    pub(super) custom_styles: Arc<BTreeMap<(String, String), ResolvedCustomStyle>>,
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
    pub(crate) badge: Style,
    pub(crate) badge_selected: Style,
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

pub(super) const DEFAULT_THEME_TOML: &str = include_str!("builtin/terminal.toml");

pub(super) fn default_raw_theme() -> &'static RawTheme {
    static THEME: std::sync::OnceLock<RawTheme> = std::sync::OnceLock::new();
    THEME.get_or_init(|| {
        toml::from_str(DEFAULT_THEME_TOML).expect("builtin terminal.toml must be valid")
    })
}

impl ResolvedTheme {
    pub(crate) fn terminal() -> Self {
        Self::from_raw(default_raw_theme(), "builtin terminal")
            .expect("builtin terminal theme must be valid")
    }

    pub(super) fn from_raw(raw: &RawTheme, source: &str) -> Result<Self> {
        let default_raw = default_raw_theme();
        let palette = Arc::new(resolve_palette(&raw.palette, source)?);
        validate_bindings(&raw.bindings, source)?;
        let scheme = ResolvedScheme::resolve(&palette, &raw.scheme, source)?;

        let resolve_component = |configured: Option<&RawStyleBinding>,
                                 legacy_name: Option<&str>,
                                 default: &RawStyleBinding,
                                 desc: &str|
         -> Result<ResolvedCustomStyle> {
            let mut merged = default.clone();
            if let Some(conf) = configured {
                merged = conf.merge_with(&merged);
            } else if let Some(leg_name) = legacy_name {
                if let Some(leg) = raw.bindings.get(leg_name) {
                    merged = leg.to_style_binding().merge_with(&merged);
                }
            }
            resolve_raw_style_binding(&merged, &scheme, &palette, source, desc)
        };

        let picker_text = resolve_component(
            raw.picker.text.as_ref(),
            Some("picker-text"),
            default_raw.picker.text.as_ref().unwrap(),
            "picker.text",
        )?.normal;

        let picker_muted = resolve_component(
            raw.picker.muted.as_ref(),
            Some("picker-muted"),
            default_raw.picker.muted.as_ref().unwrap(),
            "picker.muted",
        )?.normal;

        let picker_selected = resolve_component(
            raw.picker.selected.as_ref(),
            Some("picker-selected"),
            default_raw.picker.selected.as_ref().unwrap(),
            "picker.selected",
        )?.normal;

        let picker_selected_muted = resolve_component(
            raw.picker.selected_muted.as_ref(),
            Some("picker-selected-muted"),
            default_raw.picker.selected_muted.as_ref().unwrap(),
            "picker.selected_muted",
        )?.normal;

        let picker_marker = resolve_component(
            raw.picker.marker.as_ref(),
            Some("picker-marker"),
            default_raw.picker.marker.as_ref().unwrap(),
            "picker.marker",
        )?.normal;

        let picker_scrollbar = resolve_component(
            raw.picker.scrollbar.as_ref(),
            Some("picker-scrollbar"),
            default_raw.picker.scrollbar.as_ref().unwrap(),
            "picker.scrollbar",
        )?.normal;

        let badge_default = default_raw.picker.badge.as_ref().unwrap();
        let badge_merged = if let Some(conf) = raw.picker.badge.as_ref() {
            let mut m = conf.merge_with(badge_default);
            if let Some(conf_sel) = raw.picker.badge_selected.as_ref() {
                m.selected = Some(Box::new(match m.selected {
                    Some(existing) => conf_sel.merge_with(&existing),
                    None => conf_sel.clone(),
                }));
            }
            m
        } else {
            let mut m = badge_default.clone();
            if let Some(leg) = raw.bindings.get("picker-badge") {
                m = leg.to_style_binding().merge_with(&m);
            }
            if let Some(leg_sel) = raw.bindings.get("picker-badge-selected") {
                let sel_patch = leg_sel.to_style_binding();
                m.selected = Some(Box::new(match m.selected {
                    Some(existing) => sel_patch.merge_with(&existing),
                    None => sel_patch,
                }));
            }
            if let Some(conf_sel) = raw.picker.badge_selected.as_ref() {
                m.selected = Some(Box::new(match m.selected {
                    Some(existing) => conf_sel.merge_with(&existing),
                    None => conf_sel.clone(),
                }));
            }
            m
        };
        let resolved_badge = resolve_raw_style_binding(
            &badge_merged,
            &scheme,
            &palette,
            source,
            "picker.badge",
        )?;
        let picker_badge = resolved_badge.normal;
        let picker_badge_selected = if let Some(sel) = resolved_badge.selected {
            let mut s = sel;
            if s.bg.is_none() {
                if let Some(bg) = picker_selected.bg {
                    s = s.bg(bg);
                }
            }
            s
        } else {
            let mut s = picker_badge;
            if let Some(bg) = picker_selected.bg {
                s = s.bg(bg);
            }
            s
        };

        let chrome_divider = resolve_component(
            raw.chrome.divider.as_ref(),
            Some("chrome-divider"),
            default_raw.chrome.divider.as_ref().unwrap(),
            "chrome.divider",
        )?.normal;

        let chrome_input_prefix = resolve_component(
            raw.chrome.input_prefix.as_ref(),
            Some("chrome-input-prefix"),
            default_raw.chrome.input_prefix.as_ref().unwrap(),
            "chrome.input_prefix",
        )?.normal;

        let chrome_footer = resolve_component(
            raw.chrome.footer.as_ref(),
            Some("chrome-footer"),
            default_raw.chrome.footer.as_ref().unwrap(),
            "chrome.footer",
        )?.normal;

        let chrome_footer_key = resolve_component(
            raw.chrome.footer_key.as_ref(),
            Some("chrome-footer-key"),
            default_raw.chrome.footer_key.as_ref().unwrap(),
            "chrome.footer_key",
        )?.normal;

        let chrome_error = resolve_component(
            raw.chrome.error.as_ref(),
            Some("chrome-error"),
            default_raw.chrome.error.as_ref().unwrap(),
            "chrome.error",
        )?.normal;

        let preview_text = resolve_component(
            raw.preview.text.as_ref(),
            Some("preview-text"),
            default_raw.preview.text.as_ref().unwrap(),
            "preview.text",
        )?.normal;

        let preview_error = resolve_component(
            raw.preview.error.as_ref(),
            Some("preview-error"),
            default_raw.preview.error.as_ref().unwrap(),
            "preview.error",
        )?.normal;

        let preview_border = resolve_component(
            raw.preview.border.as_ref(),
            Some("preview-border"),
            default_raw.preview.border.as_ref().unwrap(),
            "preview.border",
        )?.normal;

        let capture_text = resolve_component(
            raw.capture.text.as_ref(),
            Some("capture-text"),
            default_raw.capture.text.as_ref().unwrap(),
            "capture.text",
        )?.normal;

        let text_default = default_raw.bindings.get("text").unwrap().to_style_binding();
        let text = resolve_component(
            None,
            Some("text"),
            &text_default,
            "text",
        )?.normal;

        #[cfg(test)]
        let muted_text_default = default_raw.bindings.get("muted-text").unwrap().to_style_binding();
        #[cfg(test)]
        let muted_text = resolve_component(
            None,
            Some("muted-text"),
            &muted_text_default,
            "muted-text",
        )?.normal;

        let mut raw_theme_overrides = BTreeMap::new();
        let mut custom_styles = BTreeMap::new();
        for (plugin_id, plugin_override) in &raw.plugins {
            for (slot_name, style_binding) in &plugin_override.styles {
                let key = (plugin_id.clone(), slot_name.clone());
                let context = format!("plugins.{plugin_id}.styles.{slot_name}");
                let resolved = resolve_raw_style_binding(
                    style_binding,
                    &scheme,
                    &palette,
                    source,
                    &context,
                )?;
                custom_styles.insert(key.clone(), resolved);
                raw_theme_overrides.insert(key, style_binding.clone());
            }
        }

        Ok(Self {
            text,
            #[cfg(test)]
            muted_text,
            chrome: ChromeTheme {
                divider: chrome_divider,
                input_prefix: chrome_input_prefix,
                footer: chrome_footer,
                footer_key: chrome_footer_key,
                error: chrome_error,
            },
            picker: PickerTheme {
                text: picker_text,
                muted: picker_muted,
                selected: picker_selected,
                selected_muted: picker_selected_muted,
                badge: picker_badge,
                badge_selected: picker_badge_selected,
                marker: picker_marker,
                scrollbar: picker_scrollbar,
            },
            preview: PreviewTheme {
                text: preview_text,
                error: preview_error,
                border: preview_border,
            },
            capture: CaptureTheme {
                text: capture_text,
            },
            scheme,
            palette,
            raw_theme_overrides: Arc::new(raw_theme_overrides),
            custom_styles: Arc::new(custom_styles),
        })
    }

    pub(crate) fn register_plugin_defaults(
        &mut self,
        plugin_id: &str,
        styles: &BTreeMap<String, RawStyleBinding>,
    ) -> Result<()> {
        let mut custom_styles = (*self.custom_styles).clone();
        for (slot_name, default_binding) in styles {
            let key = (plugin_id.to_string(), slot_name.clone());
            let resolved_style = if let Some(theme_override) = self.raw_theme_overrides.get(&key) {
                let merged = theme_override.merge_with(default_binding);
                resolve_raw_style_binding(
                    &merged,
                    &self.scheme,
                    &self.palette,
                    &format!("theme override [plugins.{plugin_id}.styles.{slot_name}]"),
                    &format!("plugins.{plugin_id}.styles.{slot_name}"),
                )?
            } else {
                resolve_raw_style_binding(
                    default_binding,
                    &self.scheme,
                    &self.palette,
                    &format!("plugin {:?} [styles.{slot_name}]", plugin_id),
                    &format!("styles.{slot_name}"),
                )?
            };
            custom_styles.insert(key, resolved_style);
        }
        self.custom_styles = Arc::new(custom_styles);
        Ok(())
    }

    pub(crate) fn register_all_plugin_defaults(
        &mut self,
        plugins: &BTreeMap<String, crate::workflow::config::PluginMetadata>,
    ) -> Result<()> {
        for (plugin_id, metadata) in plugins {
            self.register_plugin_defaults(plugin_id, &metadata.styles)?;
        }
        Ok(())
    }

    pub(crate) fn resolve_slot(
        &self,
        plugin_id: &str,
        slot: &crate::engine::SlotToken,
        selected: bool,
    ) -> Style {
        if let Some(custom) = self
            .custom_styles
            .get(&(plugin_id.to_string(), slot.as_str().to_string()))
            .or_else(|| {
                if !plugin_id.is_empty() {
                    self.custom_styles.get(&("".to_string(), slot.as_str().to_string()))
                } else {
                    None
                }
            })
        {
            if selected {
                if let Some(sel) = custom.selected {
                    let mut s = sel;
                    if s.bg.is_none() {
                        if let Some(bg) = self.picker.selected.bg {
                            s = s.bg(bg);
                        }
                    }
                    s
                } else {
                    let mut sel = custom.normal;
                    if let Some(bg) = self.picker.selected.bg {
                        sel = sel.bg(bg);
                    }
                    sel
                }
            } else {
                custom.normal
            }
        } else {
            self.resolve_builtin_slot(slot, selected)
        }
    }

    fn resolve_builtin_slot(
        &self,
        slot: &crate::engine::SlotToken,
        selected: bool,
    ) -> Style {
        use crate::engine::SlotToken;
        use ratatui::style::Modifier;

        if selected {
            let base = self.picker.selected;
            match slot {
                SlotToken::Primary => base,
                SlotToken::Secondary => self.picker.selected_muted,
                SlotToken::Muted => self.picker.selected_muted.add_modifier(Modifier::DIM),
                SlotToken::Accent => base.add_modifier(Modifier::BOLD),
                SlotToken::Badge => self.picker.badge_selected,
                SlotToken::Success => base,
                SlotToken::Warning => self.chrome.error,
                SlotToken::Error => self.chrome.error,
                SlotToken::Custom(_) => base,
            }
        } else {
            match slot {
                SlotToken::Primary => self.picker.text,
                SlotToken::Secondary => self.picker.muted,
                SlotToken::Muted => self.picker.muted.add_modifier(Modifier::DIM),
                SlotToken::Accent => self.picker.text.add_modifier(Modifier::BOLD),
                SlotToken::Badge => self.picker.badge,
                SlotToken::Success => self.picker.text,
                SlotToken::Warning => self.chrome.error,
                SlotToken::Error => self.chrome.error,
                SlotToken::Custom(_) => self.picker.text,
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn resolve_slot_style(
        &self,
        slot: crate::engine::SlotToken,
        selected: bool,
    ) -> Style {
        self.resolve_slot("", &slot, selected)
    }
}

fn resolve_raw_style_binding(
    raw: &RawStyleBinding,
    scheme: &ResolvedScheme,
    palette: &BTreeMap<String, Color>,
    source: &str,
    context_desc: &str,
) -> Result<ResolvedCustomStyle> {
    let mut normal = Style::default();
    if let Some(ref fg) = raw.foreground {
        normal = normal.fg(super::color::resolve_color_reference(
            fg,
            scheme,
            palette,
            source,
            &format!("{context_desc}.foreground"),
        )?);
    }
    if let Some(ref bg) = raw.background {
        normal = normal.bg(super::color::resolve_color_reference(
            bg,
            scheme,
            palette,
            source,
            &format!("{context_desc}.background"),
        )?);
    }
    use ratatui::style::Modifier;
    let set_mod = |s: Style, m: Modifier, val: Option<bool>| -> Style {
        match val {
            Some(true) => s.add_modifier(m),
            Some(false) => s.remove_modifier(m),
            None => s,
        }
    };
    normal = set_mod(normal, Modifier::BOLD, raw.bold);
    normal = set_mod(normal, Modifier::ITALIC, raw.italic);
    normal = set_mod(normal, Modifier::UNDERLINED, raw.underline);
    normal = set_mod(normal, Modifier::CROSSED_OUT, raw.strikethrough);
    normal = set_mod(normal, Modifier::DIM, raw.dim);
    normal = set_mod(normal, Modifier::REVERSED, raw.reversed);

    let selected = if let Some(ref sel_raw) = raw.selected {
        let mut sel = normal;
        if let Some(ref bg) = sel_raw.background {
            sel = sel.bg(super::color::resolve_color_reference(
                bg,
                scheme,
                palette,
                source,
                &format!("{context_desc}.selected.background"),
            )?);
        } else {
            sel.bg = None;
        }
        if let Some(ref fg) = sel_raw.foreground {
            sel = sel.fg(super::color::resolve_color_reference(
                fg,
                scheme,
                palette,
                source,
                &format!("{context_desc}.selected.foreground"),
            )?);
        }
        sel = set_mod(sel, Modifier::BOLD, sel_raw.bold);
        sel = set_mod(sel, Modifier::ITALIC, sel_raw.italic);
        sel = set_mod(sel, Modifier::UNDERLINED, sel_raw.underline);
        sel = set_mod(sel, Modifier::CROSSED_OUT, sel_raw.strikethrough);
        sel = set_mod(sel, Modifier::DIM, sel_raw.dim);
        sel = set_mod(sel, Modifier::REVERSED, sel_raw.reversed);
        Some(sel)
    } else {
        None
    };

    Ok(ResolvedCustomStyle { normal, selected })
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
    #[serde(default)]
    pub(super) picker: RawPickerTheme,
    #[serde(default)]
    pub(super) chrome: RawChromeTheme,
    #[serde(default)]
    pub(super) preview: RawPreviewTheme,
    #[serde(default)]
    pub(super) capture: RawCaptureTheme,
    #[serde(default)]
    pub(super) plugins: BTreeMap<String, RawPluginThemeOverride>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPickerTheme {
    #[serde(default)]
    pub(super) text: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) muted: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) selected: Option<RawStyleBinding>,
    #[serde(default, alias = "selected-muted", alias = "selected_muted")]
    pub(super) selected_muted: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) badge: Option<RawStyleBinding>,
    #[serde(default, alias = "badge-selected", alias = "badge_selected")]
    pub(super) badge_selected: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) marker: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) scrollbar: Option<RawStyleBinding>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawChromeTheme {
    #[serde(default)]
    pub(super) divider: Option<RawStyleBinding>,
    #[serde(default, alias = "input-prefix", alias = "input_prefix")]
    pub(super) input_prefix: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) footer: Option<RawStyleBinding>,
    #[serde(default, alias = "footer-key", alias = "footer_key")]
    pub(super) footer_key: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) error: Option<RawStyleBinding>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPreviewTheme {
    #[serde(default)]
    pub(super) text: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) error: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) border: Option<RawStyleBinding>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawCaptureTheme {
    #[serde(default)]
    pub(super) text: Option<RawStyleBinding>,
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

impl RawScheme {
    pub(super) fn role_value(&self, role: super::color::SchemeRole) -> &str {
        use super::color::SchemeRole;
        let val = match role {
            SchemeRole::Primary => self.primary.as_deref(),
            SchemeRole::OnPrimary => self.on_primary.as_deref(),
            SchemeRole::PrimaryContainer => self.primary_container.as_deref(),
            SchemeRole::OnPrimaryContainer => self.on_primary_container.as_deref(),
            SchemeRole::Surface => self.surface.as_deref(),
            SchemeRole::SurfaceContainer => self.surface_container.as_deref(),
            SchemeRole::OnSurface => self.on_surface.as_deref(),
            SchemeRole::OnSurfaceVariant => self.on_surface_variant.as_deref(),
            SchemeRole::Outline => self.outline.as_deref(),
            SchemeRole::Error => self.error.as_deref(),
            SchemeRole::OnError => self.on_error.as_deref(),
        };
        val.unwrap_or_else(|| {
            default_raw_theme()
                .scheme
                .role_value_no_fallback(role)
                .expect("builtin terminal scheme must specify all roles")
        })
    }

    fn role_value_no_fallback(&self, role: super::color::SchemeRole) -> Option<&str> {
        use super::color::SchemeRole;
        match role {
            SchemeRole::Primary => self.primary.as_deref(),
            SchemeRole::OnPrimary => self.on_primary.as_deref(),
            SchemeRole::PrimaryContainer => self.primary_container.as_deref(),
            SchemeRole::OnPrimaryContainer => self.on_primary_container.as_deref(),
            SchemeRole::Surface => self.surface.as_deref(),
            SchemeRole::SurfaceContainer => self.surface_container.as_deref(),
            SchemeRole::OnSurface => self.on_surface.as_deref(),
            SchemeRole::OnSurfaceVariant => self.on_surface_variant.as_deref(),
            SchemeRole::Outline => self.outline.as_deref(),
            SchemeRole::Error => self.error.as_deref(),
            SchemeRole::OnError => self.on_error.as_deref(),
        }
    }
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
    #[serde(default)]
    pub(super) dim: Option<bool>,
    #[serde(default)]
    pub(super) reversed: Option<bool>,
}

impl RawBinding {
    pub(super) fn to_style_binding(&self) -> RawStyleBinding {
        RawStyleBinding {
            foreground: self.foreground.clone(),
            background: self.background.clone(),
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strikethrough: self.strikethrough,
            dim: self.dim,
            reversed: self.reversed,
            selected: None,
        }
    }
}
