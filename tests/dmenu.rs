mod support;

use support::run_dmenu;

#[test]
fn accepts_a_selected_line_without_terminal_bytes_on_stdout() {
    let result = run_dmenu(&[], b"first\nsecond\n", b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"first\n");
}

#[test]
fn supports_nul_records_and_index_output() {
    let result = run_dmenu(&["--dmenu0", "--index"], b"first\0second\0", b"\x1b[B\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"1\0");
}

#[test]
fn accept_nth_returns_the_selected_projection() {
    let result = run_dmenu(&["--accept-nth", "2"], b"id\tlabel\n", b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"label\n");
}

#[test]
fn cancel_returns_nonzero_without_stdout() {
    let result = run_dmenu(&[], b"first\n", b"\x1b");

    assert_ne!(result.status, 0);
    assert!(result.stdout.is_empty());
}
