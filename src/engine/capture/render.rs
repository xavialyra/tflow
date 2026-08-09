use crate::terminal::Terminal;
use anyhow::{Context, Result};
use std::io::{self, Write};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) fn render_capture(
    terminal: &Terminal,
    lines: &[String],
    chrome: &crate::chrome::ChromeFrame,
) -> Result<()> {
    let (width, height) = terminal.size();
    let width = width as usize;
    let height = height as usize;
    let layout = chrome.layout();
    let viewport_width = layout.viewport_width(width);
    let inner_height = layout.content_rows(height);
    let start = lines.len().saturating_sub(inner_height);

    let mut stdout = io::stdout().lock();
    stdout.write_all(b"\x1b[?25l")?;
    let input = layout.pad_line(&chrome.input_line(), width, layout.viewport_padding);
    let divider = layout.pad_line(&chrome.divider, width, layout.viewport_padding);
    let footer = layout.pad_line(&chrome.footer, width, layout.viewport_padding);
    write_capture_line(
        &mut stdout,
        layout.input_content_row() + 1,
        &input,
        width,
        false,
    )?;
    write_capture_line(
        &mut stdout,
        layout.divider_content_row() + 1,
        &divider,
        width,
        true,
    )?;
    for row in 0..inner_height {
        let content = lines.get(start + row).map(String::as_str).unwrap_or("");
        let content = layout.pad_line(content, viewport_width, layout.content_padding);
        let content = layout.pad_line(&content, width, layout.viewport_padding);
        write_capture_line(
            &mut stdout,
            layout.content_start_row() + row + 1,
            &content,
            width,
            false,
        )?;
    }
    write_capture_line(
        &mut stdout,
        layout.footer_row(height) + 1,
        &footer,
        width,
        false,
    )?;
    stdout.flush().context("could not draw command output")
}

fn write_capture_line(
    stdout: &mut impl Write,
    row: usize,
    text: &str,
    width: usize,
    heading: bool,
) -> Result<()> {
    write!(stdout, "\x1b[{};1H\x1b[K", row)?;
    let text = clip(text, width);
    if heading {
        write!(stdout, "\x1b[1;36m{}\x1b[0m", text)?;
    } else {
        stdout.write_all(text.as_bytes())?;
    }
    Ok(())
}

fn clip(text: &str, width: usize) -> String {
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
