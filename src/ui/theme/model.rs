use super::color::{ResolvedScheme, resolve_scheme};
use anyhow::Result;
use ratatui::style::Style;
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
pub(super) struct RawWorkflowThemeOverride {
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
    pub(crate) chrome: ChromeTheme,
    pub(crate) picker: PickerTheme,
    pub(crate) capture: CaptureTheme,
    pub(crate) form: FormTheme,
    pub(super) scheme: ResolvedScheme,
    pub(super) raw_theme_overrides: Arc<BTreeMap<(String, String), RawStyleBinding>>,
    pub(super) custom_styles: Arc<BTreeMap<(String, String), ResolvedCustomStyle>>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ChromeTheme {
    pub(crate) divider: Style,
    pub(crate) border: Style,
    pub(crate) footer: Style,
    pub(crate) footer_title: Style,
    pub(crate) footer_status: Style,
    pub(crate) footer_key: Style,
    pub(crate) error: Style,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PickerTheme {
    pub(crate) text: Style,
    pub(crate) muted: Style,
    pub(crate) placeholder: Style,
    pub(crate) input_prefix: Style,
    pub(crate) cursor: Style,
    pub(crate) selected: Style,
    pub(crate) selected_muted: Style,
    pub(crate) badge: Style,
    pub(crate) badge_selected: Style,
    pub(crate) marker: Style,
    pub(crate) scrollbar: Style,
    pub(crate) preview: PreviewTheme,
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

#[derive(Debug, Clone, Copy)]
pub(crate) struct FormTheme {
    pub(crate) label: Style,
    pub(crate) input: Style,
    pub(crate) focused: Style,
    pub(crate) border: Style,
    pub(crate) focused_border: Style,
    pub(crate) error: Style,
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
        // Merge raw values first: inherited bindings must see the user's scheme.
        let mut raw_scheme = default_raw.scheme.clone();
        raw_scheme.extend(raw.scheme.clone());
        let scheme = resolve_scheme(&raw_scheme, source)?;

        let resolve_component = |configured: Option<&RawStyleBinding>,
                                 default: &RawStyleBinding,
                                 desc: &str|
         -> Result<ResolvedCustomStyle> {
            let mut merged = default.clone();
            if let Some(conf) = configured {
                merged = conf.merge_with(&merged);
            }
            resolve_raw_style_binding(&merged, &scheme, source, desc)
        };

        let picker_text = resolve_component(
            raw.picker.text.as_ref(),
            default_raw.picker.text.as_ref().unwrap(),
            "picker.text",
        )?
        .normal;

        let picker_muted = resolve_component(
            raw.picker.muted.as_ref(),
            default_raw.picker.muted.as_ref().unwrap(),
            "picker.muted",
        )?
        .normal;

        let picker_placeholder = resolve_component(
            raw.picker.placeholder.as_ref(),
            default_raw.picker.placeholder.as_ref().unwrap(),
            "picker.placeholder",
        )?
        .normal;

        let picker_selected = resolve_component(
            raw.picker.selected.as_ref(),
            default_raw.picker.selected.as_ref().unwrap(),
            "picker.selected",
        )?
        .normal;

        let picker_selected_muted = resolve_component(
            raw.picker.selected_muted.as_ref(),
            default_raw.picker.selected_muted.as_ref().unwrap(),
            "picker.selected_muted",
        )?
        .normal;

        let picker_marker = resolve_component(
            raw.picker.marker.as_ref(),
            default_raw.picker.marker.as_ref().unwrap(),
            "picker.marker",
        )?
        .normal;

        let picker_scrollbar = resolve_component(
            raw.picker.scrollbar.as_ref(),
            default_raw.picker.scrollbar.as_ref().unwrap(),
            "picker.scrollbar",
        )?
        .normal;

        let picker_input_prefix = resolve_component(
            raw.picker.input_prefix.as_ref(),
            default_raw.picker.input_prefix.as_ref().unwrap(),
            "picker.input_prefix",
        )?
        .normal;

        let picker_cursor = resolve_component(
            raw.picker.cursor.as_ref(),
            default_raw.picker.cursor.as_ref().unwrap(),
            "picker.cursor",
        )?
        .normal;

        let resolved_badge = resolve_component(
            raw.picker.badge.as_ref(),
            default_raw.picker.badge.as_ref().unwrap(),
            "picker.badge",
        )?;
        let picker_badge = resolved_badge.normal;
        let picker_badge_selected = if let Some(sel) = resolved_badge.selected {
            let mut s = sel;
            if s.bg.is_none()
                && let Some(bg) = picker_selected.bg
            {
                s = s.bg(bg);
            }
            s
        } else {
            let mut s = picker_badge;
            if let Some(bg) = picker_selected.bg {
                s = s.bg(bg);
            }
            s
        };

        let chrome_text = resolve_component(
            raw.chrome.text.as_ref(),
            default_raw.chrome.text.as_ref().unwrap(),
            "chrome.text",
        )?
        .normal;

        let chrome_divider = resolve_component(
            raw.chrome.divider.as_ref(),
            default_raw.chrome.divider.as_ref().unwrap(),
            "chrome.divider",
        )?
        .normal;

        let chrome_border = resolve_component(
            raw.chrome.border.as_ref(),
            default_raw.chrome.border.as_ref().unwrap(),
            "chrome.border",
        )?
        .normal;

        let chrome_footer = resolve_component(
            raw.chrome.footer.as_ref(),
            default_raw.chrome.footer.as_ref().unwrap(),
            "chrome.footer",
        )?
        .normal;

        let chrome_footer_title = resolve_component(
            raw.chrome.footer_title.as_ref(),
            default_raw.chrome.footer_title.as_ref().unwrap(),
            "chrome.footer_title",
        )?
        .normal;

        let chrome_footer_status = resolve_component(
            raw.chrome.footer_status.as_ref(),
            default_raw.chrome.footer_status.as_ref().unwrap(),
            "chrome.footer_status",
        )?
        .normal;

        let chrome_footer_key = resolve_component(
            raw.chrome.footer_key.as_ref(),
            default_raw.chrome.footer_key.as_ref().unwrap(),
            "chrome.footer_key",
        )?
        .normal;

        let chrome_error = resolve_component(
            raw.chrome.error.as_ref(),
            default_raw.chrome.error.as_ref().unwrap(),
            "chrome.error",
        )?
        .normal;

        let preview_text = resolve_component(
            raw.picker.preview.text.as_ref(),
            default_raw.picker.preview.text.as_ref().unwrap(),
            "picker.preview.text",
        )?
        .normal;

        let preview_error = resolve_component(
            raw.picker.preview.error.as_ref(),
            default_raw.picker.preview.error.as_ref().unwrap(),
            "picker.preview.error",
        )?
        .normal;

        let preview_border = resolve_component(
            raw.picker.preview.border.as_ref(),
            default_raw.picker.preview.border.as_ref().unwrap(),
            "picker.preview.border",
        )?
        .normal;

        let capture_text = resolve_component(
            raw.capture.text.as_ref(),
            default_raw.capture.text.as_ref().unwrap(),
            "capture.text",
        )?
        .normal;

        let mut raw_theme_overrides = BTreeMap::new();
        let mut custom_styles = BTreeMap::new();
        for (workflow_id, workflow_override) in &raw.workflows {
            for (slot_name, style_binding) in &workflow_override.styles {
                let key = (workflow_id.clone(), slot_name.clone());
                let context = format!("workflows.{workflow_id}.styles.{slot_name}");
                let resolved = resolve_raw_style_binding(style_binding, &scheme, source, &context)?;
                custom_styles.insert(key.clone(), resolved);
                raw_theme_overrides.insert(key, style_binding.clone());
            }
        }

        Ok(Self {
            text: chrome_text,
            chrome: ChromeTheme {
                divider: chrome_divider,
                border: chrome_border,
                footer: chrome_footer,
                footer_title: chrome_footer_title,
                footer_status: chrome_footer_status,
                footer_key: chrome_footer_key,
                error: chrome_error,
            },
            picker: PickerTheme {
                text: picker_text,
                muted: picker_muted,
                placeholder: picker_placeholder,
                input_prefix: picker_input_prefix,
                cursor: picker_cursor,
                selected: picker_selected,
                selected_muted: picker_selected_muted,
                badge: picker_badge,
                badge_selected: picker_badge_selected,
                marker: picker_marker,
                scrollbar: picker_scrollbar,
                preview: PreviewTheme {
                    text: preview_text,
                    error: preview_error,
                    border: preview_border,
                },
            },
            capture: CaptureTheme { text: capture_text },
            form: FormTheme {
                label: resolve_component(
                    raw.form.label.as_ref(),
                    default_raw.form.label.as_ref().unwrap(),
                    "form.label",
                )?
                .normal,
                input: resolve_component(
                    raw.form.input.as_ref(),
                    default_raw.form.input.as_ref().unwrap(),
                    "form.input",
                )?
                .normal,
                focused: resolve_component(
                    raw.form.focused.as_ref(),
                    default_raw.form.focused.as_ref().unwrap(),
                    "form.focused",
                )?
                .normal,
                border: resolve_component(
                    raw.form.border.as_ref(),
                    default_raw.form.border.as_ref().unwrap(),
                    "form.border",
                )?
                .normal,
                focused_border: resolve_component(
                    raw.form.focused_border.as_ref(),
                    default_raw.form.focused_border.as_ref().unwrap(),
                    "form.focused_border",
                )?
                .normal,
                error: resolve_component(
                    raw.form.error.as_ref(),
                    default_raw.form.error.as_ref().unwrap(),
                    "form.error",
                )?
                .normal,
            },
            scheme,
            raw_theme_overrides: Arc::new(raw_theme_overrides),
            custom_styles: Arc::new(custom_styles),
        })
    }

    #[cfg(test)]
    pub(crate) fn register_workflow_defaults(
        &mut self,
        workflow_id: &str,
        styles: &BTreeMap<String, RawStyleBinding>,
    ) -> Result<()> {
        let mut custom_styles = (*self.custom_styles).clone();
        for (slot_name, default_binding) in styles {
            let key = (workflow_id.to_string(), slot_name.clone());
            let resolved_style = if let Some(theme_override) = self.raw_theme_overrides.get(&key) {
                let merged = theme_override.merge_with(default_binding);
                resolve_raw_style_binding(
                    &merged,
                    &self.scheme,
                    &format!("theme override [workflows.{workflow_id}.styles.{slot_name}]"),
                    &format!("workflows.{workflow_id}.styles.{slot_name}"),
                )?
            } else {
                resolve_raw_style_binding(
                    default_binding,
                    &self.scheme,
                    &format!("workflow {:?} [styles.{slot_name}]", workflow_id),
                    &format!("styles.{slot_name}"),
                )?
            };
            custom_styles.insert(key, resolved_style);
        }
        self.custom_styles = Arc::new(custom_styles);
        Ok(())
    }

    pub(crate) fn register_all_workflow_defaults_with_overrides(
        &mut self,
        workflows: &BTreeMap<String, crate::workflow::config::WorkflowMetadata>,
        suite_styles: &BTreeMap<String, BTreeMap<String, RawStyleBinding>>,
        settings_styles: &BTreeMap<String, BTreeMap<String, RawStyleBinding>>,
    ) -> Result<()> {
        use std::collections::BTreeSet;
        let mut all_workflow_ids: BTreeSet<String> = workflows.keys().cloned().collect();
        all_workflow_ids.extend(suite_styles.keys().cloned());
        all_workflow_ids.extend(settings_styles.keys().cloned());

        let mut custom_styles = (*self.custom_styles).clone();
        for workflow_id in all_workflow_ids {
            let empty_styles = BTreeMap::new();
            let wf_styles = workflows
                .get(&workflow_id)
                .map(|m| &m.styles)
                .unwrap_or(&empty_styles);
            let suite_wf_styles = suite_styles.get(&workflow_id);
            let settings_wf_styles = settings_styles.get(&workflow_id);

            let mut all_slots = BTreeSet::new();
            all_slots.extend(wf_styles.keys().cloned());
            if let Some(s) = suite_wf_styles {
                all_slots.extend(s.keys().cloned());
            }
            if let Some(s) = settings_wf_styles {
                all_slots.extend(s.keys().cloned());
            }

            for slot_name in all_slots {
                let default_binding = wf_styles.get(&slot_name).cloned().unwrap_or_default();
                let mut merged =
                    if let Some(suite_override) = suite_wf_styles.and_then(|s| s.get(&slot_name)) {
                        suite_override.merge_with(&default_binding)
                    } else {
                        default_binding
                    };
                if let Some(settings_override) = settings_wf_styles.and_then(|s| s.get(&slot_name))
                {
                    merged = settings_override.merge_with(&merged);
                }
                let key = (workflow_id.clone(), slot_name.clone());
                if let Some(theme_override) = self.raw_theme_overrides.get(&key) {
                    merged = theme_override.merge_with(&merged);
                }

                let resolved = resolve_raw_style_binding(
                    &merged,
                    &self.scheme,
                    &format!("style slot [{workflow_id}.{slot_name}]"),
                    &format!("styles.{workflow_id}.{slot_name}"),
                )?;
                custom_styles.insert(key, resolved);
            }
        }
        self.custom_styles = Arc::new(custom_styles);
        Ok(())
    }

    pub(crate) fn resolve_slot(
        &self,
        workflow_id: &str,
        slot: &crate::engine::SlotToken,
        selected: bool,
    ) -> Style {
        if let Some(custom) = self
            .custom_styles
            .get(&(workflow_id.to_string(), slot.as_str().to_string()))
            .or_else(|| {
                if !workflow_id.is_empty() {
                    self.custom_styles
                        .get(&("".to_string(), slot.as_str().to_string()))
                } else {
                    None
                }
            })
        {
            if selected {
                if let Some(sel) = custom.selected {
                    let mut s = sel;
                    if s.bg.is_none()
                        && let Some(bg) = self.picker.selected.bg
                    {
                        s = s.bg(bg);
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

    fn resolve_builtin_slot(&self, slot: &crate::engine::SlotToken, selected: bool) -> Style {
        use crate::engine::SlotToken;
        use ratatui::style::{Color, Modifier};

        let success_color = self.scheme.get("success").copied().unwrap_or(Color::Green);
        let warning_color = self.scheme.get("warning").copied().unwrap_or(Color::Yellow);
        let error_color = self.scheme.get("error").copied().unwrap_or(Color::Red);

        if selected {
            let base = self.picker.selected;
            match slot {
                SlotToken::Primary => base,
                SlotToken::Secondary => self.picker.selected_muted,
                SlotToken::Muted => self.picker.selected_muted.add_modifier(Modifier::DIM),
                SlotToken::Accent => base.add_modifier(Modifier::BOLD),
                SlotToken::Badge => self.picker.badge_selected,
                SlotToken::Success => base.fg(success_color).add_modifier(Modifier::BOLD),
                SlotToken::Warning => base.fg(warning_color).add_modifier(Modifier::BOLD),
                SlotToken::Error => base.fg(error_color).add_modifier(Modifier::BOLD),
                SlotToken::Custom(_) => base,
            }
        } else {
            match slot {
                SlotToken::Primary => self.picker.text,
                SlotToken::Secondary => self.picker.muted,
                SlotToken::Muted => self.picker.muted.add_modifier(Modifier::DIM),
                SlotToken::Accent => self.picker.text.add_modifier(Modifier::BOLD),
                SlotToken::Badge => self.picker.badge,
                SlotToken::Success => self.picker.text.fg(success_color),
                SlotToken::Warning => self.picker.text.fg(warning_color),
                SlotToken::Error => self.picker.text.fg(error_color),
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
    source: &str,
    context_desc: &str,
) -> Result<ResolvedCustomStyle> {
    let mut normal = Style::default();
    if let Some(ref fg) = raw.foreground {
        normal = normal.fg(super::color::resolve_color_reference(
            fg,
            scheme,
            source,
            &format!("{context_desc}.foreground"),
        )?);
    }
    if let Some(ref bg) = raw.background {
        normal = normal.bg(super::color::resolve_color_reference(
            bg,
            scheme,
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
    pub(super) scheme: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) picker: RawPickerTheme,
    #[serde(default)]
    pub(super) chrome: RawChromeTheme,
    #[serde(default)]
    pub(super) capture: RawCaptureTheme,
    #[serde(default)]
    pub(super) form: RawFormTheme,
    #[serde(default)]
    pub(super) workflows: BTreeMap<String, RawWorkflowThemeOverride>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPickerTheme {
    #[serde(default)]
    pub(super) text: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) muted: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) placeholder: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) input_prefix: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) cursor: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) selected: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) selected_muted: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) badge: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) marker: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) scrollbar: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) preview: RawPreviewTheme,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawChromeTheme {
    #[serde(default)]
    pub(super) text: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) divider: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) border: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) footer: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) footer_title: Option<RawStyleBinding>,
    #[serde(default)]
    pub(super) footer_status: Option<RawStyleBinding>,
    #[serde(default)]
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
pub(super) struct RawFormTheme {
    pub(super) label: Option<RawStyleBinding>,
    pub(super) input: Option<RawStyleBinding>,
    pub(super) focused: Option<RawStyleBinding>,
    pub(super) border: Option<RawStyleBinding>,
    pub(super) focused_border: Option<RawStyleBinding>,
    pub(super) error: Option<RawStyleBinding>,
}

#[cfg(test)]
mod form_tests {
    use super::*;

