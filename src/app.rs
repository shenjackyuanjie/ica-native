use std::sync::Arc;

use eframe::CreationContext;
use tokio::runtime::Runtime;

use crate::{assets, ica::IcaClient};

pub mod chat_groups;
pub mod config_editer;
pub mod custom_chat;
pub mod online_mode;
pub mod open_page;
pub mod renders;

use chat_groups::ChatGroups;
use config_editer::ConfigEditer;
use custom_chat::CustomChat;
use online_mode::OnlineMode;
use open_page::AppOpenPage;

use crate::ica::types::{RoomId, room::Room};

pub struct IcaApp {
    /// 是否连接上了
    pub connected: bool,
    /// 聊天界面定制选项
    pub custom_chat: CustomChat,
    /// 在线模式
    pub online_mode: OnlineMode,
    /// 打开了什么页面
    pub open_page: AppOpenPage,
    /// 是否禁用 @ 全体 通知
    pub mute_all: bool,
    /// 是否禁用任何通知
    pub mute_any: bool,
    /// 通知等级
    pub notify_level: u8,
    /// 所有聊天
    pub chat_rooms: Vec<Room>,
    /// 是否选中某个聊天组
    pub chat_group_selected: bool,
    /// 选中了哪个聊天组
    pub chat_group_idx: usize,
    /// 聊天组
    pub chat_groups: ChatGroups,
    /// 配置文件修改
    pub config_editer: ConfigEditer,
    /// 选中的聊天室 ID
    pub selected_room_id: Option<RoomId>,
    /// tokio rt
    /// 用来开 socketio
    pub runtime: Runtime,
    /// Socketio 列表
    /// 一些 Socketio 连接
    pub ica_clients: Vec<IcaClient>,
}

impl IcaApp {
    fn setup_fonts(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();

        let font_sy_data = egui::FontData::from_static(assets::fonts::FONT_思源黑体);
        let font_unifont_data = egui::FontData::from_static(assets::fonts::FONT_UNIFONT);

        let sy_font_name = "notosans".to_string();
        let unifont_name = "unifont".to_string();

        fonts
            .font_data
            .insert(sy_font_name.clone(), Arc::new(font_sy_data));

        fonts
            .font_data
            .insert(unifont_name.clone(), Arc::new(font_unifont_data));

        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, unifont_name.clone());

        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, sy_font_name.clone());

        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push(sy_font_name.clone());

        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push(unifont_name.clone());

        ctx.set_fonts(fonts);
    }

    fn setup_async_rt() -> Runtime {
        let config = crate::cfg::get_cfg_snapshot();
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(config.tokio_rt_work_thread as usize)
            .enable_all()
            .build()
            .expect("faild to build tokio rt")
    }

    pub fn new(cc: &CreationContext<'_>) -> Self {
        Self::setup_fonts(&cc.egui_ctx);
        Self {
            connected: false,
            custom_chat: CustomChat::default(),
            online_mode: OnlineMode::default(),
            open_page: AppOpenPage::default(),
            mute_any: false,
            mute_all: false,
            notify_level: 3,
            chat_rooms: Vec::new(),
            chat_group_selected: false,
            chat_group_idx: 0,
            chat_groups: ChatGroups::new(),
            config_editer: ConfigEditer::default(),
            selected_room_id: None,
            runtime: Self::setup_async_rt(),
            ica_clients: Vec::new(),
        }
    }
}

impl eframe::App for IcaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 检测 ESC 键取消选择
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.selected_room_id = None;
        }

        // 渲染相关的方法已移到 `renders.rs` 模块
        self.render_top_panel(ctx);
        self.render_left_groups_panel(ctx);
        self.render_chat_list_panel(ctx);
        self.render_central_panel(ctx);
        self.render_windows(ctx);
    }
}