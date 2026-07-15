use crate::config::ChatAppearanceSettings;
use egui::{Grid, Ui};

impl ChatAppearanceSettings {
    /// 展示 ica 也有的选项
    pub fn show_ica_ui(&mut self, ui: &mut Ui) {
        Grid::new("custom_chat_ica_grid")
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
                ui.end_row();

                ui.vertical(|ui| {
                    ui.label("发送消息时使用 Pangu.rs");
                    ui.weak("不包括 +1");
                });
                let _ = ui.checkbox(&mut self.use_pangu_to_send_msg, "");
                ui.end_row();

                ui.vertical(|ui| {
                    ui.label("禁用文件类型选择框");
                    ui.weak("拖拽复制默认识别媒体");
                });
                let _ = ui.checkbox(&mut self.disable_file_type_selection, "");
                ui.end_row();
            });
    }

    /// 展示 ica-native 特有的选项
    pub fn show_extra_ui(
        &mut self,
        ui: &mut Ui,
        clear_on_select: &mut bool,
        auto_fetch_history_on_select: &mut bool,
        scroll_on_send: &mut bool,
    ) {
        Grid::new("custom_chat_extra_grid")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label("显示 \"话题\" 按钮");
                    ui.weak("单独显示");
                    ui.weak("回复同一条信息");
                    ui.weak("位于同一条信息回复链");
                    ui.weak("的信息");
                });
                let _ = ui.checkbox(&mut self.enable_topic_button, "");
                ui.end_row();

                ui.vertical(|ui| {
                    ui.label("纯文字模式");
                    ui.weak("去掉消息气泡与引用框");
                });
                let _ = ui.checkbox(&mut self.hide_group_member_avatar, "");
                ui.end_row();

                ui.label("选中会话后清空聊天列表搜索框");
                let _ = ui.checkbox(clear_on_select, "");
                ui.end_row();

                ui.vertical(|ui| {
                    ui.label("切换会话时自动拉取历史消息");
                    ui.weak("开启后会从协议端拉取最新漫游记录；关闭时只读取 bridge 缓存");
                });
                let _ = ui.checkbox(auto_fetch_history_on_select, "");
                ui.end_row();

                ui.label("发送消息后自动滚动到底部");
                let _ = ui.checkbox(scroll_on_send, "");
                ui.end_row();

                ui.label("选中会话时自动发送已读");
                let _ = ui.checkbox(&mut self.auto_read_on_select, "");
                ui.end_row();
            });
    }
}
