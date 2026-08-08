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
    let inner_height = height.saturating_sub(3);
    let start = lines.len().saturating_sub(inner_height);

    let mut stdout = io::stdout().lock();
    stdout.write_all(b"\x1b[?25l")?;
    write_capture_line(&mut stdout, 1, &chrome.header, width, true)?;
    write_capture_line(&mut stdout, 2, &chrome.input_line(), width, false)?;
    for row in 0..inner_height {
        let content = lines.get(start + row).map(String::as_str).unwrap_or("");
        write_capture_line(&mut stdout, 3 + row, content, width, false)?;
    }
    write_capture_line(&mut stdout, height.max(1), &chrome.footer, width, false)?;
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
