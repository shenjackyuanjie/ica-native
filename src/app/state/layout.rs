//! 消息行布局缓存所需的键与测量结果。
//!
//! 虚拟列表需要“绘制前就知道总高度”，因此消息行先估算、绘制后实测并回填；
//! 这两个类型就是这套缓存的载体：键决定缓存是否仍然有效，结果是每行的
//! 位置与高度。窗口宽度、外观模式等变化都会让键失配，从而整体重排。

/// 布局缓存键：任一字段变化都意味着旧布局作废。
#[derive(Debug, Clone, Copy)]
pub struct MessageLayoutCacheKey {
    /// 生成布局时的消息区域宽度；容差比较见 `matches`。
    pub width: f32,
    /// 是否处于纯文字模式（无气泡、无头像）。
    pub pure_text_mode: bool,
    /// 是否显示群消息头像。
    pub show_message_avatar: bool,
    /// 多选转发模式是否激活（行首多出勾选框）。
    pub forward_mode_active: bool,
}

impl MessageLayoutCacheKey {
    /// 判断另一个键是否与当前布局兼容。
    ///
    /// 宽度允许 8 像素以内的误差：拖拽窗口时逐像素重算过于昂贵，
    /// 微小差异对行高没有可感知影响。
    pub fn matches(self, other: Self) -> bool {
        (self.width - other.width).abs() <= 8.0
            && self.pure_text_mode == other.pure_text_mode
            && self.show_message_avatar == other.show_message_avatar
            && self.forward_mode_active == other.forward_mode_active
    }
}

/// 单条消息行的布局结果（相对消息区域顶部的坐标）。
#[derive(Debug, Clone, Copy)]
pub struct MessageRowLayout {
    /// 行顶部坐标。
    pub top: f32,
    /// 行高（实测值）。
    pub height: f32,
}
