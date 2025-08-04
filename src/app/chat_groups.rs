use crate::app::RoomId;

#[derive(Debug, Clone)]
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
}

impl Default for ChatGroups {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct ChatGroup {
    pub name: String,
    pub rooms: Vec<RoomId>,
}

impl ChatGroup {
    pub fn new(name: impl Into<String>, rooms: Vec<RoomId>) -> Self {
        Self {
            name: name.into(),
            rooms,
        }
    }

    pub fn new_empty(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            rooms: Vec::new(),
        }
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }
}
