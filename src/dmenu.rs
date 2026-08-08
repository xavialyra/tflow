use crate::config::DisplayType;
use crate::input::{InputDecoder, Key};
use crate::terminal::Terminal;
use crate::text::matches_query;
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::fmt::Write as FmtWrite;
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub struct Options {
    pub prompt: String,
    pub lines: Option<usize>,
    pub initial: String,
    pub index: bool,
    pub dmenu0: bool,
    pub display: DisplayType,
    pub with_nth: Option<String>,
    pub accept_nth: Option<String>,
    pub match_nth: Option<String>,
    pub nth_delimiter: Option<String>,
}

pub enum Outcome {
    Selected { value: Vec<u8>, terminator: u8 },
    Cancelled,
}

pub(crate) struct Candidate {
    raw: Vec<u8>,
    text: String,
    #[allow(dead_code)]
    pub(crate) metadata: Value,
    index: usize,
}

struct DmenuApp {
    candidates: Vec<Candidate>,
    prompt: String,
    lines: Option<usize>,
    display: DisplayType,
    index_output: bool,
    with_nth: Option<FieldFormat>,
    accept_nth: Option<FieldFormat>,
    match_nth: Option<FieldFormat>,
    delimiter: FieldDelimiter,
    record_separator: RecordSeparator,
    query: String,
    selected: usize,
    decoder: InputDecoder,
    message: Option<String>,
}

#[derive(Debug, Clone)]
enum FieldFormat {
    Fields(Vec<FieldSelector>),
    Template(Vec<TemplatePart>),
}

#[derive(Debug, Clone)]
enum TemplatePart {
    Literal(String),
    Fields(FieldSelector),
}

