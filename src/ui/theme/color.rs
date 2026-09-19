use anyhow::{Result, bail};
use ratatui::style::Color;
use std::collections::BTreeMap;

pub(super) type ResolvedScheme = BTreeMap<String, Color>;

pub(super) fn resolve_scheme(
    raw: &BTreeMap<String, String>,
    source: &str,
) -> Result<ResolvedScheme> {
    raw.iter()
        .map(|(name, value)| {
            if name.trim().is_empty() {
                bail!("{source} scheme names must not be empty");
            }
            let color = resolve_color_literal(value, source, &format!("scheme.{name}"))?;
            Ok((name.clone(), color))
        })
        .collect()
}

pub(super) fn resolve_color_reference(
    value: &str,
    scheme: &ResolvedScheme,
    source: &str,
    context_desc: &str,
) -> Result<Color> {
    let value = value.trim();
    if let Some(name) = value.strip_prefix("scheme:") {
        return scheme.get(name).copied().ok_or_else(|| {
            anyhow::anyhow!("{source} {context_desc} references unknown scheme color {name:?}")
        });
    }
    resolve_color_literal(value, source, context_desc)
}

fn resolve_color_literal(value: &str, source: &str, context_desc: &str) -> Result<Color> {
    let value = value.trim();
    if let Some(name) = value.strip_prefix("ansi:") {
        return parse_ansi_color(name).ok_or_else(|| {
            anyhow::anyhow!(
                "{source} {context_desc} has unsupported ANSI color {name:?}; expected {}",
                ansi_color_names()
            )
        });
    }
    if let Some(color) = parse_hex_color(value) {
        return Ok(color);
    }
    bail!("{source} {context_desc} has invalid color {value:?}; expected ansi:NAME or #RRGGBB")
}

pub(super) fn ansi_color_names() -> &'static str {
    "black, red, green, yellow, blue, magenta, cyan, gray, white, bright-black, bright-red, bright-green, bright-yellow, bright-blue, bright-magenta, bright-cyan, bright-white, or reset"
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
        "bright-black" => Some(Color::DarkGray),
        "bright-red" => Some(Color::LightRed),
        "bright-green" => Some(Color::LightGreen),
        "bright-yellow" => Some(Color::LightYellow),
        "bright-blue" => Some(Color::LightBlue),
        "bright-magenta" => Some(Color::LightMagenta),
        "bright-cyan" => Some(Color::LightCyan),
        "bright-white" => Some(Color::White),
        "reset" => Some(Color::Reset),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_ansi_foreground_colors() {
        let colors = [
            ("black", Color::Black),
            ("red", Color::Red),
            ("green", Color::Green),
            ("yellow", Color::Yellow),
            ("blue", Color::Blue),
            ("magenta", Color::Magenta),
            ("cyan", Color::Cyan),
            ("gray", Color::Gray),
            ("bright-black", Color::DarkGray),
            ("bright-red", Color::LightRed),
            ("bright-green", Color::LightGreen),
            ("bright-yellow", Color::LightYellow),
            ("bright-blue", Color::LightBlue),
            ("bright-magenta", Color::LightMagenta),
            ("bright-cyan", Color::LightCyan),
            ("bright-white", Color::White),
        ];

        for (name, expected) in colors {
            assert_eq!(parse_ansi_color(name), Some(expected), "ANSI color {name}");
            assert_eq!(
                parse_ansi_color(&name.to_ascii_uppercase()),
                Some(expected),
                "ANSI color {name} should be case-insensitive"
            );
        }
    }
}
