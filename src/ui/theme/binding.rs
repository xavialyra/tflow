use super::color::{ResolvedScheme, SchemeRole};
use super::model::RawBinding;
use anyhow::{Result, bail};
use ratatui::style::{Color, Modifier, Style};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ThemeBinding {
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
    pub(super) const ALL: &'static [Self] = &[
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

    pub(super) fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|binding| binding.name() == value)
    }

    pub(super) fn all_names() -> String {
        Self::ALL
            .iter()
            .map(|binding| binding.name())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(super) fn name(self) -> &'static str {
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

pub(super) fn validate_bindings(
    bindings: &BTreeMap<String, RawBinding>,
    source: &str,
) -> Result<()> {
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

pub(super) fn resolve_binding(
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

fn set_modifier(style: Style, modifier: Modifier, enabled: Option<bool>) -> Style {
    match enabled {
        Some(true) => style.add_modifier(modifier),
        Some(false) => style.remove_modifier(modifier),
        None => style,
    }
}
