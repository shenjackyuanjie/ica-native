//! 消息行布局缓存所需的键与测量结果。

#[derive(Debug, Clone, Copy)]
pub struct MessageLayoutCacheKey {
    pub width: f32,
    pub pure_text_mode: bool,
    pub forward_mode_active: bool,
}

impl MessageLayoutCacheKey {
    pub fn matches(self, other: Self) -> bool {
        (self.width - other.width).abs() <= 8.0
            && self.pure_text_mode == other.pure_text_mode
            && self.forward_mode_active == other.forward_mode_active
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MessageRowLayout {
    pub top: f32,
    pub height: f32,
}
