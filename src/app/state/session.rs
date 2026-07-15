use std::ops::{Deref, DerefMut};

use tokio::sync::{mpsc::UnboundedSender, oneshot};

use crate::config::ChatGroups;
use crate::ica::types::{
    online_data::OnlineData,
    room::{JoinRequestRoom, Room},
};
use crate::ica::{BridgeHandle, IcaCommand};

use super::{AuthState, BridgeState, SelectedChatGroup, SocketState, VisibleRoomIndicesCache};

#[derive(Debug, Clone)]
pub struct ConnectionState {
    pub bridge_key: String,
    pub socket_state: SocketState,
    pub auth_state: AuthState,
    pub online_data: OnlineData,
    pub is_shut_up: bool,
    pub last_error: Option<String>,
    pub last_notice: Option<String>,
    pub last_socket_api_response: Option<String>,
    pub setup_requested: Option<String>,
    pub fatal_error: Option<String>,
    pub last_event: Option<String>,
}

impl ConnectionState {
    pub fn new(bridge_key: String) -> Self {
        Self {
            bridge_key,
            socket_state: SocketState::Connecting,
            auth_state: AuthState::Unknown,
            online_data: OnlineData::default(),
            is_shut_up: false,
            last_error: None,
            last_notice: None,
            last_socket_api_response: None,
            setup_requested: None,
            fatal_error: None,
            last_event: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoomDirectory {
    pub connection: ConnectionState,
    pub chat_groups: ChatGroups,
    pub selected_chat_group: SelectedChatGroup,
    pub rooms: Vec<Room>,
    pub rooms_revision: u64,
    pub visible_room_indices_cache: Option<VisibleRoomIndicesCache>,
    pub join_requests: Vec<JoinRequestRoom>,
}

impl RoomDirectory {
    pub fn new(bridge_key: String, chat_groups: ChatGroups) -> Self {
        Self {
            connection: ConnectionState::new(bridge_key),
            chat_groups,
            selected_chat_group: SelectedChatGroup::All,
            rooms: Vec::new(),
            rooms_revision: 1,
            visible_room_indices_cache: None,
            join_requests: Vec::new(),
        }
    }
}

impl Deref for RoomDirectory {
    type Target = ConnectionState;

    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

impl DerefMut for RoomDirectory {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.connection
    }
}

/// A bridge command handle and all state that belongs to that same connection.
/// Keeping these together prevents index drift between parallel vectors.
pub struct BridgeSession {
    handle: BridgeHandle,
    state: BridgeState,
    stop_sender: Option<oneshot::Sender<()>>,
}

impl BridgeSession {
    pub fn new(handle: BridgeHandle, state: BridgeState, stop_sender: oneshot::Sender<()>) -> Self {
        Self {
            handle,
            state,
            stop_sender: Some(stop_sender),
        }
    }

    pub fn handle(&self) -> &BridgeHandle {
        &self.handle
    }

    pub fn state(&self) -> &BridgeState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut BridgeState {
        &mut self.state
    }

    pub fn send(&self, command: IcaCommand) -> Result<(), String> {
        self.handle.send(command)
    }

    pub(crate) fn command_sender(&self) -> UnboundedSender<IcaCommand> {
        self.handle.command_sender()
    }

    pub fn stop(&mut self) {
        if let Some(sender) = self.stop_sender.take() {
            let _ = sender.send(());
        }
    }
}

impl Deref for BridgeSession {
    type Target = BridgeState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for BridgeSession {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}
