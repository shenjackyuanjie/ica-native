//! 应用级 UI 状态：全局开关、各功能窗口状态与每个聊天窗口的输入态。
//!
//! 这里的东西不属于任何单个 bridge：`AppState` 由 `IcaApp` 持有，跨 bridge
//! 共享（外观、在线状态、各类工具窗口）；`ChatWindowUiState` 则是每个聊天
//! 窗口私有的输入法与输入弹层状态，通过 `Deref` 暴露在 `AppState` 上。

use super::super::{
    auto_sign::AutoSignState,
    chat_groups::ChatGroupEditor,
    online_mode::OnlineMode,
    open_page::AppOpenPage,
    relation_network::RelationNetworkState,
    settings::{ConfigEditor, NoticerEditor},
    stickers::{StickerPickerTab, StickerStore},
    tools::{
        AccountToolsState, FileToolsState, GroupToolsState, MessageToolsState, RoomToolsState,
    },
};
use super::{BridgeSession, ChatListScrollTarget, ImageViewerState};
use crate::config::{ChatAppearanceSettings, ConfigStore, ReEditDraftConflictMode};
use std::sync::{Arc, Mutex, atomic::AtomicBool};

/// 群成员面板的列表过滤方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroupMemberFilter {
    /// 全部成员。
    #[default]
    All,
    /// 仅被禁言的成员。
    Muted,
}

/// 窄窗口下主区域展示的页面。宽屏仍同时展示会话列表和聊天内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompactChatPanel {
    /// 展示会话列表（选择会话）。
    #[default]
    Conversations,
    /// 展示聊天内容。
    Chat,
}

/// 群成员禁言的二次确认参数（目标、时长）。
#[derive(Debug, Clone)]
pub struct GroupBanConfirmation {
    /// 目标群（负的群号）。
    pub room_id: i64,
    /// 被禁言成员 QQ 号。
    pub target_id: i64,
    /// 被禁言成员名称，用于确认文案。
    pub target_name: String,
    /// 禁言时长（秒）。
    pub duration: u64,
}

