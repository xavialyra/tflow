use super::render;
use crate::input::{InputDecoder, Key};
use crate::terminal::Terminal;
use crate::text::sanitize_text;
use anyhow::Result;

pub(crate) struct CaptureSession {
    title: String,
    lines: Vec<String>,
    status: String,
    decoder: InputDecoder,
}

impl CaptureSession {
    pub(crate) fn new(title: &str, output: &str, status: &str) -> Self {
        Self {
            title: title.to_string(),
            lines: capture_lines(output),
            status: status.to_string(),
            decoder: InputDecoder::default(),
        }
    }

    pub(crate) fn wait_for_return(&mut self, terminal: &Terminal) -> Result<()> {
        loop {
            render::render_capture(terminal, &self.title, &self.lines, &self.status)?;
            let bytes = terminal.read_input(80)?;
            let mut keys = self.decoder.feed(&bytes);
            keys.extend(self.decoder.flush_due());
            if keys.iter().any(is_return_key) {
                return Ok(());
            }
        }
    }
}

fn capture_lines(output: &str) -> Vec<String> {
    if output.is_empty() {
        vec!["(no output)".to_string()]
    } else {
        output.lines().map(sanitize_text).collect()
    }
}

fn is_return_key(key: &Key) -> bool {
    matches!(
        key,
        Key::CtrlC | Key::Escape | Key::Enter | Key::Char(_) | Key::Alt(_)
    )
}

#[cfg(test)]
mod tests {
    use super::capture_lines;

    #[test]
    fn capture_lines_keeps_a_visible_line_for_empty_output() {
        assert_eq!(capture_lines(""), vec!["(no output)"]);
    }

    #[test]
    fn capture_lines_sanitizes_each_output_line() {
        assert_eq!(capture_lines("one\x1b[31mtwo\x1b[0m"), vec!["onetwo"]);
    }
}
