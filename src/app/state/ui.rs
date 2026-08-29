use crate::config::{ChatAppearanceSettings, ConfigStore, ReEditDraftConflictMode};

use super::super::{
    auto_sign::AutoSignState,
    chat_groups::ChatGroupEditor,
    online_mode::OnlineMode,
    open_page::AppOpenPage,
    relation_network::RelationNetworkState,
    settings::ConfigEditor,
    stickers::{StickerPickerTab, StickerStore},
    tools::{
        AccountToolsState, FileToolsState, GroupToolsState, MessageToolsState, RoomToolsState,
    },
};
use super::{BridgeSession, ChatListScrollTarget, ImageViewerState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroupMemberFilter {
    #[default]
    All,
    Muted,
}

/// 窄窗口下主区域展示的页面。宽屏仍同时展示会话列表和聊天内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompactChatPanel {
    #[default]
    Conversations,
    Chat,
}

#[derive(Debug, Clone)]
pub struct GroupBanConfirmation {
    pub room_id: i64,
    pub target_id: i64,
    pub target_name: String,
    pub duration: u64,
}

#[derive(Debug)]
pub struct GroupMemberPanelState {
    pub open: bool,
    pub search_query: String,
    pub filter: GroupMemberFilter,
    pub custom_duration: String,
    pub error: Option<String>,
    pub confirmation: Option<GroupBanConfirmation>,
}

impl Default for GroupMemberPanelState {
    fn default() -> Self {
        Self {
            open: false,
            search_query: String::new(),
            filter: GroupMemberFilter::All,
            custom_duration: "600".to_string(),
            error: None,
            confirmation: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct GroupFilePanelState {
    pub open: bool,
    pub directory_fid: String,
    pub file_fid: String,
    pub folder_name: String,
    pub list_start: String,
}

pub struct AppState {
    pub custom_chat: ChatAppearanceSettings,
    pub online_mode: OnlineMode,
    pub open_page: AppOpenPage,
    pub mute_all: bool,
    pub mute_any: bool,
    pub notify_level: u8,
    pub chat_group_editor: ChatGroupEditor,
    pub config_editor: ConfigEditor,
    pub chat_list_scroll_target: ChatListScrollTarget,
    pub compact_chat_panel: CompactChatPanel,
    pub clear_search_on_room_select: bool,
    pub auto_fetch_history_on_room_select: bool,
    pub scroll_to_bottom_after_send: bool,
    pub reedit_draft_conflict_mode: ReEditDraftConflictMode,
    pub active_bridge_idx: Option<usize>,
    /// bridge 句柄、停止信号和 UI 状态统一存放在一个会话对象中。
    pub bridge_states: Vec<BridgeSession>,
    pub clipboard_paste_failed: bool,
    pub ime_composing: bool,
    pub ime_event_this_frame: bool,
    pub show_face_picker: bool,
    pub show_mention_picker: bool,
    pub mention_search_query: String,
    pub mention_search_focus_requested: bool,
    pub mention_replace_trigger: bool,
    pub mention_selected_index: usize,
    pub image_viewer: Option<std::sync::Arc<std::sync::Mutex<ImageViewerState>>>,
    pub socket_api_event: String,
    pub socket_api_args: String,
    pub socket_api_expect_ack: bool,
    pub socket_api_preset_idx: usize,
    pub group_tools: GroupToolsState,
    pub account_tools: AccountToolsState,
    pub file_tools: FileToolsState,
    pub message_tools: MessageToolsState,
    pub room_tools: RoomToolsState,
    pub auto_sign: AutoSignState,
    pub relation_network: std::sync::Arc<std::sync::Mutex<RelationNetworkState>>,
    pub sticker_store: StickerStore,
    pub sticker_picker_tab: StickerPickerTab,
    pub sticker_category: String,
    pub sticker_new_category: String,
    pub media_notice: Option<String>,
    pub media_error: Option<String>,
    pub group_member_panel: GroupMemberPanelState,
    pub group_file_panel: GroupFilePanelState,
}

impl AppState {
    pub fn new(
        config: &crate::config::IcaCfg,
        store: &ConfigStore,
        bridge_states: Vec<BridgeSession>,
        sticker_store: StickerStore,
    ) -> Self {
        Self {
            custom_chat: config.custom_chat.clone(),
            online_mode: OnlineMode::default(),
            open_page: AppOpenPage::default(),
            mute_any: false,
            mute_all: false,
            notify_level: 3,
            chat_group_editor: ChatGroupEditor::default(),
            config_editor: ConfigEditor::new(store),
            chat_list_scroll_target: ChatListScrollTarget::Top,
            compact_chat_panel: CompactChatPanel::default(),
            clear_search_on_room_select: config.ui_setting.clear_search_on_room_select,
            auto_fetch_history_on_room_select: config.ui_setting.auto_fetch_history_on_room_select,
            scroll_to_bottom_after_send: config.ui_setting.scroll_to_bottom_after_send,
            reedit_draft_conflict_mode: config.ui_setting.reedit_draft_conflict_mode,
            active_bridge_idx: (!bridge_states.is_empty()).then_some(0),
            bridge_states,
            clipboard_paste_failed: false,
            ime_composing: false,
            ime_event_this_frame: false,
            show_face_picker: false,
            show_mention_picker: false,
            mention_search_query: String::new(),
            mention_search_focus_requested: false,
            mention_replace_trigger: false,
            mention_selected_index: 0,
            image_viewer: None,
            socket_api_event: String::new(),
            socket_api_args: "[]".to_string(),
            socket_api_expect_ack: true,
            socket_api_preset_idx: 0,
            group_tools: GroupToolsState::default(),
            account_tools: AccountToolsState::default(),
            file_tools: FileToolsState::default(),
            message_tools: MessageToolsState::default(),
            room_tools: RoomToolsState::default(),
            auto_sign: AutoSignState::default(),
            relation_network: std::sync::Arc::new(std::sync::Mutex::new(
                RelationNetworkState::default()
                    .with_render_setting(config.ui_setting.relation_network.clone()),
            )),
            sticker_store,
            sticker_picker_tab: StickerPickerTab::default(),
            sticker_category: "默认".to_string(),
            sticker_new_category: String::new(),
            media_notice: None,
            media_error: None,
            group_member_panel: GroupMemberPanelState::default(),
            group_file_panel: GroupFilePanelState {
                list_start: "0".to_string(),
                ..Default::default()
            },
        }
    }
}
