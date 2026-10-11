//! 编译期内嵌的静态资源。
//!
//! 通过 `include_bytes!` / `include_image!` 把字体、图标与矢量图编入二进制，
//! 避免运行时依赖外部文件路径；APNG 表情是例外，按需从 `assets/face` 读盘并缓存，
//! 见 [`crate::face_data`]。
//!
//! 子模块按资源类型划分，常量名与 `assets/` 目录结构一一对应；
//! 新增资源时在此处登记，不要在使用处直接写 `include_bytes!`。

/// 字体资源：中文界面默认字体与生僻字形兜底字体。
pub mod fonts {
    /// Noto Sans CJK SC Regular，中文界面默认字体。
    pub const FONT_思源黑体: &[u8] = include_bytes!("../assets/fonts/NotoSansCJKsc-Regular.otf");
    /// Unifont 16，渲染主字体缺失的字形（罕见符号等）时的兜底字体。
    pub const FONT_UNIFONT: &[u8] = include_bytes!("../assets/fonts/unifont-16.0.04.otf");
}

/// 位图资源：应用图标。
pub mod png {
    /// 512×512 应用图标，`egui_main` 解码后作为窗口图标。
    pub const ICON_512X: &[u8] = include_bytes!("../assets/png/icon_512x512.png");
}

/// SVG 矢量图标：聊天分组与静音标记。
pub mod svg {
    /// 分组图标，用于分组导航与房间列表。
    pub const CHAT_GROUP: egui::ImageSource =
        egui::include_image!("../assets/svg/chat-group-icon.svg");
    /// 静音图标，标记已静音的会话。
    pub const CHAT_MUTE: egui::ImageSource = egui::include_image!("../assets/svg/chat-mute.svg");
}

/// WebP 动图资源：通知提示。
pub mod webp {
    /// 通知图标，用于新消息提醒。
    pub const NOTIFICATION: egui::ImageSource =
        egui::include_image!("../assets/webp/notification.webp");
}
