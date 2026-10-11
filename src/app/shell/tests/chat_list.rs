use super::visible_row_range;

#[test]
fn visible_range_is_empty_when_stale_scroll_offset_exceeds_shortened_list() {
    assert_eq!(visible_row_range(2_500.0, 3_000.0, 54.0, 17), 17..17);
}

#[test]
fn visible_range_keeps_overscan_inside_list_bounds() {
    assert_eq!(visible_row_range(108.0, 324.0, 54.0, 20), 0..8);
}
