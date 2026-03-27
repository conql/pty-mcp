#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BufferView {
    #[default]
    Plain,
    Ansi,
    Raw,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferReadRequest {
    pub offset: usize,
    pub limit: usize,
    pub pattern: Option<String>,
    pub ignore_case: bool,
    pub view: BufferView,
}

impl BufferReadRequest {
    pub fn new(limit: usize) -> Self {
        Self {
            offset: 0,
            limit,
            pattern: None,
            ignore_case: false,
            view: BufferView::Plain,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferLine {
    pub line_number: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferReadPage {
    pub offset: usize,
    pub returned: usize,
    pub has_more: bool,
    pub total_lines: usize,
    pub lines: Vec<BufferLine>,
}
