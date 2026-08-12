use serde::{Deserialize, Serialize};

/// 聊天界面的持久化外观选项。此类型只保存数据，渲染由 GPUI 应用层负责。
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ChatAppearanceSettings {
    #[serde(default)]
    pub hide_chat_img: bool,
    #[serde(default)]
    pub hide_chat_video: bool,
    #[serde(default)]
    pub disable_super_face: bool,
    #[serde(default)]
    pub disable_img_swap_in_chat: bool,
    #[serde(default)]
    pub disable_chat_group: bool,
    #[serde(default)]
    pub disable_chat_group_dot: bool,
    #[serde(default)]
    pub disable_highlight_url: bool,
    #[serde(default)]
    pub use_local_image_viewer: bool,
    #[serde(default = "default_true")]
    pub disable_adaptive_single_panel_mode: bool,
    #[serde(default)]
    pub remove_emoji_in_group_name: bool,
    #[serde(default = "default_true")]
    pub sort_stickers_by_time: bool,
    #[serde(default)]
    pub disable_image_viewer_touch_gestures: bool,
    #[serde(default)]
    pub use_pangu_to_view_msg: bool,
    #[serde(default)]
    pub use_pangu_to_send_msg: bool,
    #[serde(default)]
    pub disable_file_type_selection: bool,
    #[serde(default = "default_true")]
    pub enable_topic_button: bool,
    #[serde(default)]
    pub hide_group_member_avatar: bool,
    #[serde(default)]
    pub high_contrast_chat: bool,
    #[serde(default = "default_true")]
    pub auto_read_on_select: bool,
}

const fn default_true() -> bool {
    true
}

impl Default for ChatAppearanceSettings {
    fn default() -> Self {
        Self {
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
            high_contrast_chat: false,
            auto_read_on_select: true,
        }
    }
}
