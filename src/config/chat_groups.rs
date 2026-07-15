use serde::{Deserialize, Serialize};

use crate::ica::types::RoomId;
use crate::ica::types::room::Room;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatGroups {
    pub groups: Vec<ChatGroup>,
}

impl ChatGroups {
    pub fn new() -> Self {
        Self { groups: Vec::new() }
    }

    pub fn group_names(&self) -> Vec<String> {
        self.groups.iter().map(|g| g.name()).collect()
    }

    pub fn add_group(&mut self, group: ChatGroup) {
        self.groups.push(group);
    }

    pub fn remove_group(&mut self, index: usize) {
        if index < self.groups.len() {
            self.groups.remove(index);
        }
    }

    pub fn rename_group(&mut self, index: usize, new_name: String) {
        if let Some(group) = self.groups.get_mut(index) {
            group.name = new_name;
        }
    }

    pub fn toggle_room_in_group(&mut self, group_index: usize, room_id: RoomId) -> bool {
        if let Some(group) = self.groups.get_mut(group_index) {
            if let Some(pos) = group.rooms.iter().position(|&id| id == room_id) {
                group.rooms.remove(pos);
                false
            } else {
                group.rooms.push(room_id);
                true
            }
        } else {
            false
        }
    }

    pub fn is_room_in_group(&self, group_index: usize, room_id: RoomId) -> bool {
        self.groups
            .get(group_index)
            .map(|g| g.rooms.contains(&room_id))
            .unwrap_or(false)
    }

    pub fn has_unread_in_group(&self, group_index: usize, rooms: &[Room]) -> bool {
        let Some(group) = self.groups.get(group_index) else {
            return false;
        };
        rooms.iter().any(|room| {
            room.unread_count > 0
                && (group.rooms.contains(&room.room_id)
                    || (group.include_all_personal && room.room_id > 0))
        })
    }

    pub fn move_group(&mut self, from: usize, to: usize) {
        if from < self.groups.len() && to < self.groups.len() {
            let group = self.groups.remove(from);
            self.groups.insert(to, group);
        }
    }
}

impl Default for ChatGroups {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatGroup {
    pub name: String,
    #[serde(default)]
    pub rooms: Vec<RoomId>,
    #[serde(
        default,
        rename(deserialize = "includeAllPersonal"),
        alias = "include_all_personal"
    )]
    pub include_all_personal: bool,
}

impl ChatGroup {
    pub fn new(name: impl Into<String>, rooms: Vec<RoomId>) -> Self {
        Self {
            name: name.into(),
            rooms,
            include_all_personal: false,
        }
    }

    pub fn new_empty(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            rooms: Vec::new(),
            include_all_personal: false,
        }
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::ChatGroup;

    #[test]
    fn deserializes_protocol_include_all_personal() {
        let group: ChatGroup = serde_json::from_value(json!({
            "name": "全部私聊",
            "index": 0,
            "rooms": [],
            "includeAllPersonal": true,
        }))
        .expect("protocol chat group should deserialize");

        assert!(group.include_all_personal);
    }

    #[test]
    fn deserializes_legacy_snake_case_include_all_personal() {
        let group: ChatGroup = serde_json::from_value(json!({
            "name": "全部私聊",
            "rooms": [],
            "include_all_personal": true,
        }))
        .expect("legacy local chat group should deserialize");

        assert!(group.include_all_personal);
    }
}
