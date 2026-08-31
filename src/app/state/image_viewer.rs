//! 图片查看器状态。

use std::sync::atomic::AtomicBool;

use super::super::media::{ImageAction, ImageSource};

/// 图片查看器状态（通过 Arc<Mutex<>> 在主窗口和 viewport 间共享）
#[derive(Debug)]
pub struct ImageViewerState {
    /// 当前图片 URL
    pub url: String,
    /// 当前会话中可连续浏览的图片 URL。
    pub images: Vec<String>,
    /// 与 `images` 对齐的来源信息，用于复制、保存和在聊天中定位。
    pub sources: Vec<ImageSource>,
    /// 当前图片在 images 中的位置。
    pub image_index: usize,
    /// 缩放比例 (1.0 = 适应窗口)
    pub zoom: f32,
    /// 平移偏移量（像素）
    pub pan_offset: egui::Vec2,
    /// 窗口已关闭
    pub closed: AtomicBool,
    /// 适应窗口的基础缩放比例（渲染时更新）
    pub base_scale: f32,
    /// 是否请求 1:1 原始尺寸
    pub request_original_size: bool,
    /// viewport 只写入动作，主应用在下一帧统一处理副作用。
    pub pending_action: Option<ImageAction>,
}

impl ImageViewerState {
    pub fn new(url: String) -> Self {
        Self::with_images(url.clone(), vec![url])
    }

    pub fn with_images(url: String, mut images: Vec<String>) -> Self {
        if images.is_empty() {
            images.push(url.clone());
        }
        let image_index = images.iter().position(|item| item == &url).unwrap_or(0);
        let url = images[image_index].clone();
        let sources = images.iter().cloned().map(ImageSource::url).collect();
        Self {
            url,
            images,
            sources,
            image_index,
            zoom: 1.0,
            pan_offset: egui::Vec2::ZERO,
            closed: AtomicBool::new(false),
            base_scale: 1.0,
            request_original_size: false,
            pending_action: None,
        }
    }

    pub fn with_sources(current: ImageSource, mut sources: Vec<ImageSource>) -> Self {
        if sources.is_empty() {
            sources.push(current.clone());
        }
        let image_index = sources
            .iter()
            .position(|source| source == &current)
            .or_else(|| sources.iter().position(|source| source.url == current.url))
            .unwrap_or(0);
        let images = sources.iter().map(|source| source.url.clone()).collect();
        let url = sources[image_index].url.clone();
        Self {
            url,
            images,
            sources,
            image_index,
            zoom: 1.0,
            pan_offset: egui::Vec2::ZERO,
            closed: AtomicBool::new(false),
            base_scale: 1.0,
            request_original_size: false,
            pending_action: None,
        }
    }

    pub fn current_source(&self) -> ImageSource {
        self.sources
            .get(self.image_index)
            .cloned()
            .unwrap_or_else(|| ImageSource::url(self.url.clone()))
    }

    pub fn navigate(&mut self, offset: isize) -> bool {
        let next_index = self.image_index as isize + offset;
        if !(0..self.images.len() as isize).contains(&next_index) {
            return false;
        }
        self.image_index = next_index as usize;
        self.url = self.images[self.image_index].clone();
        self.fit_to_window();
        self.request_original_size = false;
        true
    }

    /// 适应窗口大小（重置缩放和偏移）
    pub fn fit_to_window(&mut self) {
        self.zoom = 1.0;
        self.pan_offset = egui::Vec2::ZERO;
    }

    /// 放大 20%
    pub fn zoom_in(&mut self) {
        self.zoom = (self.zoom * 1.2).min(20.0);
    }

    /// 缩小 20%
    pub fn zoom_out(&mut self) {
        self.zoom = (self.zoom / 1.2).max(0.05);
    }

    /// 缩放百分比文本（相对于原始像素大小）
    pub fn zoom_percent_text(&self) -> String {
        format!("{:.0}%", self.base_scale * self.zoom * 100.0)
    }
}

#[cfg(test)]
mod tests {
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
}