/// 群成员面板状态（每个聊天窗口一份）。
#[derive(Debug)]
pub struct GroupMemberPanelState {
    /// 面板是否打开。
    pub open: bool,
    /// 成员搜索词。
    pub search_query: String,
    /// 列表过滤方式。
    pub filter: GroupMemberFilter,
    /// 自定义禁言时长输入框内容（秒）。
    pub custom_duration: String,
    /// 面板内的错误提示。
    pub error: Option<String>,
    /// 待确认的禁言操作。
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

/// 群文件面板状态（每个聊天窗口一份）。
#[derive(Debug)]
pub struct GroupFilePanelState {
    /// 面板是否打开。
    pub open: bool,
    /// 当前浏览的目录 FID。
    pub directory_fid: String,
    /// 下载目标文件 FID。
    pub file_fid: String,
    /// 新建文件夹名称输入框。
    pub folder_name: String,
    /// 列表分页起始偏移。
    pub list_start: String,
}

/// 两个定制聊天界面独立窗口共享的可编辑配置草稿。
///
/// 窗口只修改这份草稿并置 `dirty`，主循环下一帧统一比对并写回应用状态与
/// 配置文件，避免两个窗口和应用主状态同时修改同一份配置。
pub struct ChatAppearanceViewportState {
    /// 定制聊天界面的外观设置草稿。
    pub custom_chat: ChatAppearanceSettings,
    /// “选中会话后清空聊天列表搜索框”草稿。
    pub clear_search_on_room_select: bool,
    /// “切换会话时自动拉取历史消息”草稿。
    pub auto_fetch_history_on_room_select: bool,
    /// “发送消息后自动滚动到底部”草稿。
    pub scroll_to_bottom_after_send: bool,
    /// 重新编辑草稿冲突处理模式草稿。
    pub reedit_draft_conflict_mode: ReEditDraftConflictMode,
    /// 草稿是否被修改过；主循环据此决定是否应用。
    pub dirty: bool,
}

impl ChatAppearanceViewportState {
    /// 从当前配置初始化草稿；尚未修改。
    fn new(config: &crate::config::IcaCfg) -> Self {
        Self {
            custom_chat: config.custom_chat.clone(),
            clear_search_on_room_select: config.ui_setting.clear_search_on_room_select,
            auto_fetch_history_on_room_select: config.ui_setting.auto_fetch_history_on_room_select,
            scroll_to_bottom_after_send: config.ui_setting.scroll_to_bottom_after_send,
            reedit_draft_conflict_mode: config.ui_setting.reedit_draft_conflict_mode,
            dirty: false,
        }
    }
}

/// 应用级 UI 状态：跨 bridge 共享的开关、窗口与各功能模块状态。
///
/// 由 `IcaApp` 每帧驱动；与具体 bridge 无关的状态放这里，按 bridge 的
/// 状态放各自的 `BridgeState`。
pub struct AppState {
    /// 本地录音与语音播放控制器。
    pub audio: crate::app::audio::AudioController,
    /// 定制聊天界面外观设置（与配置同步的运行时副本）。
    pub custom_chat: ChatAppearanceSettings,
    /// 在线状态模式。
    pub online_mode: OnlineMode,
    /// 顶层“打开了哪些页面/窗口”的开关集合。
    pub open_page: AppOpenPage,
    /// 禁用 @全体 通知。
    pub mute_all: bool,
    /// 禁用任何通知。
    pub mute_any: bool,
    /// 通知启用级别（1-5）。
    pub notify_level: u8,
    /// 聊天分组编辑器状态。
    pub chat_group_editor: ChatGroupEditor,
    /// 配置文件（TOML）编辑器状态。
    pub config_editor: ConfigEditor,
    /// Noticer 设置编辑器状态。
    pub noticer_editor: NoticerEditor,
    /// Agent 上下文设置界面状态。
    pub agent_context_ui: crate::app::agent_context::AgentContextUi,
    /// 会话列表的滚动目标（一次性请求）。
    pub chat_list_scroll_target: ChatListScrollTarget,
    /// 窄窗口下主区域展示的页面。
    pub compact_chat_panel: CompactChatPanel,
    /// 选中会话后清空聊天列表搜索框。
    pub clear_search_on_room_select: bool,
    /// 切换会话时自动拉取历史消息。
    pub auto_fetch_history_on_room_select: bool,
    /// 发送消息后自动滚动到底部。
    pub scroll_to_bottom_after_send: bool,
    /// 重新编辑草稿冲突处理模式。
    pub reedit_draft_conflict_mode: ReEditDraftConflictMode,
    /// 当前活跃 bridge 在 `bridge_states` 中的下标。
    pub active_bridge_idx: Option<usize>,
    /// bridge 句柄、停止信号和 UI 状态统一存放在一个会话对象中。
    pub bridge_states: Vec<BridgeSession>,
    /// 每个聊天窗口各自的输入法、输入弹层和群面板状态。
    pub chat_ui: ChatWindowUiState,
    /// 图片查看器状态；打开时经 `Arc<Mutex<..>>` 与查看 viewport 共享。
    pub image_viewer: Option<std::sync::Arc<std::sync::Mutex<ImageViewerState>>>,
    /// 联系人独立窗口关闭后通知主视口更新打开状态。
    pub contacts_viewport_closed: Arc<AtomicBool>,
    /// 两个定制聊天界面窗口关闭后通知主视口更新打开状态。
    pub custom_chat_ica_viewport_closed: Arc<AtomicBool>,
    /// 定制聊天界面(extra)窗口关闭后通知主视口更新打开状态。
    pub custom_chat_extra_viewport_closed: Arc<AtomicBool>,
    /// 两个定制聊天界面窗口共享的配置草稿。
    pub chat_appearance_viewport: Arc<Mutex<ChatAppearanceViewportState>>,
    /// 高级 Socket API：待发送的事件名。
    pub socket_api_event: String,
    /// 高级 Socket API：待发送的 JSON 参数数组文本。
    pub socket_api_args: String,
    /// 高级 Socket API：是否等待 ack。
    pub socket_api_expect_ack: bool,
    /// 高级 Socket API：当前选中的预设下标。
    pub socket_api_preset_idx: usize,
    /// 群/成员管理工具窗口状态。
    pub group_tools: GroupToolsState,
    /// 账号/登录设备工具窗口状态。
    pub account_tools: AccountToolsState,
    /// 文件/资源工具窗口状态。
    pub file_tools: FileToolsState,
    /// 消息检索/历史工具窗口状态。
    pub message_tools: MessageToolsState,
    /// 会话设置工具窗口状态。
    pub room_tools: RoomToolsState,
    /// 全群自动签到任务状态。
    pub auto_sign: AutoSignState,
    /// QQ 关系网窗口状态（与关系网 viewport 共享）。
    pub relation_network: std::sync::Arc<std::sync::Mutex<RelationNetworkState>>,
    /// 表情商店数据。
    pub sticker_store: StickerStore,
    /// 表情选择器当前页签。
    pub sticker_picker_tab: StickerPickerTab,
    /// 表情选择器当前分类。
    pub sticker_category: String,
    /// 表情选择器“新分类”输入框内容。
    pub sticker_new_category: String,
    /// 媒体相关提示（保存成功等）。
    pub media_notice: Option<String>,
    /// 媒体相关错误提示。
    pub media_error: Option<String>,
}

impl AppState {
    /// 从配置、配置存储、已装配的 bridge 会话与表情商店创建初始状态。
    ///
    /// 会从 `ui_setting` 带回用户可见的行为开关；有 bridge 时默认选中第一个。
    pub fn new(
        config: &crate::config::IcaCfg,
        store: &ConfigStore,
        bridge_states: Vec<BridgeSession>,
        sticker_store: StickerStore,
    ) -> Self {
        Self {
            audio: crate::app::audio::AudioController::default(),
            custom_chat: config.custom_chat.clone(),
            online_mode: OnlineMode::default(),
            open_page: AppOpenPage::default(),
            mute_any: false,
            mute_all: false,
            notify_level: 3,
            chat_group_editor: ChatGroupEditor::default(),
            config_editor: ConfigEditor::new(store),
            noticer_editor: NoticerEditor::new(store),
            agent_context_ui: crate::app::agent_context::AgentContextUi::new(&config.agent_context),
            chat_list_scroll_target: ChatListScrollTarget::Top,
            compact_chat_panel: CompactChatPanel::default(),
            clear_search_on_room_select: config.ui_setting.clear_search_on_room_select,
            auto_fetch_history_on_room_select: config.ui_setting.auto_fetch_history_on_room_select,
            scroll_to_bottom_after_send: config.ui_setting.scroll_to_bottom_after_send,
            reedit_draft_conflict_mode: config.ui_setting.reedit_draft_conflict_mode,
            active_bridge_idx: (!bridge_states.is_empty()).then_some(0),
            bridge_states,
            chat_ui: ChatWindowUiState::default(),
            image_viewer: None,
            contacts_viewport_closed: Arc::new(AtomicBool::new(false)),
            custom_chat_ica_viewport_closed: Arc::new(AtomicBool::new(false)),
            custom_chat_extra_viewport_closed: Arc::new(AtomicBool::new(false)),
            chat_appearance_viewport: Arc::new(Mutex::new(ChatAppearanceViewportState::new(
                config,
            ))),
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
        }
    }
}

/// 每个聊天窗口各自维护输入法、输入弹层和群面板状态。
///
/// 主窗口与独立聊天窗口各持一份，绘制时通过交换该状态复用同一套聊天界面。
#[derive(Debug, Default)]
pub struct ChatWindowUiState {
    /// 剪贴板粘贴失败提示（等待展示）。
    pub clipboard_paste_failed: bool,
    /// 输入法是否正在组词。
    pub ime_composing: bool,
    /// 本帧是否处理过输入法事件。
    pub ime_event_this_frame: bool,
    /// 表情选择浮层是否打开。
    pub show_face_picker: bool,
    /// @提及选择浮层是否打开。
    pub show_mention_picker: bool,
    /// @提及浮层的搜索词。
    pub mention_search_query: String,
    /// 是否请求把焦点移到 @提及浮层。
    pub mention_search_focus_requested: bool,
    /// 选中提及后需要替换触发字符（`@`）的信号。
    pub mention_replace_trigger: bool,
    /// @提及浮层当前键盘选中的下标。
    pub mention_selected_index: usize,
    /// 群成员面板状态。
    pub group_member_panel: GroupMemberPanelState,
    /// 群文件面板状态。
    pub group_file_panel: GroupFilePanelState,
}

/// 让 `AppState` 可以直接访问当前聊天窗口的输入态字段（`ime_composing` 等）。
impl std::ops::Deref for AppState {
    type Target = ChatWindowUiState;
    fn deref(&self) -> &Self::Target {
        &self.chat_ui
    }
}

impl std::ops::DerefMut for AppState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.chat_ui
    }
}

impl Default for GroupFilePanelState {
    fn default() -> Self {
        Self {
            open: false,
            directory_fid: String::new(),
            file_fid: String::new(),
            folder_name: String::new(),
            list_start: "0".into(),
        }
    }
}

impl ChatWindowUiState {
    /// 关闭 @提及浮层并重置其全部中间状态。
    pub fn close_mention_picker(&mut self) {
        self.show_mention_picker = false;
        self.mention_search_query.clear();
        self.mention_search_focus_requested = false;
        self.mention_replace_trigger = false;
        self.mention_selected_index = 0;
    }

    /// Esc 关闭最上层的输入弹层（@提及优先于表情）。
    ///
    /// 返回是否消费了这次取消；无弹层打开时返回 `false`。
    pub fn cancel_composer_popup(&mut self) -> bool {
        if self.show_mention_picker {
            self.close_mention_picker();
            true
        } else if self.show_face_picker {
            self.show_face_picker = false;
            true
        } else {
            false
        }
    }
}
