use super::*;

fn image() -> Arc<ColorImage> {
    Arc::new(ColorImage::new([1, 1], vec![egui::Color32::WHITE]))
}

fn begin(state: &mut LoaderState, uri: &str, now: Instant) -> u64 {
    match state.prepare_load(uri, now) {
        PrepareLoad::WaitingBytes { generation } => generation,
        _ => panic!("此处应当是等待中的图片"),
    }
}

#[test]
fn active_working_set_can_temporarily_exceed_budget_then_recovers() {
    let now = Instant::now();
    let mut state = LoaderState::new(4);
    let first = begin(&mut state, "first", now);
    assert!(state.mark_decoding("first", first, None));
    assert!(state.complete_ready("first", first, image(), 4, now));

    let second = begin(&mut state, "second", now);
    assert!(state.mark_decoding("second", second, None));
    assert!(state.complete_ready("second", second, image(), 4, now));
    assert_eq!(state.ready.len(), 2);

    let later = now + ACTIVE_IMAGE_GRACE + Duration::from_millis(1);
    assert!(matches!(
        state.prepare_load("second", later),
        PrepareLoad::Ready(_)
    ));
    assert_eq!(state.ready.len(), 1);
    assert!(state.ready.contains("second"));
}
