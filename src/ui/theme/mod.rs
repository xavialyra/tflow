mod color;
mod load;
mod model;

pub(crate) use load::{ThemeLoadOptions, cli_named_theme, load};
pub(crate) use model::{RawStyleBinding, ResolvedTheme, Theme};

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
    fn dynamic_scheme_names_and_merged_references() {
        let raw: RawTheme = toml::from_str(
            r##"
            [scheme]
            "brand.link" = "#12aBcD"
            accent = "#345678"
            selection = "ansi:green"

            [chrome.footer_title]
            bold = false

            [picker.text]
            foreground = "scheme:brand.link"

            [capture.text]
            foreground = "scheme:on-selection"
            background = "#abcdef"
            "##,
        )
        .unwrap();
        let theme = ResolvedTheme::from_raw(&raw, "dynamic theme").unwrap();
        assert_eq!(theme.picker.text.fg, Some(Color::Rgb(18, 171, 205)));
        assert_eq!(theme.picker.text.bg, Some(Color::Reset));
        // Inherited component references resolve against the merged scheme.
        assert_eq!(theme.picker.marker.fg, Some(Color::Rgb(52, 86, 120)));
        assert_eq!(theme.chrome.footer_title.fg, theme.picker.marker.fg);
        assert!(
            theme
                .chrome
                .footer_title
                .sub_modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(theme.picker.selected.bg, Some(Color::Green));
        // User bindings can refer to omitted, inherited scheme names too.
        assert_eq!(
            theme.capture.text.fg,
            theme.scheme.get("on-selection").copied()
        );
        assert_eq!(theme.capture.text.bg, Some(Color::Rgb(171, 205, 239)));
    }

    #[test]
    fn color_values_trim_outer_whitespace_without_changing_scheme_keys() {
        let raw: RawTheme = toml::from_str(
            r##"
            [scheme]
            accent = " \t#123456\n "
            background = " ansi:white "
            " accent " = " ansi:red "
            "brand link" = " ansi:green "

            [picker.text]
            foreground = " \tscheme:accent\n "

            [picker.badge.selected]
            background = " ansi:reset "

            [chrome.footer_title]
            foreground = " scheme:brand link "

            [capture.text]
            foreground = " #abcdef "
            background = " ansi:black "
            "##,
        )
        .unwrap();
        let theme = ResolvedTheme::from_raw(&raw, "whitespace theme").unwrap();
        assert_eq!(theme.scheme.get(" accent "), Some(&Color::Red));
        assert_eq!(theme.scheme.get("accent"), Some(&Color::Rgb(18, 52, 86)));
        assert_eq!(theme.picker.text.fg, Some(Color::Rgb(18, 52, 86)));
        assert_eq!(theme.picker.text.bg, Some(Color::White));
        assert_eq!(theme.chrome.footer_title.fg, Some(Color::Green));
        assert_eq!(theme.picker.badge_selected.bg, Some(Color::Reset));
        assert_eq!(theme.capture.text.fg, Some(Color::Rgb(171, 205, 239)));
        assert_eq!(theme.capture.text.bg, Some(Color::Black));
    }

    #[test]
    fn selected_component_fields_inherit_and_false_and_reset_override() {
        let raw: RawTheme = toml::from_str(
            r#"
            [scheme]
            selection = "ansi:green"

            [picker.selected]
            foreground = "ansi:reset"
            bold = false

            [picker.badge]
            italic = true

            [picker.badge.selected]
            bold = false
            background = "ansi:reset"
            "#,
        )
        .unwrap();
        let theme = ResolvedTheme::from_raw(&raw, "partial theme").unwrap();
        assert_eq!(theme.picker.selected.fg, Some(Color::Reset));
        assert_eq!(theme.picker.selected.bg, Some(Color::Green));
        assert!(theme.picker.selected.sub_modifier.contains(Modifier::BOLD));
        let selected = theme.picker.badge_selected;
        assert_eq!(selected.bg, Some(Color::Reset));
        assert!(selected.add_modifier.contains(Modifier::ITALIC));
        assert!(selected.sub_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn workflow_selected_defaults_merge_before_resolving() {
        use crate::engine::SlotToken;
        let raw: RawTheme = toml::from_str(
            r##"
            [scheme]
            branch = "#123456"

            [workflows.git.styles.branch]
            bold = false

            [workflows.git.styles.branch.selected]
            italic = false
            background = "ansi:reset"
            "##,
        )
        .unwrap();
        let defaults: BTreeMap<String, RawStyleBinding> = toml::from_str(
            r#"
            [branch]
            foreground = "scheme:branch"
            bold = true
            underline = true

            [branch.selected]
            foreground = "scheme:accent"
            italic = true
            strikethrough = true
            "#,
        )
        .unwrap();
        let mut theme = ResolvedTheme::from_raw(&raw, "workflow theme").unwrap();
        theme.register_workflow_defaults("git", &defaults).unwrap();
        let normal = theme.resolve_slot("git", &SlotToken::from("branch"), false);
        assert_eq!(normal.fg, Some(Color::Rgb(18, 52, 86)));
        assert!(normal.sub_modifier.contains(Modifier::BOLD));
        let selected = theme.resolve_slot("git", &SlotToken::from("branch"), true);
        assert_eq!(selected.fg, theme.scheme.get("accent").copied());
        assert_eq!(selected.bg, Some(Color::Reset));
        assert!(
            selected
                .sub_modifier
                .contains(Modifier::BOLD | Modifier::ITALIC)
        );
        assert!(
            selected
                .add_modifier
                .contains(Modifier::UNDERLINED | Modifier::CROSSED_OUT)
        );
    }

    #[test]
    fn invalid_colors_and_unknown_references_report_source_and_field() {
        for value in [
            "red",
            "reset",
            "#123",
            "#12345678",
            "#gg1234",
            // Six bytes after '#', with a UTF-8 character crossing a pair boundary.
            "#aé123",
            "#abcé1",
            "ansi:neon-blue",
            "ansi:",
            "",
            "ansi: red",
            "#12 456",
        ] {
            for field in [
                "scheme",
                "picker.text",
                "picker.badge.selected",
                "workflows.git.styles.branch",
            ] {
                let key = if field == "scheme" {
                    "unused"
                } else {
                    "foreground"
                };
                let raw: RawTheme =
                    toml::from_str(&format!("[{field}]\n{key} = {value:?}\n")).unwrap();
                let error = ResolvedTheme::from_raw(&raw, "invalid theme")
                    .unwrap_err()
                    .to_string();
                assert!(error.contains("invalid theme"), "{error}");
                assert!(error.contains(&format!("{field}.{key}")), "{error}");
            }
        }
        for field in [
            "picker.text",
            "picker.badge.selected",
            "workflows.git.styles.branch.selected",
        ] {
            let raw: RawTheme =
                toml::from_str(&format!("[{field}]\nforeground = \"scheme:missing\"\n")).unwrap();
            let error = ResolvedTheme::from_raw(&raw, "unknown theme")
                .unwrap_err()
                .to_string();
            assert!(error.contains("unknown scheme color"), "{error}");
            assert!(error.contains(&format!("{field}.foreground")), "{error}");
            assert!(error.contains("missing"), "{error}");
        }
        // Even unused scheme aliases are invalid; scheme values are literals only.
        let raw: RawTheme = toml::from_str("[scheme]\nunused = \"scheme:accent\"\n").unwrap();
        assert!(
            ResolvedTheme::from_raw(&raw, "alias theme")
                .unwrap_err()
                .to_string()
                .contains("scheme.unused")
        );
        let raw: RawTheme = toml::from_str("[scheme]\n\"\" = \"ansi:red\"\n").unwrap();
        assert!(ResolvedTheme::from_raw(&raw, "empty theme").is_err());
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
    fn scheme_and_binding_references_resolve() {
        let raw: RawTheme = toml::from_str(
            r##"
            [scheme]
            brand = "#102030"
            paper = "#F2E9E1"
            ink = "#204060"
            accent = "#102030"
            selection = "#F2E9E1"
            on-selection = "#204060"
            background = "#F2E9E1"
            foreground = "#204060"
            muted = "#102030"
            border = "#102030"

            [picker.selected]
            foreground = "scheme:on-selection"
            background = "scheme:selection"
            bold = true
            italic = true
            underline = true
            strikethrough = true

            [chrome.divider]
            foreground = "scheme:border"

            [chrome.error]
            foreground = "scheme:on-error"
            background = "scheme:error"

            [picker.preview.text]
            foreground = "scheme:muted"

            [picker.preview.error]
            foreground = "scheme:on-error"
            background = "scheme:error"

            [capture.text]
            foreground = "scheme:accent"
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
    fn user_scheme_names_do_not_shadow_ansi_literals() {
        let raw: RawTheme = toml::from_str(
            r##"
            [scheme]
            cyan = "#102030"
            black = "#203040"
            red = "#304050"
            white = "#F0E0D0"
            border = "ansi:cyan"

            [chrome.text]
            foreground = "ansi:black"
            "##,
        )
        .unwrap();
        let theme = ResolvedTheme::from_raw(&raw, "test theme").unwrap();

        assert_eq!(theme.text.fg, Some(Color::Black));
        assert_eq!(theme.chrome.error.bg, Some(Color::Red));
        assert_eq!(theme.chrome.error.fg, Some(Color::White));
        assert_eq!(theme.chrome.divider.fg, Some(Color::Cyan));
    }

    #[test]
    fn omitted_bindings_use_defaults_and_partial_bindings_are_supported() {
        let raw: RawTheme = toml::from_str(
            r##"
            [scheme]
            quiet = "#696969"
            muted = "#696969"

            [picker.marker]
            bold = false
            "##,
        )
        .unwrap();
        let theme = ResolvedTheme::from_raw(&raw, "test theme").unwrap();

        let baseline = ResolvedTheme::terminal();
        assert_eq!(theme.picker.marker.fg, baseline.picker.marker.fg);
        assert_eq!(theme.picker.marker.bg, baseline.picker.marker.bg);
        assert!(!theme.picker.marker.add_modifier.contains(Modifier::BOLD));
        assert_eq!(theme.picker.muted.fg, Some(Color::Rgb(105, 105, 105)));
        assert_eq!(theme.chrome.footer_key, baseline.chrome.footer_key);
    }

    #[test]
    fn theme_tables_reject_unknown_fields() {
        assert!(toml::from_str::<RawTheme>("[chrome]\nunknown = {}\n").is_err());
        assert!(toml::from_str::<RawTheme>("[picker]\nunknown = {}\n").is_err());
    }

    #[test]
    fn explicit_refs_and_style_fields_keep_their_names_in_errors() {
        let ref_error =
            toml::from_str::<ThemeRef>("source = \"mystery\"\nname = \"x\"\n").unwrap_err();
        assert!(ref_error.to_string().contains("mystery"));

        let field_error =
            toml::from_str::<RawTheme>("[chrome.text]\nforegroundd = \"scheme:accent\"\n")
                .unwrap_err();
        assert!(field_error.to_string().contains("foregroundd"));
    }

    #[test]
    fn configured_named_refs_are_relative_to_the_config_directory() {
        let root = temporary_root();
        fs::create_dir_all(root.join("themes")).unwrap();
        fs::write(
            root.join("themes/work.toml"),
            "[scheme]\naccent = \"ansi:blue\"\n",
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
            "[scheme]\naccent = \"ansi:yellow\"\nmuted = \"ansi:gray\"\n",
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
    fn workflow_styles_and_theme_overrides_with_selected_state() {
        use crate::engine::SlotToken;

        let raw_theme: RawTheme = toml::from_str(
            r#"
            [scheme]
            accent-blue = "ansi:blue"
            selection-bg = "ansi:green"
            highlight-gold = "ansi:yellow"
            accent = "ansi:blue"
            selection = "ansi:green"

            [workflows.git.styles.branch]
            bold = false

            [workflows.git.styles.branch.selected]
            foreground = "scheme:highlight-gold"

            [workflows.git.styles.hash]
            foreground = "scheme:highlight-gold"
            "#,
        )
        .unwrap();
        let mut theme = ResolvedTheme::from_raw(&raw_theme, "tokyonight").unwrap();

        let mut workflow_styles = BTreeMap::new();
        workflow_styles.insert(
            "branch".to_string(),
            RawStyleBinding {
                foreground: Some("scheme:accent".to_string()),
                bold: Some(true),
                ..Default::default()
            },
        );
        workflow_styles.insert(
            "tag".to_string(),
            RawStyleBinding {
                foreground: Some("scheme:accent-blue".to_string()),
                reversed: Some(true),
                ..Default::default()
            },
        );
        theme
            .register_workflow_defaults("git", &workflow_styles)
            .unwrap();

        // 1. Normal slot: branch should have foreground = Blue (scheme:accent), and bold = false (overridden by theme)
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
            [scheme]
            brand = "ansi:blue"
            badge-fg = "ansi:white"
            badge-bg = "ansi:red"
            sel-bg = "ansi:green"
            sel-fg = "ansi:yellow"
            accent = "ansi:blue"
            selection = "ansi:green"

            # Structured TOML table hierarchy
            [picker.text]
            foreground = "scheme:accent"

            [picker.badge]
            foreground = "scheme:badge-fg"
            background = "scheme:badge-bg"
            bold = true

            [picker.badge.selected]
            foreground = "scheme:sel-fg"
            background = "scheme:sel-bg"
            underline = true

            [chrome.divider]
            foreground = "scheme:brand"
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
    fn picker_input_prefix_and_chrome_footer_title_resolve() {
        let raw_theme: RawTheme = toml::from_str(
            r#"
            [scheme]
            brand = "ansi:yellow"
            border_col = "ansi:green"
            title_col = "ansi:magenta"
            status_col = "ansi:gray"
            accent = "ansi:yellow"

            [picker.input_prefix]
            foreground = "scheme:accent"
            bold = true

            [picker.cursor]
            foreground = "scheme:accent"
            background = "scheme:border_col"
            underline = true

            [chrome.border]
            foreground = "scheme:border_col"

            [chrome.footer_title]
            foreground = "scheme:title_col"
            bold = true

            [chrome.footer_status]
            foreground = "scheme:status_col"
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
        assert_eq!(theme.picker.cursor.fg, Some(Color::Yellow));
        assert_eq!(theme.picker.cursor.bg, Some(Color::Green));
        assert!(
            theme
                .picker
                .cursor
                .add_modifier
                .contains(Modifier::UNDERLINED)
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
