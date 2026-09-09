mod color;
mod load;
mod model;

pub(crate) use load::{ThemeLoadOptions, cli_named_theme, load};
pub(crate) use model::{RawStyleBinding, ResolvedTheme, Theme};

#[cfg(test)]
use color::{ResolvedScheme, SchemeRole};
#[cfg(test)]
use model::RawTheme;
#[cfg(test)]
use ratatui::style::{Color, Modifier};
#[cfg(test)]
use std::collections::BTreeMap;
#[cfg(test)]
use std::fs;

#[cfg(test)]
mod tests {
    use super::*;
    use model::ThemeRef;
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
        let path = env::temp_dir().join(format!("tlaunch-theme-{}-{suffix}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
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
    fn rejects_noncanonical_theme_field_aliases() {
        for (section, field) in [
            ("picker", "input-prefix"),
            ("picker", "prefix"),
            ("picker", "selected-muted"),
            ("picker", "badge-selected"),
            ("chrome", "muted-text"),
            ("chrome", "footer-title"),
            ("chrome", "footer-status"),
            ("chrome", "footer-key"),
        ] {
            let source = format!("[{section}]\n{field} = {{ foreground = \"ansi:red\" }}\n");
            let error = toml::from_str::<RawTheme>(&source).expect_err(&source);
            assert!(error.to_string().contains("unknown field"), "{error}");
        }
    }

    #[test]
    fn terminal_theme_contains_default_bindings() {
        let theme = ResolvedTheme::terminal();

        assert_eq!(theme.text.fg, Some(Color::Reset));
        assert_eq!(theme.text.bg, Some(Color::Reset));
        assert_eq!(theme.chrome.text.fg, Some(Color::Reset));
        assert_eq!(theme.chrome.muted_text.fg, Some(Color::Reset));
        assert_eq!(theme.chrome.border.fg, Some(Color::Reset));
        assert_eq!(theme.chrome.footer_title.fg, Some(Color::Cyan));
        assert!(
            theme
                .chrome
                .footer_title
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(theme.picker.input_prefix.fg, Some(Color::Cyan));
        assert_eq!(theme.picker.input_prefix.bg, Some(Color::Reset));
        assert!(
            theme
                .picker
                .input_prefix
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(theme.picker.selected.fg, Some(Color::Cyan));
        assert_eq!(theme.picker.selected.bg, Some(Color::Reset));
        assert_eq!(theme.picker.marker.fg, Some(Color::Cyan));
        assert_eq!(theme.chrome.error.fg, Some(Color::White));
        assert_eq!(theme.chrome.error.bg, Some(Color::Red));
        assert_eq!(theme.picker.preview.text.fg, Some(Color::Reset));
        assert_eq!(theme.picker.preview.error.fg, Some(Color::White));
        assert_eq!(theme.picker.preview.error.bg, Some(Color::Red));
        assert_eq!(theme.picker.preview.border.fg, Some(Color::Reset));
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

            [picker.selected]
            foreground = "scheme:on-primary-container"
            background = "scheme:primary-container"
            bold = true
            italic = true
            underline = true
            strikethrough = true

            [chrome.divider]
            foreground = "scheme:outline"

            [chrome.error]
            foreground = "scheme:on-error"
            background = "scheme:error"

            [picker.preview.text]
            foreground = "scheme:on-surface-variant"

            [picker.preview.error]
            foreground = "scheme:on-error"
            background = "scheme:error"

            [capture.text]
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
        assert_eq!(theme.picker.preview.text.fg, Some(Color::Rgb(16, 32, 48)));
        assert_eq!(theme.picker.preview.error.fg, Some(Color::White));
        assert_eq!(theme.picker.preview.error.bg, Some(Color::Red));
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

            [chrome.text]
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

            [picker.marker]
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
        assert!(toml::from_str::<RawTheme>("[chrome]\nunknown = {}\n").is_err());
        assert!(toml::from_str::<RawTheme>("[picker]\nunknown = {}\n").is_err());
    }

    #[test]
    fn explicit_refs_and_style_fields_keep_their_names_in_errors() {
        let ref_error =
            toml::from_str::<ThemeRef>("source = \"mystery\"\nname = \"x\"\n").unwrap_err();
        assert!(ref_error.to_string().contains("mystery"));

        let field_error =
            toml::from_str::<RawTheme>("[chrome.text]\nforegroundd = \"scheme:primary\"\n")
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

    #[test]
    fn raw_colors_in_styles_are_rejected() {
        let raw: RawTheme = toml::from_str(
            r##"
            [workflows.git.styles.branch]
            foreground = "#ff0000"
            "##,
        )
        .unwrap();
        let error = ResolvedTheme::from_raw(&raw, "test theme").unwrap_err();
        assert!(error.to_string().contains(
            "must reference a scheme role (scheme:ROLE) or palette color (palette:NAME)"
        ));

        let raw2: RawTheme = toml::from_str(
            r#"
            [workflows.git.styles.branch]
            foreground = "red"
            "#,
        )
        .unwrap();
        let error2 = ResolvedTheme::from_raw(&raw2, "test theme").unwrap_err();
        assert!(error2.to_string().contains(
            "must reference a scheme role (scheme:ROLE) or palette color (palette:NAME)"
        ));
    }

    #[test]
    fn workflow_styles_and_theme_overrides_with_selected_state() {
        use crate::engine::SlotToken;

        let raw_theme: RawTheme = toml::from_str(
            r#"
            [palette]
            accent-blue = "blue"
            selection-bg = "green"
            highlight-gold = "yellow"

            [scheme]
            primary = "palette:accent-blue"
            primary-container = "palette:selection-bg"

            [workflows.git.styles.branch]
            bold = false

            [workflows.git.styles.branch.selected]
            foreground = "palette:highlight-gold"

            [workflows.git.styles.hash]
            foreground = "palette:highlight-gold"
            "#,
        )
        .unwrap();
        let mut theme = ResolvedTheme::from_raw(&raw_theme, "tokyonight").unwrap();

        let mut workflow_styles = BTreeMap::new();
        workflow_styles.insert(
            "branch".to_string(),
            RawStyleBinding {
                foreground: Some("scheme:primary".to_string()),
                bold: Some(true),
                ..Default::default()
            },
        );
        workflow_styles.insert(
            "tag".to_string(),
            RawStyleBinding {
                foreground: Some("palette:accent-blue".to_string()),
                reversed: Some(true),
                ..Default::default()
            },
        );
        theme
            .register_workflow_defaults("git", &workflow_styles)
            .unwrap();

        // 1. Normal slot: branch should have foreground = Blue (scheme:primary), and bold = false (overridden by theme)
        let branch_normal = theme.resolve_slot("git", &SlotToken::from("branch"), false);
        assert_eq!(branch_normal.fg, Some(Color::Blue));
        assert!(!branch_normal.add_modifier.contains(Modifier::BOLD));

        // 2. Selected slot: branch selected state was overridden with highlight-gold (Yellow).
        // Background should automatically lock to picker.selected.bg (Green).
        let branch_selected = theme.resolve_slot("git", &SlotToken::from("branch"), true);
        assert_eq!(branch_selected.fg, Some(Color::Yellow));
        assert_eq!(branch_selected.bg, Some(Color::Green));

        // 3. Tag slot: not overridden by theme, default has reversed = true
        let tag_normal = theme.resolve_slot("git", &SlotToken::from("tag"), false);
        assert_eq!(tag_normal.fg, Some(Color::Blue));
        assert!(tag_normal.add_modifier.contains(Modifier::REVERSED));

        // Tag selected slot: no explicit selected block, so background locks to picker.selected.bg
        let tag_selected = theme.resolve_slot("git", &SlotToken::from("tag"), true);
        assert_eq!(tag_selected.fg, Some(Color::Blue));
        assert_eq!(tag_selected.bg, Some(Color::Green));
        assert!(tag_selected.add_modifier.contains(Modifier::REVERSED));

        // 4. Hash slot: declared solely in theme overrides (no workflow default)
        let hash_normal = theme.resolve_slot("git", &SlotToken::from("hash"), false);
        assert_eq!(hash_normal.fg, Some(Color::Yellow));

        // 5. Builtin slots fallback works
        let primary_normal = theme.resolve_slot("git", &SlotToken::Primary, false);
        assert_eq!(primary_normal, theme.picker.text);
    }

    #[test]
    fn structured_picker_badge_and_chrome_table_syntax() {
        use crate::engine::SlotToken;

        let raw_theme: RawTheme = toml::from_str(
            r#"
            [palette]
            brand = "blue"
            badge-fg = "white"
            badge-bg = "red"
            sel-bg = "green"
            sel-fg = "yellow"

            [scheme]
            primary = "palette:brand"
            primary-container = "palette:sel-bg"

            # Structured TOML table hierarchy
            [picker.text]
            foreground = "scheme:primary"

            [picker.badge]
            foreground = "palette:badge-fg"
            background = "palette:badge-bg"
            bold = true

            [picker.badge.selected]
            foreground = "palette:sel-fg"
            background = "palette:sel-bg"
            underline = true

            [chrome.divider]
            foreground = "palette:brand"
            "#,
        )
        .unwrap();
        let theme = ResolvedTheme::from_raw(&raw_theme, "structured").unwrap();

        // Check picker text
        assert_eq!(theme.picker.text.fg, Some(Color::Blue));

        // Check chrome divider
        assert_eq!(theme.chrome.divider.fg, Some(Color::Blue));

        // Check badge is terminal style (bold = true, NO REVERSED)
        assert_eq!(theme.picker.badge.fg, Some(Color::White));
        assert_eq!(theme.picker.badge.bg, Some(Color::Red));
        assert!(theme.picker.badge.add_modifier.contains(Modifier::BOLD));
        assert!(!theme.picker.badge.add_modifier.contains(Modifier::REVERSED));

        // Check badge selected is terminal style (underline = true, NO REVERSED)
        assert_eq!(theme.picker.badge_selected.fg, Some(Color::Yellow));
        assert_eq!(theme.picker.badge_selected.bg, Some(Color::Green));
        assert!(
            theme
                .picker
                .badge_selected
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
        assert!(
            !theme
                .picker
                .badge_selected
                .add_modifier
                .contains(Modifier::REVERSED)
        );

        // Built-in slot resolution delivers terminal styles directly
        let unselected = theme.resolve_slot("", &SlotToken::Badge, false);
        assert_eq!(unselected, theme.picker.badge);

        let selected = theme.resolve_slot("", &SlotToken::Badge, true);
        assert_eq!(selected, theme.picker.badge_selected);
    }

    #[test]
    fn default_theme_badge_is_not_reversed() {
        let theme = ResolvedTheme::terminal();
        assert!(!theme.picker.badge.add_modifier.contains(Modifier::REVERSED));
        assert!(
            !theme
                .picker
                .badge_selected
                .add_modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn picker_input_prefix_and_chrome_footer_title_resolve() {
        let raw_theme: RawTheme = toml::from_str(
            r#"
            [palette]
            accent = "yellow"
            border_col = "green"
            title_col = "magenta"
            status_col = "gray"

            [scheme]
            primary = "palette:accent"

            [picker.input_prefix]
            foreground = "palette:accent"
            bold = true

            [chrome.border]
            foreground = "palette:border_col"

            [chrome.footer_title]
            foreground = "palette:title_col"
            bold = true

            [chrome.footer_status]
            foreground = "palette:status_col"
            dim = true
            "#,
        )
        .unwrap();
        let theme = ResolvedTheme::from_raw(&raw_theme, "picker_and_chrome_ext").unwrap();

        assert_eq!(theme.picker.input_prefix.fg, Some(Color::Yellow));
        assert!(
            theme
                .picker
                .input_prefix
                .add_modifier
                .contains(Modifier::BOLD)
        );

        assert_eq!(theme.chrome.border.fg, Some(Color::Green));
        assert_eq!(theme.chrome.footer_title.fg, Some(Color::Magenta));
        assert!(
            theme
                .chrome
                .footer_title
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(theme.chrome.footer_status.fg, Some(Color::Gray));
        assert!(
            theme
                .chrome
                .footer_status
                .add_modifier
                .contains(Modifier::DIM)
        );
    }
}
