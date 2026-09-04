use super::model::RawBinding;
use anyhow::{Result, bail};
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
    PickerBadge,
    PickerBadgeSelected,
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
        Self::PickerBadge,
        Self::PickerBadgeSelected,
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
            Self::PickerBadge => "picker-badge",
            Self::PickerBadgeSelected => "picker-badge-selected",
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