    #[test]
    fn form_theme_inherits_scheme_and_resolves_component_overrides() {
        let raw: RawTheme = toml::from_str(
            r##"
            [scheme]
            accent = "#123456"
            [form.focused]
            background = "#654321"
            [form.border]
            foreground = "#aabbcc"
            [form.focused_border]
            foreground = "#ddeeff"
        "##,
        )
        .unwrap();
        let theme = ResolvedTheme::from_raw(&raw, "form theme").unwrap();
        assert_eq!(
            theme.form.label.fg,
            Some(ratatui::style::Color::Rgb(0x12, 0x34, 0x56))
        );
        assert_eq!(
            theme.form.focused.bg,
            Some(ratatui::style::Color::Rgb(0x65, 0x43, 0x21))
        );
        assert_eq!(
            theme.form.border.fg,
            Some(ratatui::style::Color::Rgb(0xaa, 0xbb, 0xcc))
        );
        assert_eq!(
            theme.form.focused_border.fg,
            Some(ratatui::style::Color::Rgb(0xdd, 0xee, 0xff))
        );
        assert_eq!(theme.form.input.fg, ResolvedTheme::terminal().form.input.fg);
    }

    #[test]
    fn resolves_semantic_state_slots() {
        use crate::engine::SlotToken;
        let theme = ResolvedTheme::terminal();
        let success_style = theme.resolve_slot("", &SlotToken::Success, false);
        assert_eq!(success_style.fg, Some(ratatui::style::Color::Green));

        let warning_style = theme.resolve_slot("", &SlotToken::Warning, false);
        assert_eq!(warning_style.fg, Some(ratatui::style::Color::Yellow));

        let error_style = theme.resolve_slot("", &SlotToken::Error, false);
        assert_eq!(error_style.fg, Some(ratatui::style::Color::Red));

        let sel_success = theme.resolve_slot("", &SlotToken::Success, true);
        assert_eq!(sel_success.fg, Some(ratatui::style::Color::Green));
        assert!(
            sel_success
                .sub_modifier
                .contains(ratatui::style::Modifier::BOLD)
                || sel_success
                    .add_modifier
                    .contains(ratatui::style::Modifier::BOLD)
        );
    }
}
