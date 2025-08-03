use egui::{Grid, Ui};

/// 定制聊天界面选项
pub struct CustomChat {
    /// 隐藏聊天图片
    pub hide_chat_img: bool,
    /// 隐藏聊天视频
    pub hide_chat_video: bool,
    /// 禁用超级表情
    pub disable_super_face: bool,
    /// 禁用同会话多图切换
    pub disable_img_swap_in_chat: bool,
    /// 禁用聊天分组
    pub disable_chat_group: bool,
    /// 禁用聊天分组的红点
    pub disable_chat_group_dot: bool,
    /// 禁用高亮 url
    pub disable_highlight_url: bool,
    /// 使用本地看图器 (todo?)
    pub use_local_image_viewer: bool,
    /// 禁用自适应单面板模式
    /// 宽度较低的时候启用
    pub disable_adaptive_single_panel_mode: bool,
    /// 移除群名内表情
    pub remove_emoji_in_group_name: bool,
    /// 时间倒序排列 stickers
    pub sort_stickers_by_time: bool,
    /// 禁用图片查看器触摸板新手势 (todo)
    pub disable_image_viewer_touch_gestures: bool,
    /// 查看消息时使用 Pangu.rs
    /// 在中英文间添加空格
    pub use_pangu_to_view_msg: bool,
    /// 发送消息时使用 Pangu.rs
    /// 不包括 +1 消息
    pub use_pangu_to_send_msg: bool,
    /// 禁用文件类型选择框
    /// 拖拽复制默认识别媒体
    pub disable_file_type_selection: bool,
}

impl CustomChat {
    pub fn show_ui(&mut self, ui: &mut Ui) {
        Grid::new("custom_chat_grid_two_columns")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                ui.label("隐藏聊天图片");
                let _ = ui.checkbox(&mut self.hide_chat_img, "");
                ui.end_row();

                ui.label("隐藏聊天视频");
                let _ = ui.checkbox(&mut self.hide_chat_video, "");
                ui.end_row();

                ui.label("禁用超级表情");
                let _ = ui.checkbox(&mut self.disable_super_face, "");
                ui.end_row();

                ui.label("禁用同会话多图切换");
                let _ = ui.checkbox(&mut self.disable_img_swap_in_chat, "");
                ui.end_row();

                ui.label("禁用聊天分组");
                let _ = ui.checkbox(&mut self.disable_chat_group, "");
                ui.end_row();

                ui.label("禁用聊天分组的红点");
                let _ = ui.checkbox(&mut self.disable_chat_group_dot, "");
                ui.end_row();

                ui.label("禁用高亮 URL");
                let _ = ui.checkbox(&mut self.disable_highlight_url, "");
                ui.end_row();

                ui.label("使用本地看图器");
                let _ = ui.checkbox(&mut self.use_local_image_viewer, "");
                ui.end_row();

                ui.label("禁用自适应单面板模式");
                let _ = ui.checkbox(&mut self.disable_adaptive_single_panel_mode, "");
                ui.end_row();

                ui.label("移除群名内表情");
                let _ = ui.checkbox(&mut self.remove_emoji_in_group_name, "");
                ui.end_row();

                ui.label("时间倒序排列 stickers");
                let _ = ui.checkbox(&mut self.sort_stickers_by_time, "");
                ui.end_row();

                ui.label("禁用图片查看器触摸板手势");
                let _ = ui.checkbox(&mut self.disable_image_viewer_touch_gestures, "");
                ui.end_row();

                ui.vertical(|ui| {
                    ui.label("查看消息时使用 Pangu.rs");
                    ui.weak("中英文自动加空格");
                });
                let _ = ui.checkbox(&mut self.use_pangu_to_view_msg, "");
                // ui.end_row();
                // ui.label("");
                ui.end_row();

                ui.vertical(|ui| {
                    ui.label("发送消息时使用 Pangu.rs");
                    ui.weak("不包括 +1");
                });
                let _ = ui.checkbox(&mut self.use_pangu_to_send_msg, "");
                // ui.end_row();
                // ui.label("");
                ui.end_row();

                ui.vertical(|ui| {
                    ui.label("禁用文件类型选择框");
                    ui.weak("拖拽复制默认识别媒体");
                });
                let _ = ui.checkbox(&mut self.disable_file_type_selection, "");
                // ui.end_row();
                // ui.label("");
                ui.end_row();
            });
    }
}

impl Default for CustomChat {
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
        }
    }
}
