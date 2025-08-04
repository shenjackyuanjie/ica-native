
use crate::app::RoomId;

#[derive(Debug, Clone)]
pub struct ChatGroup {
    pub name: String,
    pub rooms: Vec<RoomId>,
}