#[derive(Debug, Clone)]
enum FieldSelector {
    Single(usize),
    Range { start: usize, end: Option<usize> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordSeparator {
    Newline,
    Nul,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldDelimiter {
    Character(char),
    Whitespace,
}

impl RecordSeparator {
    fn byte(self) -> u8 {
        match self {
            Self::Newline => b'\n',
            Self::Nul => 0,
        }
    }
}

impl FieldDelimiter {
    fn fields(self, text: &str) -> Vec<&str> {
        match self {
            Self::Character(delimiter) => text.split(delimiter).collect(),
            Self::Whitespace => text.split_whitespace().collect(),
        }
    }

    fn output_separator(self) -> String {
        match self {
            Self::Character(delimiter) => delimiter.to_string(),
            Self::Whitespace => " ".to_string(),
        }
    }
}

impl FieldFormat {
    fn parse(source: Option<&str>) -> Result<Option<Self>> {
        let Some(source) = source else {
            return Ok(None);
        };
        if source.trim() == "0" {
            return Ok(None);
        }

        let looks_like_field_list = source
            .chars()
            .all(|character| character.is_ascii_digit() || matches!(character, ',' | '.'));
        if looks_like_field_list {
            let selectors = source
                .split(',')
                .map(|token| parse_selector(token, source))
                .collect::<Result<Vec<_>>>()?;
            if selectors.is_empty() {
                bail!("field format must not be empty");
            }
            return Ok(Some(Self::Fields(selectors)));
        }

        Ok(Some(Self::Template(parse_template(source)?)))
    }

    fn render(&self, text: &str, delimiter: FieldDelimiter, joiner: &str) -> String {
        let fields = delimiter.fields(text);
        match self {
            Self::Fields(selectors) => selectors
                .iter()
                .flat_map(|selector| selected_fields(&fields, selector))
                .collect::<Vec<_>>()
                .join(joiner),
            Self::Template(parts) => {
                let mut output = String::new();
                for part in parts {
                    match part {
                        TemplatePart::Literal(literal) => output.push_str(literal),
                        TemplatePart::Fields(selector) => {
                            output.push_str(&selected_fields(&fields, selector).join(joiner))
                        }
                    }
                }
                output
            }
        }
    }
}

impl DmenuApp {
    fn new(
        candidates: Vec<Candidate>,
        options: Options,
        with_nth: Option<FieldFormat>,
        accept_nth: Option<FieldFormat>,
        match_nth: Option<FieldFormat>,
        delimiter: FieldDelimiter,
        record_separator: RecordSeparator,
    ) -> Self {
        Self {
            candidates,
            prompt: sanitize_for_display(&options.prompt),
            lines: options.lines,
            display: options.display,
            index_output: options.index,
            with_nth,
            accept_nth,
            match_nth,
            delimiter,
            record_separator,
            query: sanitize_for_display(&options.initial),
            selected: 0,
            decoder: InputDecoder::default(),
            message: None,
        }
    }

    fn run(&mut self, terminal: &mut Terminal) -> Result<Outcome> {
        loop {
            self.render(terminal)?;
            let bytes = terminal.read_input(80)?;
            let mut keys = self.decoder.feed(&bytes);
            keys.extend(self.decoder.flush_due());

            for key in keys {
                if let Some(outcome) = self.handle_key(key) {
                    return Ok(outcome);
                }
            }
        }
    }

    fn handle_key(&mut self, key: Key) -> Option<Outcome> {
        match key {
            Key::Escape | Key::Ctrl('c' | 'd') => Some(Outcome::Cancelled),
            Key::Enter => {
                let matching = self.matching_indices();
                if let Some(index) = matching.get(self.selected) {
                    return Some(Outcome::Selected {
                        value: self.output_for(*index),
                        terminator: self.record_separator.byte(),
                    });
                }
                if !self.query.is_empty() {
                    return Some(Outcome::Selected {
                        value: self.query.as_bytes().to_vec(),
                        terminator: self.record_separator.byte(),
                    });
                }
                self.message = Some("no matching item".to_string());
                None
            }
            Key::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.message = None;
                None
            }
            Key::Down => {
                let count = self.matching_indices().len();
                if count > 0 {
                    self.selected = (self.selected + 1).min(count - 1);
                }
                self.message = None;
                None
            }
            Key::Backspace => {
                if self.query.pop().is_some() {
                    self.selected = 0;
                    self.message = None;
                }
                None
            }
            Key::Ctrl('u') => {
                if !self.query.is_empty() {
                    self.query.clear();
                    self.selected = 0;
                    self.message = None;
                }
                None
            }
            Key::Ctrl('w') => {
                let previous_length = self.query.len();
                while self.query.chars().last().is_some_and(char::is_whitespace) {
                    self.query.pop();
                }
                while !self.query.chars().last().is_some_and(char::is_whitespace)
                    && !self.query.is_empty()
                {
                    self.query.pop();
                }
                if self.query.len() != previous_length {
                    self.selected = 0;
                    self.message = None;
                }
                None
            }
            Key::Char(character) if !character.is_control() => {
                self.query.push(character);
                self.selected = 0;
                self.message = None;
                None
            }
            Key::Char(_) | Key::Alt(_) | Key::Ctrl(_) => None,
        }
    }

    fn matching_indices(&self) -> Vec<usize> {
        self.candidates
            .iter()
            .enumerate()
            .filter_map(|(index, candidate)| {
                matches_query(&self.match_text(candidate), &self.query).then_some(index)
            })
            .collect()
    }

    fn display_text(&self, candidate: &Candidate) -> String {
        let rendered = match self.display {
            DisplayType::Text => self
                .with_nth
                .as_ref()
                .map(|format| format.render(&candidate.text, self.delimiter, " "))
                .unwrap_or_else(|| candidate.text.clone()),
        };
        sanitize_for_display(&rendered)
    }

    fn match_text(&self, candidate: &Candidate) -> String {
        let rendered = self
            .match_nth
            .as_ref()
            .map(|format| format.render(&candidate.text, self.delimiter, " "))
            .unwrap_or_else(|| self.display_text(candidate));
        sanitize_for_display(&rendered)
    }

    fn output_for(&self, index: usize) -> Vec<u8> {
        let candidate = &self.candidates[index];
        if self.index_output {
            return candidate.index.to_string().into_bytes();
        }
        if let Some(format) = &self.accept_nth {
            return format
                .render(
                    &candidate.text,
                    self.delimiter,
                    &self.delimiter.output_separator(),
                )
                .into_bytes();
        }
        candidate.raw.clone()
    }

    fn render(&self, terminal: &Terminal) -> Result<()> {
        let (width, height) = terminal.size();
        let width = width as usize;
        let height = height as usize;
        let matching = self.matching_indices();
        let footer = self.footer();
        let available_rows = height.saturating_sub(3);
        let list_height = self
            .lines
            .map(|lines| lines.min(available_rows))
            .unwrap_or(available_rows);
        let mut lines = Vec::with_capacity(height);

        lines.push(" TUI Launcher  [dmenu]".to_string());
        lines.push(query_line(&self.prompt, &self.query, width));

        let start = if self.selected >= list_height && list_height > 0 {
            self.selected + 1 - list_height
        } else {
            0
        };
        let selected_row = if matching.is_empty() || list_height == 0 {
            None
        } else {
            Some(2 + self.selected.saturating_sub(start))
        };

        if list_height > 0 {
            if matching.is_empty() {
                lines.push(if self.candidates.is_empty() {
                    "   (no input)".to_string()
                } else {
                    "   (no matches)".to_string()
                });
            } else {
                for (offset, index) in matching.iter().skip(start).take(list_height).enumerate() {
                    let candidate = &self.candidates[*index];
                    let marker = if self.selected == start + offset {
                        "> "
                    } else {
                        "  "
                    };
                    lines.push(format!("{}{}", marker, self.display_text(candidate)));
                }
            }
        }

        while lines.len() < height.saturating_sub(1) {
            lines.push(String::new());
        }
        lines.push(footer);

        let mut output = String::new();
        output.push_str("\x1b[H");
        for row in 0..height {
            let line = lines.get(row).map(String::as_str).unwrap_or("");
            let clipped = clip(line, width);
            if selected_row == Some(row) {
                output.push_str("\x1b[7m");
                output.push_str(&clipped);
                output.push_str("\x1b[0m");
            } else if row == 0 {
                output.push_str("\x1b[1;36m");
                output.push_str(&clipped);
                output.push_str("\x1b[0m");
            } else {
                output.push_str(&clipped);
            }
            output.push_str("\x1b[K");
            if row + 1 < height {
                output.push_str("\r\n");
            }
        }

        if height >= 2 {
            let cursor_width =
                UnicodeWidthStr::width(query_line(&self.prompt, &self.query, width).as_str());
            let cursor_column = cursor_width.min(width.saturating_sub(1)).max(1) + 1;
            write!(output, "\x1b[2;{}H\x1b[?25h", cursor_column)?;
        }
        terminal
            .write_output(output.as_bytes())
            .context("could not draw dmenu launcher")
    }

    fn footer(&self) -> String {
        if let Some(message) = &self.message {
            return format!(" {}", message);
        }
        " Up/Down select | Enter accept | Esc cancel | Ctrl-C cancel".to_string()
    }
}

pub fn run(options: Options) -> Result<Outcome> {
    let input_fd = io::stdin().as_raw_fd();
    if unsafe { libc::isatty(input_fd) } == 1 {
        bail!("dmenu mode expects newline- or NUL-delimited candidates on stdin");
    }
    if options.lines == Some(0) {
        bail!("--lines must be greater than zero");
    }

    let record_separator = if options.dmenu0 {
        RecordSeparator::Nul
    } else {
        RecordSeparator::Newline
    };
    let delimiter = parse_delimiter(options.nth_delimiter.as_deref())?;
    let with_nth = FieldFormat::parse(options.with_nth.as_deref())?;
    let accept_nth = FieldFormat::parse(options.accept_nth.as_deref())?;
    let match_nth = FieldFormat::parse(options.match_nth.as_deref())?;

    let mut input = Vec::new();
    io::stdin()
        .read_to_end(&mut input)
        .context("could not read dmenu candidates from stdin")?;
    let candidates = parse_candidates(&input, record_separator);

    let tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .context("could not open /dev/tty for dmenu interaction")?;
    let mut terminal = Terminal::enter_with_fds(tty.as_raw_fd(), tty.as_raw_fd())?;
    let mut app = DmenuApp::new(
        candidates,
        options,
        with_nth,
        accept_nth,
        match_nth,
        delimiter,
        record_separator,
    );
    let result = app.run(&mut terminal);
    let leave_result = terminal.leave();

    result.and_then(|outcome| {
        leave_result?;
        Ok(outcome)
    })
}

fn parse_candidates(input: &[u8], separator: RecordSeparator) -> Vec<Candidate> {
    if input.is_empty() {
        return Vec::new();
    }

    let separator_byte = separator.byte();
    let input = input.strip_suffix(&[separator_byte]).unwrap_or(input);
    input
        .split(|byte| *byte == separator_byte)
        .enumerate()
        .map(|(index, record)| {
            let record = if separator == RecordSeparator::Newline {
                record.strip_suffix(b"\r").unwrap_or(record)
            } else {
                record
            };
            let (text_bytes, metadata) = if separator == RecordSeparator::Newline {
                parse_rofi_record(record)
            } else {
                (record, Value::Object(Map::new()))
            };
            Candidate {
                raw: text_bytes.to_vec(),
                text: String::from_utf8_lossy(text_bytes).into_owned(),
                metadata,
                index,
            }
        })
        .collect()
}

fn parse_rofi_record(record: &[u8]) -> (&[u8], Value) {
    let Some(metadata_start) = record.iter().position(|byte| *byte == 0) else {
        return (record, Value::Object(Map::new()));
    };

    let text = &record[..metadata_start];
    let mut metadata = Map::new();
    for entry in record[metadata_start + 1..].split(|byte| *byte == 0) {
        let Some(separator) = entry.iter().position(|byte| *byte == 0x1f) else {
            continue;
        };
        let key = String::from_utf8_lossy(&entry[..separator]);
        if key.is_empty() {
            continue;
        }
        let value = String::from_utf8_lossy(&entry[separator + 1..]);
        metadata.insert(key.into_owned(), Value::String(value.into_owned()));
    }
    (text, Value::Object(metadata))
}

fn parse_delimiter(value: Option<&str>) -> Result<FieldDelimiter> {
    let value = value.unwrap_or("\t");
    if value == "whitespace" {
        return Ok(FieldDelimiter::Whitespace);
    }

    let mut characters = value.chars();
    let Some(delimiter) = characters.next() else {
        bail!("--nth-delimiter must not be empty");
    };
    if characters.next().is_some() || !delimiter.is_ascii() {
        bail!("--nth-delimiter must be a single ASCII character or 'whitespace'");
    }
    Ok(FieldDelimiter::Character(delimiter))
}

fn parse_selector(token: &str, source: &str) -> Result<FieldSelector> {
    let token = token.trim();
    if token.is_empty() {
        bail!("field format {:?} contains an empty selector", source);
    }

    if let Some((start, end)) = token.split_once("..") {
        let start = parse_index(start, source)?;
        let end = if end.is_empty() {
            None
        } else {
            Some(parse_index(end, source)?)
        };
        if end.is_some_and(|end| start > end) {
            bail!("field range {:?} is backwards", token);
        }
        return Ok(FieldSelector::Range { start, end });
    }

    Ok(FieldSelector::Single(parse_index(token, source)?))
}

fn parse_index(value: &str, source: &str) -> Result<usize> {
    let index = value.trim().parse::<usize>().with_context(|| {
        format!(
            "invalid field selector {:?} in field format {:?}",
            value, source
        )
    })?;
    if index == 0 {
        bail!("field indexes start at 1; use 0 to disable a field format");
    }
    Ok(index)
}

fn parse_template(source: &str) -> Result<Vec<TemplatePart>> {
    let characters: Vec<char> = source.chars().collect();
    let mut parts = Vec::new();
    let mut literal = String::new();
    let mut position = 0;

    while position < characters.len() {
        match characters[position] {
            '{' => {
                if !literal.is_empty() {
                    parts.push(TemplatePart::Literal(std::mem::take(&mut literal)));
                }
                let start = position + 1;
                let Some(relative_end) = characters[start..]
                    .iter()
                    .position(|character| *character == '}')
                else {
                    bail!("field format {:?} has an unclosed '{{'", source);
                };
                let end = start + relative_end;
                let selector = parse_selector(
                    characters[start..end].iter().collect::<String>().as_str(),
                    source,
                )?;
                parts.push(TemplatePart::Fields(selector));
                position = end + 1;
            }
            '}' => bail!("field format {:?} has an unmatched '}}'", source),
            character => {
                literal.push(character);
                position += 1;
            }
        }
    }

    if !literal.is_empty() {
        parts.push(TemplatePart::Literal(literal));
    }
    Ok(parts)
}

fn selected_fields<'a>(fields: &[&'a str], selector: &FieldSelector) -> Vec<&'a str> {
    match selector {
        FieldSelector::Single(index) => fields.get(index - 1).copied().into_iter().collect(),
        FieldSelector::Range { start, end } => {
            let first = start - 1;
            let last = end.unwrap_or(fields.len()).min(fields.len());
            if first >= last {
                Vec::new()
            } else {
                fields[first..last].to_vec()
            }
        }
    }
}

