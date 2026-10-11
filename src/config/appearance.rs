//! 聊天界面的持久化外观与交互选项。
//!
//! 只保存数据（`ChatAppearanceSettings`）与枚举定义；对应的 egui 渲染与读写
//! 入口位于 `app::settings` 与聊天视图。选项命名与 Icalingua++ 的设置项对齐，
//! 便于两边配置互译。

use serde::{Deserialize, Serialize};

/// 与本体一致的发送快捷键；未配置时保留 Enter 发送。
#[derive(Debug, Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
pub enum MessageSendKey {
    /// Enter 发送，Shift/Ctrl+Enter 换行。
    #[default]
    Enter,
    /// Ctrl+Enter 发送，Enter 换行。
    CtrlEnter,
    /// Shift+Enter 发送，Enter 换行。
    ShiftEnter,
}

impl MessageSendKey {
    /// 返回该快捷键的人类可读说明，供设置界面 hint 文本使用。
    pub fn hint(self) -> &'static str {
        match self {
            Self::Enter => "Enter 发送，Shift/Ctrl+Enter 换行",
            Self::CtrlEnter => "Ctrl+Enter 发送，Enter 换行",
            Self::ShiftEnter => "Shift+Enter 发送，Enter 换行",
        }
    }
}

/// 聊天界面的持久化外观选项。此类型只保存数据，egui 渲染位于 `app::settings`。
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ChatAppearanceSettings {
    /// 发送消息的快捷键（与本体一致的三种模式）。
    #[serde(default)]
    pub key_to_send_message: MessageSendKey,
    /// 是否在聊天消息中隐藏图片（不加载、不占位）。
    #[serde(default)]
    pub hide_chat_img: bool,
    /// 是否在聊天消息中隐藏视频。
    #[serde(default)]
    pub hide_chat_video: bool,
    /// 是否禁用超级表情（大图表情）。
    #[serde(default)]
    pub disable_super_face: bool,
    /// 是否禁用聊天中的图片替换（如表情按图片大小适配的替换行为）。
    #[serde(default)]
    pub disable_img_swap_in_chat: bool,
    /// 是否停用聊天分组功能（分组数据仍保留，仅不生效）。
    #[serde(default)]
    pub disable_chat_group: bool,
    /// 是否不显示分组里的未读圆点。
    #[serde(default)]
    pub disable_chat_group_dot: bool,
    /// 是否不高亮消息中的 URL 链接。
    #[serde(default)]
    pub disable_highlight_url: bool,
    /// 是否使用内置图片查看器（否则交给系统默认程序）。
    #[serde(default)]
    pub use_local_image_viewer: bool,
    /// 是否禁用自适应单面板模式（窄窗口下把会话列表与聊天区合并为一栏）。
    #[serde(default = "default_true")]
    pub disable_adaptive_single_panel_mode: bool,
    /// 是否移除群名里的 emoji，让列表更紧凑。
    #[serde(default)]
    pub remove_emoji_in_group_name: bool,
    /// 是否按修改时间排序收藏表情（否则按名称）。
    #[serde(default = "default_true")]
    pub sort_stickers_by_time: bool,
    /// 是否禁用图片查看器的触摸手势（触屏设备上避免误操作）。
    #[serde(default)]
    pub disable_image_viewer_touch_gestures: bool,
    /// 查看消息时是否做「盘古之白」处理（中西文之间补空格）。
    #[serde(default)]
    pub use_pangu_to_view_msg: bool,
    /// 发送消息时是否做「盘古之白」处理。
    #[serde(default)]
    pub use_pangu_to_send_msg: bool,
    /// 是否禁用发送时的文件类型选择（直接按路径上传）。
    #[serde(default)]
    pub disable_file_type_selection: bool,
    /// 是否启用群话题按钮。
    #[serde(default = "default_true")]
    pub enable_topic_button: bool,
    /// 是否隐藏群聊中其他成员的头像。
    #[serde(default)]
    pub hide_group_member_avatar: bool,
    /// 在群聊中显示其他成员的消息头像。
    #[serde(default = "default_true")]
    pub show_message_avatar: bool,
    /// 是否使用高对比度聊天配色。
    #[serde(default)]
    pub high_contrast_chat: bool,
    /// 仅隐藏最左侧的聊天分组导航，不改变会话列表和分组数据。
    #[serde(default)]
    pub hide_chat_group_sidebar: bool,
    /// 选中会话时是否自动标记为已读。
    #[serde(default = "default_true")]
    pub auto_read_on_select: bool,
}

/// `default_true`：serde 缺省值与 `Default` 实现共用，保证两边一致。
const fn default_true() -> bool {
    true
}

/// `Default` 实现：与各字段 `#[serde(default)]` 的取值保持一致。
impl Default for ChatAppearanceSettings {
    fn default() -> Self {
        Self {
            key_to_send_message: MessageSendKey::default(),
            hide_chat_img: false,
            hide_chat_video: false,
            disable_super_face: false,
            disable_img_swap_in_chat: false,
            disable_chat_group: false,
            disable_chat_group_dot: false,
            disable_highlight_url: false,
            use_local_image_viewer: false,
            disable_adaptive_single_panel_mode: true,
            remove_emoji_in_group_name: false,
            sort_stickers_by_time: true,
            disable_image_viewer_touch_gestures: false,
            use_pangu_to_view_msg: false,
            use_pangu_to_send_msg: false,
            disable_file_type_selection: false,
            enable_topic_button: true,
            hide_group_member_avatar: false,
            show_message_avatar: true,
            high_contrast_chat: false,
            hide_chat_group_sidebar: false,
            auto_read_on_select: true,
        }
    }
}
