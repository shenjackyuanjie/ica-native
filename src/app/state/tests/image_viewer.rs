use super::ImageViewerState;
use crate::app::media::ImageSource;

#[test]
fn image_viewer_navigates_within_gallery_and_resets_transform() {
    let mut viewer = ImageViewerState::with_images(
        "second".to_string(),
        vec!["first".to_string(), "second".to_string()],
    );
    viewer.zoom = 3.0;
    viewer.pan_offset = egui::vec2(12.0, 8.0);

    assert!(viewer.navigate(-1));
    assert_eq!(viewer.url, "first");
    assert_eq!(viewer.zoom, 1.0);
    assert_eq!(viewer.pan_offset, egui::Vec2::ZERO);
    assert!(!viewer.navigate(-1));
}

#[test]
fn image_viewer_keeps_message_location_aligned_while_navigating() {
    let first = ImageSource::message("first".to_string(), -42, "m1".to_string());
    let second = ImageSource::message("second".to_string(), -42, "m2".to_string());
    let mut viewer =
        ImageViewerState::with_sources(second.clone(), vec![first.clone(), second.clone()]);

    assert_eq!(viewer.current_source(), second);
    assert!(viewer.navigate(-1));
    assert_eq!(viewer.current_source(), first);
}