fn query_line(prompt: &str, query: &str, width: usize) -> String {
    let prefix = format!(" {}", prompt);
    let prefix = clip(&prefix, width);
    let available = width.saturating_sub(UnicodeWidthStr::width(prefix.as_str()));
    format!("{}{}", prefix, clip_tail(query, available))
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

fn clip_tail(text: &str, width: usize) -> String {
    if width == 0 || UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    if width <= 3 {
        return text
            .chars()
            .rev()
            .take(width)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
    }

    let mut result = String::new();
    let mut used = 0;
    for character in text.chars().rev() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width - 3 {
            break;
        }
        result.push(character);
        used += character_width;
    }
    let tail = result.chars().rev().collect::<String>();
    format!("...{}", tail)
}

fn sanitize_for_display(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut escape = false;
    let mut csi = false;
    let mut osc = false;
    let mut osc_escape = false;

    for character in text.chars() {
        if osc {
            if osc_escape {
                osc_escape = false;
                if character == '\\' {
                    osc = false;
                }
            } else if character == '\u{7}' {
                osc = false;
            } else if character == '\u{1b}' {
                osc_escape = true;
            }
            continue;
        }
        if csi {
            if ('@'..='~').contains(&character) {
                csi = false;
            }
            continue;
        }
        if escape {
            escape = false;
            match character {
                '[' => csi = true,
                ']' => osc = true,
                _ => {}
            }
            continue;
        }
        if character == '\u{1b}' {
            escape = true;
            continue;
        }
        if character.is_control() {
            if character == '\t' {
                output.push(' ');
            }
            continue;
        }
        output.push(character);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(text: &str) -> Candidate {
        Candidate {
            raw: text.as_bytes().to_vec(),
            text: text.to_string(),
            metadata: Value::Object(Map::new()),
            index: 0,
        }
    }

    #[test]
    fn defaults_to_tab_delimiter() {
        assert_eq!(
            parse_delimiter(None).unwrap(),
            FieldDelimiter::Character('\t')
        );
        let format = FieldFormat::parse(Some("2")).unwrap().unwrap();
        assert_eq!(
            format.render("1\tFirst", FieldDelimiter::Character('\t'), " "),
            "First"
        );
    }

    #[test]
    fn whitespace_delimiter_collapses_runs() {
        let format = FieldFormat::parse(Some("2,3")).unwrap().unwrap();
        assert_eq!(
            format.render(
                "USER           123  command",
                FieldDelimiter::Whitespace,
                " "
            ),
            "123 command"
        );
    }

    #[test]
    fn parses_comma_fields_and_ranges() {
        let format = FieldFormat::parse(Some("2,4")).unwrap().unwrap();
        assert_eq!(
            format.render("a\tb\tc\td", FieldDelimiter::Character('\t'), " "),
            "b d"
        );
        let format = FieldFormat::parse(Some("{2..}")).unwrap().unwrap();
        assert_eq!(
            format.render("a\tb\tc", FieldDelimiter::Character('\t'), " "),
            "b c"
        );
    }

    #[test]
    fn parses_free_format_templates() {
        let format = FieldFormat::parse(Some("PID={1} CMD={2}"))
            .unwrap()
            .unwrap();
        assert_eq!(
            format.render("123\tinit", FieldDelimiter::Character('\t'), " "),
            "PID=123 CMD=init"
        );
    }

    #[test]
    fn zero_disables_a_field_format() {
        assert!(FieldFormat::parse(Some("0")).unwrap().is_none());
    }

    #[test]
    fn display_and_match_projections_are_independent() {
        let options = Options {
            prompt: "> ".to_string(),
            lines: None,
            initial: String::new(),
            index: false,
            dmenu0: false,
            display: DisplayType::Text,
            with_nth: Some("1".to_string()),
            accept_nth: Some("2".to_string()),
            match_nth: Some("3".to_string()),
            nth_delimiter: None,
        };
        let app = DmenuApp::new(
            vec![candidate("123\t456\t789")],
            options,
            FieldFormat::parse(Some("1")).unwrap(),
            FieldFormat::parse(Some("2")).unwrap(),
            FieldFormat::parse(Some("3")).unwrap(),
            FieldDelimiter::Character('\t'),
            RecordSeparator::Newline,
        );
        assert_eq!(app.display_text(&app.candidates[0]), "123");
        assert_eq!(app.match_text(&app.candidates[0]), "789");
        assert_eq!(app.output_for(0), b"456");
    }

    #[test]
    fn preserves_original_record_bytes() {
        let candidates = parse_candidates(b"foo\tbar\n", RecordSeparator::Newline);
        assert_eq!(candidates[0].raw, b"foo\tbar");
        assert_eq!(candidates[0].index, 0);
    }

    #[test]
    fn parses_nul_separated_records() {
        let candidates = parse_candidates(b"one\0two\0", RecordSeparator::Nul);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[1].raw, b"two");
        assert_eq!(candidates[1].index, 1);
        assert!(candidates[1].metadata.as_object().unwrap().is_empty());
    }

    #[test]
    fn parses_rofi_metadata_without_a_protocol_namespace() {
        let candidates = parse_candidates(
            b"Firefox\0icon\x1ffirefox,web-browser\0urgent\x1ftrue\n",
            RecordSeparator::Newline,
        );
        assert_eq!(candidates[0].raw, b"Firefox");
        assert_eq!(candidates[0].text, "Firefox");
        assert_eq!(candidates[0].metadata["icon"], "firefox,web-browser");
        assert_eq!(candidates[0].metadata["urgent"], "true");
    }

    #[test]
    fn validates_field_delimiters() {
        assert_eq!(
            parse_delimiter(Some(":")).unwrap(),
            FieldDelimiter::Character(':')
        );
        assert_eq!(
            parse_delimiter(Some("whitespace")).unwrap(),
            FieldDelimiter::Whitespace
        );
        assert!(parse_delimiter(Some("::")).is_err());
        assert!(parse_delimiter(Some("é")).is_err());
    }
}
