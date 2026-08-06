use super::render;
use crate::discovery::sanitize_text;
use crate::engine::{Key, PreparedCommand};
use crate::input::InputDecoder;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use std::process::Stdio;

pub(crate) struct CaptureSession {
    title: String,
    lines: Vec<String>,
    status: String,
    decoder: InputDecoder,
}

pub(crate) struct CaptureOutcome {
    pub(crate) status: String,
    pub(crate) success: bool,
}

impl CaptureSession {
    pub(crate) fn new(title: &str) -> Self {
        Self {
            title: title.to_string(),
            lines: Vec::new(),
            status: String::new(),
            decoder: InputDecoder::default(),
        }
    }

    pub(crate) fn execute(
        &mut self,
        prepared: PreparedCommand,
        terminal: &mut Terminal,
        command_id: &str,
    ) -> Result<CaptureOutcome> {
        let process_result = (|| {
            terminal.leave()?;
            let result = prepared
                .process()
                .stdin(Stdio::null())
                .output()
                .with_context(|| format!("could not run command {}", command_id));
            terminal.reenter()?;
            Ok::<_, anyhow::Error>(result)
        })()?;

        let (text, status, success) = match process_result {
            Ok(output) => {
                let mut text = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.is_empty() {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(&stderr);
                }
                (
                    sanitize_text(&text),
                    status_message(&output.status),
                    output.status.success(),
                )
            }
            Err(error) => (error.to_string(), "failed".to_string(), false),
        };

        self.lines = capture_lines(&text);
        self.status = status.clone();
        Ok(CaptureOutcome { status, success })
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

fn status_message(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(0) => "finished successfully".to_string(),
        Some(code) => format!("finished with exit code {}", code),
        None => "terminated by signal".to_string(),
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
