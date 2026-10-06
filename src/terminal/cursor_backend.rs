//! Keep the cursor hidden while Ratatui emits a frame, and restore its
//! visibility only after the final cursor position has been written.
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

pub(super) struct CursorBackend<B> {
    inner: B,
    drawing: bool,
    show_after_draw: bool,
}

impl<B> CursorBackend<B> {
    pub(super) fn new(inner: B) -> Self {
        Self {
            inner,
            drawing: false,
            show_after_draw: false,
        }
    }

    pub(super) fn abort_draw(&mut self) {
        self.drawing = false;
        self.show_after_draw = false;
    }
}

impl<B: Backend> Backend for CursorBackend<B> {
    type Error = B::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.drawing = true;
        self.show_after_draw = false;
        self.inner.hide_cursor()?;
        self.inner.draw(content)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        if self.drawing {
            self.show_after_draw = false;
            Ok(())
        } else {
            self.inner.hide_cursor()
        }
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        if self.drawing {
            self.show_after_draw = true;
            Ok(())
        } else {
            self.inner.show_cursor()
        }
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }
    fn clear_region(&mut self, kind: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(kind)
    }
    fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(n)
    }
    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }
    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        if self.drawing {
            self.drawing = false;
            if self.show_after_draw {
                self.inner.show_cursor()?;
            }
            self.show_after_draw = false;
        }
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::CrosstermBackend;

    #[test]
    fn cursor_is_hidden_during_text_and_image_output_and_shown_after_positioning() {
        let mut output = Vec::new();
        {
            let mut backend = CursorBackend::new(CrosstermBackend::new(&mut output));
            let mut text = Cell::default();
            text.set_symbol("DETAILS");
            let mut image = Cell::default();
            image.set_symbol("\x1b_Ga=T;IMAGE\x1b\\");
            backend
                .draw([(10, 2, &text), (40, 4, &image)].into_iter())
                .unwrap();
            // This is the order Ratatui calls: show, then position, then flush.
            backend.show_cursor().unwrap();
            backend.set_cursor_position((3, 0)).unwrap();
            backend.flush().unwrap();
        }
        let bytes = String::from_utf8(output).unwrap();
        let hide = bytes.find("\x1b[?25l").unwrap();
        let image = bytes.find("IMAGE").unwrap();
        let position = bytes.find("\x1b[1;4H").unwrap();
        let show = bytes.find("\x1b[?25h").unwrap();
        assert!(
            hide < image && image < position && position < show,
            "{bytes:?}"
        );
        assert_eq!(bytes.matches("\x1b[?25h").count(), 1);
    }

    #[test]
    fn cursorless_frame_does_not_show_cursor_and_abort_allows_restoration() {
        let mut output = Vec::new();
        {
            let mut backend = CursorBackend::new(CrosstermBackend::new(&mut output));
            backend.draw(std::iter::empty()).unwrap();
            backend.hide_cursor().unwrap();
            backend.flush().unwrap();
        }
        assert!(!output.windows(6).any(|bytes| bytes == b"\x1b[?25h"));
        {
            let mut backend = CursorBackend::new(CrosstermBackend::new(&mut output));
            backend.draw(std::iter::empty()).unwrap();
            backend.abort_draw();
            backend.show_cursor().unwrap();
        }
        assert!(output.ends_with(b"\x1b[?25h"));
    }
}
