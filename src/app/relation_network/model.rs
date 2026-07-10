use std::collections::{HashMap, HashSet};

use crate::ica::types::{RoomId, room::Room};

use super::super::state::GroupMember;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum RelationNodeKind {
    SelfUser,
    Friend,
    Acquaintance,
    Stranger,
    Group,
}

impl RelationNodeKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::SelfUser => "自己",
            Self::Friend => "好友",
            Self::Acquaintance => "共同群好友",
            Self::Stranger => "仅同群",
            Self::Group => "群",
        }
    }

    pub(super) fn color(self) -> egui::Color32 {
        match self {
            Self::SelfUser => egui::Color32::from_rgb(255, 215, 0),
            Self::Friend => egui::Color32::from_rgb(74, 144, 217),
            Self::Acquaintance => egui::Color32::from_rgb(135, 206, 235),
            Self::Stranger => egui::Color32::from_rgb(176, 196, 222),
            Self::Group => egui::Color32::from_rgb(231, 76, 60),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct RelationNode {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) kind: RelationNodeKind,
    pub(super) value: usize,
    pub(super) radius: f32,
    pub(super) size_level: u8,
    pub(super) qq: Option<i64>,
    pub(super) group_id: Option<i64>,
    pub(super) member_count: Option<usize>,
    pub(super) common_group_count: usize,
    pub(super) role: String,
}

impl RelationNode {
    fn new_user(user_id: i64, name: String, kind: RelationNodeKind) -> Self {
        Self {
            id: format!("u:{user_id}"),
            name,
            kind,
            value: 0,
            radius: 8.0,
            size_level: 0,
            qq: Some(user_id),
            group_id: None,
            member_count: None,
            common_group_count: 0,
            role: String::new(),
        }
    }

    fn new_group(room: &Room, member_count: Option<usize>) -> Self {
        let group_id = -room.room_id;
        Self {
            id: format!("g:{group_id}"),
            name: if room.room_name.trim().is_empty() {
                group_id.to_string()
            } else {
                room.room_name.clone()
            },
            kind: RelationNodeKind::Group,
            value: 0,
            radius: 8.0,
            size_level: 0,
            qq: None,
            group_id: Some(group_id),
            member_count,
            common_group_count: 0,
            role: String::new(),
        }
    }

    fn update_size(&mut self) {
        let min_size = 8.0_f32;
        let max_size = 44.0_f32;
        let max_value = 50_000.0_f32;
        let value = self
            .member_count
            .filter(|_| self.kind == RelationNodeKind::Group)
            .unwrap_or(self.value)
            .max(self.value) as f32;

        self.radius = if value <= 0.0 {
            min_size
        } else {
            let normalized = ((value + 1.0).ln() / (max_value + 1.0).ln()).powf(0.7);
            (min_size + (max_size - min_size) * normalized).clamp(min_size, max_size)
        };

        self.size_level = if value < 10.0 {
            0
        } else if value < 100.0 {
            1
        } else if value < 1_000.0 {
            2
        } else if value < 10_000.0 {
            3
        } else {
            4
        };
    }

    pub(super) fn matches_query(&self, query: &str) -> bool {
        query.is_empty()
            || self.name.to_lowercase().contains(query)
            || self.qq.is_some_and(|qq| qq.to_string().contains(query))
            || self
                .group_id
                .is_some_and(|group_id| group_id.to_string().contains(query))
    }
}

#[derive(Debug, Clone)]
pub(super) struct RelationLink {
    pub(super) source: String,
    pub(super) target: String,
}

#[derive(Debug, Clone, Default)]
pub(super) struct RelationGraph {
    pub(super) nodes: Vec<RelationNode>,
    pub(super) links: Vec<RelationLink>,
    pub(super) node_index: HashMap<String, usize>,
    pub(super) group_node_indices: Vec<usize>,
    pub(super) node_counts: RelationNodeCounts,
    pub(super) loaded_group_count: usize,
    pub(super) total_group_count: usize,
}

impl RelationGraph {
    pub(super) fn node_counts(&self) -> RelationNodeCounts {
        self.node_counts
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct RelationNodeCounts {
    pub(super) self_user: usize,
    pub(super) friend: usize,
    pub(super) acquaintance: usize,
    pub(super) stranger: usize,
    pub(super) group: usize,
}

#[derive(Debug, Clone)]
pub(super) struct RelationGraphOptions {
    pub(super) show_self_user: bool,
    pub(super) show_friends: bool,
    pub(super) show_acquaintances: bool,
    pub(super) show_strangers: bool,
    pub(super) show_groups: bool,
}

impl Default for RelationGraphOptions {
    fn default() -> Self {
        Self {
            show_self_user: true,
            show_friends: true,
            show_acquaintances: true,
            show_strangers: false,
            show_groups: true,
        }
    }
}

impl RelationGraphOptions {
    pub(super) fn allows(&self, kind: RelationNodeKind) -> bool {
        match kind {
            RelationNodeKind::SelfUser => self.show_self_user,
            RelationNodeKind::Friend => self.show_friends,
            RelationNodeKind::Acquaintance => self.show_acquaintances,
            RelationNodeKind::Stranger => self.show_strangers,
            RelationNodeKind::Group => self.show_groups,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct RelationGraphBuilder {
    nodes: HashMap<String, RelationNode>,
    links: HashMap<(String, String), RelationLink>,
    user_group_map: HashMap<i64, HashSet<i64>>,
    friend_ids: HashSet<i64>,
    login_user_id: Option<i64>,
}

impl RelationGraphBuilder {
    pub(super) fn build(
        login_user_id: Option<i64>,
        rooms: &[Room],
        group_members_by_room: &HashMap<RoomId, Vec<GroupMember>>,
        include_unloaded_groups: bool,
    ) -> RelationGraph {
        let mut builder = Self {
            login_user_id,
            ..Self::default()
        };
        builder.add_self_user();
        builder.add_private_rooms(rooms);
        builder.add_groups(rooms, group_members_by_room, include_unloaded_groups);
        builder.add_group_members(group_members_by_room);
        builder.finalize(rooms, group_members_by_room)
    }

    fn add_self_user(&mut self) {
        let Some(user_id) = self.login_user_id else {
            return;
        };
        let node = RelationNode::new_user(user_id, "我".to_string(), RelationNodeKind::SelfUser);
        self.nodes.insert(node.id.clone(), node);
    }

    fn add_private_rooms(&mut self, rooms: &[Room]) {
        for room in rooms.iter().filter(|room| room.room_id > 0) {
            let user_id = room.room_id;
            self.friend_ids.insert(user_id);
            let node = RelationNode::new_user(
                user_id,
                if room.room_name.trim().is_empty() {
                    user_id.to_string()
                } else {
                    room.room_name.clone()
                },
                RelationNodeKind::Friend,
            );
            self.nodes.entry(node.id.clone()).or_insert(node);
            if let Some(login_user_id) = self.login_user_id {
                self.add_link(format!("u:{login_user_id}"), format!("u:{user_id}"));
            }
        }
    }

    fn add_groups(
        &mut self,
        rooms: &[Room],
        group_members_by_room: &HashMap<RoomId, Vec<GroupMember>>,
        include_unloaded_groups: bool,
    ) {
        for room in rooms.iter().filter(|room| room.room_id < 0) {
            if !include_unloaded_groups && !group_members_by_room.contains_key(&room.room_id) {
                continue;
            }
            let member_count = group_members_by_room.get(&room.room_id).map(Vec::len);
            let node = RelationNode::new_group(room, member_count);
            let group_id = node.group_id.unwrap_or_default();
            self.nodes.entry(node.id.clone()).or_insert(node);

            if let Some(login_user_id) = self.login_user_id {
                self.user_group_map
                    .entry(login_user_id)
                    .or_default()
                    .insert(group_id);
                self.add_link(format!("u:{login_user_id}"), format!("g:{group_id}"));
            }
        }
    }

    fn add_group_members(&mut self, group_members_by_room: &HashMap<RoomId, Vec<GroupMember>>) {
        for (&room_id, members) in group_members_by_room {
            if room_id >= 0 {
                continue;
            }
            let group_id = -room_id;
            let group_node_id = format!("g:{group_id}");
            if !self.nodes.contains_key(&group_node_id) {
                continue;
            }

            for member in members {
                let user_id = member.user_id;
                let user_node_id = format!("u:{user_id}");
                self.user_group_map
                    .entry(user_id)
                    .or_default()
                    .insert(group_id);

                if Some(user_id) != self.login_user_id {
                    let kind = if self.friend_ids.contains(&user_id) {
                        RelationNodeKind::Friend
                    } else if self
                        .user_group_map
                        .get(&user_id)
                        .is_some_and(|groups| groups.len() >= 2)
                    {
                        RelationNodeKind::Acquaintance
                    } else {
                        RelationNodeKind::Stranger
                    };

                    let name = member.display_name();
                    let name = if name.trim().is_empty() {
                        user_id.to_string()
                    } else {
                        name.to_string()
                    };
                    let node = self
                        .nodes
                        .entry(user_node_id.clone())
                        .or_insert_with(|| RelationNode::new_user(user_id, name, kind));

                    if node.kind != RelationNodeKind::Friend {
                        node.kind = kind;
                    }
                    if node.name == user_id.to_string() && !member.display_name().trim().is_empty()
                    {
                        node.name = member.display_name().to_string();
                    }
                    if !member.role.trim().is_empty() {
                        node.role = member.role.clone();
                    }
                }

                self.add_link(group_node_id.clone(), user_node_id);
            }
        }
    }

    fn add_link(&mut self, source: String, target: String) {
        if source == target {
            return;
        }
        let key = if source <= target {
            (source.clone(), target.clone())
        } else {
            (target.clone(), source.clone())
        };
        if self.links.contains_key(&key) {
            return;
        }
        if let Some(node) = self.nodes.get_mut(&source) {
            node.value += 1;
        }
        if let Some(node) = self.nodes.get_mut(&target) {
            node.value += 1;
        }
        self.links.insert(key, RelationLink { source, target });
    }

    fn finalize(
        mut self,
        rooms: &[Room],
        group_members_by_room: &HashMap<RoomId, Vec<GroupMember>>,
    ) -> RelationGraph {
        for node in self.nodes.values_mut() {
            if let Some(qq) = node.qq {
                node.common_group_count = self.user_group_map.get(&qq).map_or(0, HashSet::len);
            }
            node.update_size();
        }

        let mut nodes: Vec<_> = self.nodes.into_values().collect();
        nodes.sort_by(|left, right| {
            node_kind_order(left.kind)
                .cmp(&node_kind_order(right.kind))
                .then_with(|| right.value.cmp(&left.value))
                .then_with(|| left.name.cmp(&right.name))
        });

        let mut links: Vec<_> = self.links.into_values().collect();
        links.sort_by(|left, right| {
            left.source
                .cmp(&right.source)
                .then_with(|| left.target.cmp(&right.target))
        });
        let node_index = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.clone(), index))
            .collect();
        let group_node_indices = nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| (node.kind == RelationNodeKind::Group).then_some(index))
            .collect();
        let mut node_counts = RelationNodeCounts::default();
        for node in &nodes {
            match node.kind {
                RelationNodeKind::SelfUser => node_counts.self_user += 1,
                RelationNodeKind::Friend => node_counts.friend += 1,
                RelationNodeKind::Acquaintance => node_counts.acquaintance += 1,
                RelationNodeKind::Stranger => node_counts.stranger += 1,
                RelationNodeKind::Group => node_counts.group += 1,
            }
        }

        RelationGraph {
            nodes,
            links,
            node_index,
            group_node_indices,
            node_counts,
            loaded_group_count: group_members_by_room
                .keys()
                .filter(|room_id| **room_id < 0)
                .count(),
            total_group_count: rooms.iter().filter(|room| room.room_id < 0).count(),
        }
    }
}

pub(super) fn node_kind_order(kind: RelationNodeKind) -> u8 {
    match kind {
        RelationNodeKind::SelfUser => 0,
        RelationNodeKind::Friend => 1,
        RelationNodeKind::Acquaintance => 2,
        RelationNodeKind::Stranger => 3,
        RelationNodeKind::Group => 4,
    }
}
