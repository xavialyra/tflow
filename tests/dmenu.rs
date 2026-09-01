mod support;

use std::fmt::Write as _;
use support::{run_dmenu, run_dmenu_steps, run_dmenu_steps_waiting_for_text, run_tty_dmenu};

#[test]
fn accepts_a_selected_line_without_terminal_bytes_on_stdout() {
    let result = run_dmenu(&[], b"first\nsecond\n", b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"first\n");
}

#[test]
fn tty_stdin_is_an_empty_candidate_source_for_free_text_input() {
    let result = run_tty_dmenu(b"typed value\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"typed value\n");
}

#[test]
fn candidate_json_can_exceed_the_default_script_output_limit() {
    let mut input = String::new();
    for index in 0..30_000 {
        writeln!(&mut input, "candidate-{index:05}").unwrap();
    }

    let result = run_dmenu(&[], input.as_bytes(), b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"candidate-00000\n");
}

#[test]
fn supports_nul_records_and_index_output() {
    let result = run_dmenu(&["--dmenu0", "--index"], b"first\0second\0", b"\x1b[B\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"1\0");
}

#[test]
fn accept_nth_returns_the_selected_projection() {
    let result = run_dmenu(&["--accept-nth=2"], b"id\tlabel\n", b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"label\n");
}

#[test]
fn accept_nth_supports_templates_and_whitespace_fields() {
    let result = run_dmenu(
        &[
            "--accept-nth=USER={1} PID={2} CMD={3}",
            "--nth-delimiter=whitespace",
        ],
        b"alice   123   init\n",
        b"\r",
    );

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"USER=alice PID=123 CMD=init\n");
}

#[test]
fn accept_nth_supports_open_field_ranges() {
    let result = run_dmenu(&["--accept-nth={2..}"], b"id\tfirst\tsecond\n", b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"first\tsecond\n");
}

#[test]
fn rofi_metadata_is_not_written_with_the_selected_record() {
    let result = run_dmenu(
        &[],
        b"Firefox\0icon\x1ffirefox,web-browser\0urgent\x1ftrue\n",
        b"\r",
    );

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"Firefox\n");
}

#[test]
fn an_empty_record_can_be_selected() {
    let result = run_dmenu(&[], b"\n", b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"\n");
}

#[test]
fn empty_accept_keeps_processing_following_input() {
    let result = run_dmenu(&[], b"", b"\rvalue\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"value\n");
}

#[test]
fn returns_an_unmatched_query() {
    let result = run_dmenu(&[], b"first\nsecond\n", b"missing\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"missing\n");
}

#[test]
fn route_like_input_remains_a_dmenu_query() {
    let result = run_dmenu_steps(&[], b"first\n", &[&b"app terminal"[..], &b"\r"[..]]);

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"app terminal\n");
}

#[test]
fn dmenu_does_not_inherit_the_default_views_tab_binding() {
    let result = run_dmenu_steps(&[], b"first\nsecond\n", &[&b"\t"[..], &b"\r"[..]]);

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"first\n");
}

#[test]
fn dmenu_can_cancel_the_global_command_selector_and_continue() {
    let result = run_dmenu_steps_waiting_for_text(
        &[],
        b"first\nsecond\n",
        &[&b"\x0b"[..], &b"\x1b"[..], &b"\r"[..]],
        &["dmenu", "commands", "dmenu"],
    );

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"first\n");
}

#[test]
fn dirty_input_reconciles_before_accepting() {
    let result = run_dmenu(&[], b"alpha\nbeta\n", b"a\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"alpha\n");
}

#[test]
fn text_selection_and_accept_share_one_input_batch() {
    let result = run_dmenu(&[], b"alpha\nbeta\n", b"a\x1b[B\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"beta\n");
}

#[test]
fn match_projection_keeps_the_original_source_index() {
    let result = run_dmenu(
        &["--match-nth=2", "--index"],
        b"0\talpha\n1\tbeta\n",
        b"beta\r",
    );

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"1\n");
}

#[test]
fn initial_query_filters_the_configured_items() {
    let result = run_dmenu(&["--initial=second"], b"first\nsecond\n", b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"second\n");
}

#[test]
fn initial_query_strips_terminal_control_sequences() {
    let result = run_dmenu(&["--initial=red\x1b[31m"], b"first\n", b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"red\n");
}

#[test]
fn selected_candidate_preserves_non_utf8_bytes() {
    let result = run_dmenu(&[], &[0xff, b'\n'], b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, [0xff, b'\n']);
}

#[test]
fn cancel_with_a_nonempty_query_returns_nonzero_without_stdout() {
    let result = run_dmenu(&[], b"first\n", b"query\x1b");

    assert_ne!(result.status, 0);
    assert!(result.stdout.is_empty());
}

#[test]
fn cancel_returns_nonzero_without_stdout() {
    let result = run_dmenu(&[], b"first\n", b"\x1b");

    assert_ne!(result.status, 0);
    assert!(result.stdout.is_empty());
}
