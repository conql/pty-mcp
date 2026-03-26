#[path = "../src/buffer/store.rs"]
mod store;
#[path = "../src/buffer/view.rs"]
mod view;

use store::BufferStore;
use view::{BufferReadRequest, BufferView};

#[test]
fn append_and_read_preserve_order_and_line_numbers() {
    let mut store = BufferStore::new(32);
    store.append_bytes(b"one\ntwo\nthree\n");

    let page = store
        .read(&BufferReadRequest::new(10))
        .expect("buffer read should succeed");

    assert_eq!(page.returned, 3);
    assert_eq!(page.total_lines, 3);
    assert_eq!(
        page.lines
            .iter()
            .map(|line| (line.line_number, line.text.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "one"), (2, "two"), (3, "three")]
    );
}

#[test]
fn regex_filter_and_view_modes_work_together() {
    let mut store = BufferStore::new(32);
    store.append_bytes(b"\x1b[31mError\x1b[0m happened\nINFO done\nraw\x03\n");

    let mut plain_read = BufferReadRequest::new(10);
    plain_read.pattern = Some("error".to_string());
    plain_read.ignore_case = true;
    plain_read.view = BufferView::Plain;
    let plain_page = store.read(&plain_read).expect("plain filter should work");
    assert_eq!(plain_page.returned, 1);
    assert_eq!(plain_page.lines[0].text, "Error happened");

    let mut ansi_read = BufferReadRequest::new(10);
    ansi_read.pattern = Some("Error".to_string());
    ansi_read.view = BufferView::Ansi;
    let ansi_page = store.read(&ansi_read).expect("ansi filter should work");
    assert_eq!(ansi_page.returned, 1);
    assert!(ansi_page.lines[0].text.contains("\x1b[31m"));

    let mut raw_read = BufferReadRequest::new(10);
    raw_read.pattern = Some(r"\\x03".to_string());
    raw_read.view = BufferView::Raw;
    let raw_page = store.read(&raw_read).expect("raw filter should work");
    assert_eq!(raw_page.returned, 1);
    assert_eq!(raw_page.lines[0].text, "raw\\x03");
}

#[test]
fn invalid_regex_is_reported_as_invalid_regex_error() {
    let mut store = BufferStore::new(32);
    store.append_bytes(b"alpha\nbeta\n");

    let mut request = BufferReadRequest::new(10);
    request.pattern = Some("(".to_string());

    let error = store.read(&request).expect_err("invalid regex must fail");
    assert_eq!(error.error_code(), "INVALID_REGEX");
}

#[test]
fn retention_trims_old_lines_and_keeps_monotonic_numbering() {
    let mut store = BufferStore::new(3);
    assert_eq!(store.max_lines(), 3);
    store.append_bytes(b"l1\nl2\nl3\n");
    store.set_max_lines(3);
    store.append_bytes(b"l4\n");
    store.append_bytes(b"l5");

    let page = store
        .read(&BufferReadRequest::new(10))
        .expect("buffer read should succeed");

    assert_eq!(page.total_lines, 3);
    assert_eq!(
        page.lines
            .iter()
            .map(|line| (line.line_number, line.text.as_str()))
            .collect::<Vec<_>>(),
        vec![(3, "l3"), (4, "l4"), (5, "l5")]
    );

    let stats = store.stats();
    assert_eq!(stats.total_lines_seen, 5);
    assert_eq!(stats.dropped_lines, 2);
}
