mod binding;
mod color;
mod load;
mod model;

pub(crate) use load::{ThemeLoadOptions, cli_named_theme, load};
#[allow(unused_imports)]
pub(crate) use model::{
    CaptureTheme, ChromeTheme, PickerTheme, PreviewTheme, ResolvedTheme, Theme, ThemeRef,
};

#[cfg(test)]
use binding::ThemeBinding;
#[cfg(test)]
use color::{ResolvedScheme, SchemeRole};
#[cfg(test)]
use model::RawTheme;
#[cfg(test)]
use ratatui::style::{Color, Modifier, Style};
#[cfg(test)]
use std::collections::BTreeMap;
#[cfg(test)]
use std::fs;

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
    fn theme_tables_reject_unknown_fields() {
        assert!(toml::from_str::<RawTheme>("[scheme]\nprimaryy = \"palette:cyan\"\n").is_err());

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
