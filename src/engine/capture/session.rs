use crate::terminal::sanitize_text;

pub(crate) struct CaptureSession {
    title: String,
    output: String,
    lines: Vec<String>,
    status: String,
}

impl CaptureSession {
    pub(crate) fn new(title: &str, output: &str, status: &str) -> Self {
        Self {
            title: title.to_string(),
            output: output.to_string(),
            lines: capture_lines(output),
            status: status.to_string(),
        }
    }

    pub(crate) fn output(&self) -> &str {
        &self.output
    }

    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    pub(crate) fn lines(&self) -> &[String] {
        &self.lines
    }

    pub(crate) fn status(&self) -> &str {
        &self.status
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
