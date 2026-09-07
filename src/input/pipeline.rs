use crate::input::{InputDecoder, InputEvent};
use crate::terminal::{InputRead, Terminal};
use anyhow::Result;
use std::collections::VecDeque;

/// Session-owned input pipeline. It preserves pending decoder events across frames.
#[derive(Debug, Default)]
pub(crate) struct InputPipeline {
    decoder: InputDecoder,
    pending: VecDeque<InputEvent>,
}

impl InputPipeline {
    pub(crate) fn read_normal(
        &mut self,
        terminal: &mut Terminal,
        timeout: i32,
    ) -> Result<InputRead> {
        let read = terminal.read_input(timeout)?;
        match &read {
            InputRead::Data(bytes) => self.feed_normal(bytes),
            InputRead::Timeout => self.flush_normal_due(),
            InputRead::Eof => {
                let pending = self.decoder.take_pending_raw();
                if !pending.is_empty() {
                    self.pending.push_back(InputEvent::Bytes(pending));
                }
                self.pending.push_back(InputEvent::Eof);
            }
        }
        Ok(read)
    }

    pub(crate) fn feed_normal(&mut self, bytes: &[u8]) {
        self.pending.extend(
            self.decoder
                .feed(bytes)
                .into_iter()
                .map(InputEvent::decoded),
        );
    }

    pub(crate) fn flush_normal_due(&mut self) {
        self.pending.extend(
            self.decoder
                .flush_due()
                .into_iter()
                .map(InputEvent::decoded),
        );
    }

    pub(crate) fn pop_input(&mut self) -> Option<InputEvent> {
        self.pending.pop_front()
    }

    pub(crate) fn pending_is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Discard launcher input already decoded or buffered before a foreground
    /// command takes over the terminal.
    pub(crate) fn discard_pending(&mut self) {
        self.pending.clear();
        self.decoder.take_pending_raw();
    }

    pub(crate) fn preserve_decoder_pending(&mut self) {
        let pending = self.decoder.take_pending_raw();
        if !pending.is_empty() {
            self.pending.push_back(InputEvent::Bytes(pending));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classified_paste_is_transferred_without_redecoding() {
        let mut pipeline = InputPipeline::default();
        pipeline.feed_normal(b"\x1b[200~text\x02\x1b[201~");
        pipeline.preserve_decoder_pending();
        assert_eq!(
            pipeline.pop_input(),
            Some(InputEvent::Paste {
                text: Some("text\u{2}".to_string()),
                raw: b"\x1b[200~text\x02\x1b[201~".to_vec(),
            })
        );
    }

    #[test]
    fn incomplete_decoder_bytes_become_raw_events_on_strategy_change() {
        let mut pipeline = InputPipeline::default();
        pipeline.feed_normal(&[0xc3]);
        pipeline.preserve_decoder_pending();
        assert_eq!(pipeline.pop_input(), Some(InputEvent::Bytes(vec![0xc3])));
    }

    #[test]
    fn discarding_after_a_foreground_trigger_removes_decoded_and_partial_input() {
        let mut pipeline = InputPipeline::default();
        pipeline.feed_normal(b"\rqueued");
        assert!(matches!(pipeline.pop_input(), Some(InputEvent::Key { .. })));
        pipeline.feed_normal(&[0xc3]);

        pipeline.discard_pending();

        assert!(pipeline.pending_is_empty());
        pipeline.preserve_decoder_pending();
        assert!(pipeline.pending_is_empty());
    }
}
