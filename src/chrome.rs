use crate::router::RouteDisplay;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Default)]
pub(crate) struct EngineChrome {
    pub(crate) title: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) commands: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ShellInput {
    pub(crate) raw: String,
    pub(crate) params: String,
    pub(crate) changed: bool,
    pub(crate) rejected: bool,
}

impl ShellInput {
    pub(crate) fn new(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        Self {
            params: raw.clone(),
            raw,
            changed: false,
            rejected: false,
        }
    }

    pub(crate) fn with_params(raw: impl Into<String>, params: impl Into<String>) -> Self {
        Self {
            raw: raw.into(),
            params: params.into(),
            changed: false,
            rejected: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChromeFrame {
    pub(crate) divider: String,
    pub(crate) input: String,
    pub(crate) footer: String,
}

impl ChromeFrame {
    pub(crate) fn input_line(&self) -> String {
        format!(" > {}", self.input)
    }

    pub(crate) fn compose(
        width: usize,
        route: &RouteDisplay,
        input: &str,
        engine: EngineChrome,
        error: Option<&str>,
    ) -> Self {
        let width = width.saturating_sub(1);
        let footer = if let Some(error) = error {
            clip(error, width)
        } else {
            footer_line(
                width,
                engine.title.as_deref(),
                engine.status.as_deref().unwrap_or(""),
                &engine.commands,
            )
        };
        Self {
            divider: divider_line(width, &route.label()),
            input: input.to_string(),
            footer,
        }
    }
}

pub(crate) fn divider_line(width: usize, label: &str) -> String {
    if width == 0 {
        return String::new();
    }
    if label.is_empty() {
        return "─".repeat(width);
    }

    let label = clip(label, width);
    let label_width = UnicodeWidthStr::width(label.as_str());
    if label_width >= width {
        return label;
    }
    format!("{} {}", label, "─".repeat(width - label_width - 1))
}

fn footer_line(
    width: usize,
    title: Option<&str>,
    status: &str,
    commands: &[(String, String)],
) -> String {
    if width == 0 {
        return String::new();
    }
    let left = footer_label(title, status);
    let right = command_text(commands);
    if right.is_empty() {
        return clip(&left, width);
    }
    if left.is_empty() {
        return right_aligned(width, &right);
    }

    let separator = " | ";
    let separator_width = UnicodeWidthStr::width(separator);
    let full_width = UnicodeWidthStr::width(left.as_str())
        + separator_width
        + UnicodeWidthStr::width(right.as_str());
    if full_width <= width {
        let padding = width - full_width;
        return format!("{}{}{}{}", left, " ".repeat(padding), separator, right);
    }
    if separator_width >= width {
        return clip(&right, width);
    }

    let right_budget = (width * 3 / 5).max(1);
    let right = clip(&right, right_budget);
    let left_budget =
        width.saturating_sub(UnicodeWidthStr::width(right.as_str()) + separator_width);
    if left_budget == 0 {
        return clip(&right, width);
    }
    let left = clip(&left, left_budget);
    let used = UnicodeWidthStr::width(left.as_str())
        + separator_width
        + UnicodeWidthStr::width(right.as_str());
    let padding = width.saturating_sub(used);
    format!("{}{}{}{}", left, " ".repeat(padding), separator, right)
}

fn footer_label(title: Option<&str>, status: &str) -> String {
    let mut parts = Vec::new();
    if let Some(title) = title.filter(|title| !title.is_empty()) {
        parts.push(title.to_string());
    }
    if !status.is_empty() {
        parts.push(status.to_string());
    }
    parts.join(" | ")
}

fn right_aligned(width: usize, text: &str) -> String {
    let text = clip(text, width);
    let padding = width.saturating_sub(UnicodeWidthStr::width(text.as_str()));
    format!("{}{}", " ".repeat(padding), text)
}

fn command_text(commands: &[(String, String)]) -> String {
    commands
        .iter()
        .map(|(key, label)| format!("{} {}", display_binding(key), label))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn display_binding(key: &str) -> String {
    if key == "enter" {
        return "Enter".to_string();
    }
    key.strip_prefix("alt+")
        .map(|character| format!("Alt-{}", character.to_ascii_uppercase()))
        .unwrap_or_else(|| key.to_string())
}

pub(crate) fn clip(text: &str, width: usize) -> String {
    if width == 0 || UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    if width <= 3 {
        return text.chars().take(width).collect();
    }

    let mut result = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width - 3 {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result.push_str("...");
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> RouteDisplay {
        RouteDisplay {
            view_ref: "apps:default".to_string(),
            alias: Some("app".to_string()),
        }
    }

    #[test]
    fn composes_route_engine_status_and_commands() {
        let frame = ChromeFrame::compose(
            80,
            &route(),
            "terminal",
            EngineChrome {
                title: None,
                status: Some("12 results".to_string()),
                commands: vec![("enter".to_string(), "Open".to_string())],
            },
            None,
        );
        assert!(frame.divider.starts_with("apps:default (app) "));
        assert!(frame.divider.contains('─'));
        assert_eq!(UnicodeWidthStr::width(frame.divider.as_str()), 79);
        assert_eq!(frame.input, "terminal");
        assert_eq!(frame.input_line(), " > terminal");
        assert!(frame.footer.starts_with("12 results"));
        assert!(frame.footer.ends_with(" | Enter Open"));
        assert_eq!(UnicodeWidthStr::width(frame.footer.as_str()), 79);
    }

    #[test]
    fn divider_fills_width_without_a_label() {
        assert_eq!(divider_line(5, ""), "─────");
    }

    #[test]
    fn error_replaces_the_complete_footer() {
        let frame = ChromeFrame::compose(
            80,
            &route(),
            "",
            EngineChrome::default(),
            Some("view alias is ambiguous"),
        );
        assert_eq!(frame.footer, "view alias is ambiguous");
    }
}
