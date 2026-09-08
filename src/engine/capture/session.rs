use crate::terminal::sanitize_text;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct CaptureSession {
    output: String,
    lines: Arc<[String]>,
}

impl CaptureSession {
    pub(crate) fn new(output: &str) -> Self {
        Self {
            output: output.to_string(),
            lines: capture_lines(output).into(),
        }
    }

    pub(crate) fn output(&self) -> &str {
        &self.output
    }

    pub(crate) fn shared_lines(&self) -> Arc<[String]> {
        Arc::clone(&self.lines)
    }
}

fn capture_lines(output: &str) -> Vec<String> {
    if output.is_empty() {
        vec!["(no output)".to_string()]
    } else {
        output.lines().map(sanitize_text).collect()
    }
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
