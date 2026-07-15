//! Administrative account/group/file/message/room/socket tools.
//!
//! Each tool keeps its state, validation and egui view together because they
//! form one cohesive, low-level bridge API surface.

mod account;
mod file;
mod group;
mod message;
mod room;
mod socket;

pub(super) use account::AccountToolsState;
pub(super) use file::FileToolsState;
pub(super) use group::GroupToolsState;
pub(super) use message::MessageToolsState;
pub(super) use room::RoomToolsState;
