use std::collections::VecDeque;

use regex::RegexBuilder;

use super::view::{BufferLine, BufferReadPage, BufferReadRequest, BufferView};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BufferReadError {
    InvalidRegex { pattern: String, message: String },
}

impl BufferReadError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::InvalidRegex { .. } => "INVALID_REGEX",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferStoreStats {
    pub total_lines_seen: usize,
    pub retained_lines: usize,
    pub dropped_lines: usize,
    pub total_bytes_seen: usize,
    pub retained_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredLine {
    line_number: usize,
    ansi: String,
    plain: String,
    raw: String,
    byte_len: usize,
}

#[derive(Debug)]
pub struct BufferStore {
    max_lines: usize,
    lines: VecDeque<StoredLine>,
    pending_bytes: Vec<u8>,
    pending_line_number: Option<usize>,
    next_line_number: usize,
    total_bytes_seen: usize,
    retained_bytes: usize,
    dropped_lines: usize,
}

impl BufferStore {
    pub fn new(max_lines: usize) -> Self {
        Self {
            max_lines: max_lines.max(1),
            lines: VecDeque::new(),
            pending_bytes: Vec::new(),
            pending_line_number: None,
            next_line_number: 1,
            total_bytes_seen: 0,
            retained_bytes: 0,
            dropped_lines: 0,
        }
    }

    pub fn append_bytes(&mut self, chunk: &[u8]) {
        self.total_bytes_seen += chunk.len();
        for &byte in chunk {
            self.ensure_pending_line();
            if byte == b'\n' {
                self.finalize_pending_line();
                continue;
            }
            self.pending_bytes.push(byte);
            self.retained_bytes += 1;
        }
        self.enforce_retention();
    }

    pub fn read(&self, request: &BufferReadRequest) -> Result<BufferReadPage, BufferReadError> {
        let filtered = self.filter_lines(request)?;
        let start = request.offset.min(filtered.len());
        let limit = request.limit;
        let end = start.saturating_add(limit).min(filtered.len());
        let selected = &filtered[start..end];

        let lines = selected
            .iter()
            .map(|line| BufferLine {
                line_number: line.line_number,
                text: line.text_for_view(request.view),
            })
            .collect::<Vec<_>>();

        Ok(BufferReadPage {
            offset: start,
            returned: lines.len(),
            has_more: end < filtered.len(),
            total_lines: self.total_retained_lines(),
            lines,
        })
    }

    pub fn stats(&self) -> BufferStoreStats {
        BufferStoreStats {
            total_lines_seen: self.next_line_number.saturating_sub(1),
            retained_lines: self.total_retained_lines(),
            dropped_lines: self.dropped_lines,
            total_bytes_seen: self.total_bytes_seen,
            retained_bytes: self.retained_bytes,
        }
    }

    pub fn max_lines(&self) -> usize {
        self.max_lines
    }

    pub fn set_max_lines(&mut self, max_lines: usize) {
        self.max_lines = max_lines.max(1);
        self.enforce_retention();
    }

    fn ensure_pending_line(&mut self) {
        if self.pending_line_number.is_none() {
            self.pending_line_number = Some(self.next_line_number);
            self.next_line_number += 1;
        }
    }

    fn finalize_pending_line(&mut self) {
        let Some(line_number) = self.pending_line_number.take() else {
            return;
        };

        let mut line_bytes = std::mem::take(&mut self.pending_bytes);
        if line_bytes.last() == Some(&b'\r') {
            line_bytes.pop();
            self.retained_bytes = self.retained_bytes.saturating_sub(1);
        }

        let ansi = String::from_utf8_lossy(&line_bytes).into_owned();
        let plain = String::from_utf8_lossy(&strip_ansi_escapes::strip(&line_bytes)).into_owned();
        let raw = bytes_to_raw_string(&line_bytes);
        let byte_len = line_bytes.len();

        self.lines.push_back(StoredLine {
            line_number,
            ansi,
            plain,
            raw,
            byte_len,
        });

        self.enforce_retention();
    }

    fn enforce_retention(&mut self) {
        while self.total_retained_lines() > self.max_lines {
            if let Some(dropped) = self.lines.pop_front() {
                self.dropped_lines += 1;
                self.retained_bytes = self.retained_bytes.saturating_sub(dropped.byte_len);
                continue;
            }

            if self.pending_line_number.take().is_some() {
                self.dropped_lines += 1;
                self.retained_bytes = self.retained_bytes.saturating_sub(self.pending_bytes.len());
                self.pending_bytes.clear();
            } else {
                break;
            }
        }
    }

    fn total_retained_lines(&self) -> usize {
        self.lines.len() + usize::from(self.pending_line_number.is_some())
    }

    fn filter_lines<'a>(
        &'a self,
        request: &BufferReadRequest,
    ) -> Result<Vec<ReadLineRef<'a>>, BufferReadError> {
        let mut lines = self
            .lines
            .iter()
            .map(ReadLineRef::from_stored)
            .collect::<Vec<_>>();

        if let Some(line_number) = self.pending_line_number {
            lines.push(ReadLineRef::from_pending(line_number, &self.pending_bytes));
        }

        let Some(pattern) = request.pattern.as_deref() else {
            return Ok(lines);
        };

        let regex = RegexBuilder::new(pattern)
            .case_insensitive(request.ignore_case)
            .build()
            .map_err(|error| BufferReadError::InvalidRegex {
                pattern: pattern.to_string(),
                message: error.to_string(),
            })?;

        Ok(lines
            .into_iter()
            .filter(|line| regex.is_match(&line.text_for_view(request.view)))
            .collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReadLineRef<'a> {
    line_number: usize,
    ansi: std::borrow::Cow<'a, str>,
    plain: std::borrow::Cow<'a, str>,
    raw: std::borrow::Cow<'a, str>,
}

impl<'a> ReadLineRef<'a> {
    fn from_stored(line: &'a StoredLine) -> Self {
        Self {
            line_number: line.line_number,
            ansi: std::borrow::Cow::Borrowed(line.ansi.as_str()),
            plain: std::borrow::Cow::Borrowed(line.plain.as_str()),
            raw: std::borrow::Cow::Borrowed(line.raw.as_str()),
        }
    }

    fn from_pending(line_number: usize, bytes: &'a [u8]) -> Self {
        let ansi = String::from_utf8_lossy(bytes).into_owned();
        let plain = String::from_utf8_lossy(&strip_ansi_escapes::strip(bytes)).into_owned();
        let raw = bytes_to_raw_string(bytes);
        Self {
            line_number,
            ansi: std::borrow::Cow::Owned(ansi),
            plain: std::borrow::Cow::Owned(plain),
            raw: std::borrow::Cow::Owned(raw),
        }
    }

    fn text_for_view(&self, view: BufferView) -> String {
        match view {
            BufferView::Plain => self.plain.to_string(),
            BufferView::Ansi => self.ansi.to_string(),
            BufferView::Raw => self.raw.to_string(),
        }
    }
}

fn bytes_to_raw_string(bytes: &[u8]) -> String {
    let mut output = String::new();
    for &byte in bytes {
        for escaped in std::ascii::escape_default(byte) {
            output.push(char::from(escaped));
        }
    }
    output
}
