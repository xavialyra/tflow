use crate::chrome::{ChromeContent, ChromeCursor, ChromeFrame};
use crate::terminal::Terminal;
use anyhow::Result;

pub(super) fn capture_content(
    terminal: &Terminal,
    lines: &[String],
    chrome: &ChromeFrame,
) -> Result<ChromeContent> {
    let (_, height) = terminal.size();
    let content_rows = chrome.layout().content_rows(height as usize);
    let start = lines.len().saturating_sub(content_rows);
    Ok(ChromeContent::new(
        lines[start..].to_vec(),
        None,
        ChromeCursor::Hidden,
    ))
}
