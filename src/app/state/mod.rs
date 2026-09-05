//! GUI 侧的应用状态。
//!
//! 状态按关注点拆到多个子模块，本文件只声明模块并统一再导出，
//! 因此 `crate::app::state::X` 的引用路径与拆分前保持一致。

mod announcement;
mod bridge;
mod connection;
mod conversation;
mod forward;
mod image_viewer;
mod interaction;
mod layout;
mod member;
mod search;
mod session;
mod ui;

pub use announcement::GroupAnnouncementViewerState;
pub use bridge::{BridgeState, DatabaseUpgradeProgress, VisibleRoomIndicesCache};
pub use connection::{AuthState, SocketState};
pub use conversation::ConversationState;
pub use forward::{ForwardViewerAction, ForwardViewerState};
pub use image_viewer::ImageViewerState;
pub use interaction::{ChatListScrollTarget, MessageAction, PendingFile, PendingImage};
pub use layout::{MessageLayoutCacheKey, MessageRowLayout};
pub use member::GroupMember;
pub use search::{MemberHistoryState, MessageSearchState};
pub use session::{
    BridgeSession, ConnectionState, RoomDirectory, StatusMessage, StatusMessageKind,
};
pub use ui::{
    AppState, ChatWindowUiState, CompactChatPanel, GroupBanConfirmation, GroupFilePanelState,
    GroupMemberFilter,
};
