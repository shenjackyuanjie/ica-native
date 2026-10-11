//! 聊天分组：会话的分组容器与分组内房间的增删改查。
//!
//! 分组数据由 bridge 下发并在本地同步回 bridge，因此虽然位于 `config` 下，
//! 却不写入配置文件（`IcaCfg::chat_groups` 标了 `#[serde(skip)]`）。
//! 本模块只维护数据与规则，界面编辑位于 `app::chat_groups`。

#[cfg(test)]
#[path = "tests/chat_groups.rs"]
mod tests;

use crate::ica::types::RoomId;
use crate::ica::types::room::Room;
use serde::{Deserialize, Serialize};

/// 全部分组的集合；顺序即界面展示顺序（可用 `move_group` 调整）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatGroups {
    /// 分组列表。
    pub groups: Vec<ChatGroup>,
}

impl ChatGroups {
    /// 创建空分组集合。
    pub fn new() -> Self {
        Self { groups: Vec::new() }
    }

    /// 按顺序取所有分组名（克隆出新 `Vec`，调用方可自由修改）。
    pub fn group_names(&self) -> Vec<String> {
        self.groups.iter().map(|g| g.name()).collect()
    }

    /// 在末尾追加一个分组。
    pub fn add_group(&mut self, group: ChatGroup) {
        self.groups.push(group);
    }

    /// 按下标删除分组；下标越界时安全跳过（不 panic）。
    pub fn remove_group(&mut self, index: usize) {
        if index < self.groups.len() {
            self.groups.remove(index);
        }
    }

    /// 重命名指定分组；下标越界时安全跳过。
    pub fn rename_group(&mut self, index: usize, new_name: String) {
        if let Some(group) = self.groups.get_mut(index) {
            group.name = new_name;
        }
    }

    /// 切换某个房间是否属于指定分组。
    ///
    /// 返回 `true` 表示本次是「加入」，`false` 表示「移出」；
    /// 分组不存在时返回 `false`。
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

    /// 判断房间是否已在指定分组中；分组不存在时返回 `false`。
    pub fn is_room_in_group(&self, group_index: usize, room_id: RoomId) -> bool {
        self.groups
            .get(group_index)
            .map(|g| g.rooms.contains(&room_id))
            .unwrap_or(false)
    }

    /// 判断分组内是否有未读会话。
    ///
    /// 计入两种房间：显式加入分组的，以及 `include_all_personal` 开启时的
    /// 全部私聊（`room_id > 0`，与 QQ 的私聊 ID 约定一致）。
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

    /// 把分组从 `from` 挪到 `to`（拖动排序用）；任一下标越界时安全跳过。
    pub fn move_group(&mut self, from: usize, to: usize) {
        if from < self.groups.len() && to < self.groups.len() {
            let group = self.groups.remove(from);
            self.groups.insert(to, group);
        }
    }
}

/// `Default` 实现：等价于空集合。
impl Default for ChatGroups {
    fn default() -> Self {
        Self::new()
    }
}

/// 一个聊天分组。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatGroup {
    /// 分组名（界面展示用，可重命名）。
    pub name: String,
    /// 显式加入该分组的房间 ID 列表。
    #[serde(default)]
    pub rooms: Vec<RoomId>,
    /// 是否自动包含全部私聊会话。
    ///
    /// 反序列化时接受旧命名的 `includeAllPersonal`，并兼容 snake_case 别名。
    #[serde(
        default,
        rename(deserialize = "includeAllPersonal"),
        alias = "include_all_personal"
    )]
    pub include_all_personal: bool,
}

impl ChatGroup {
    /// 用名称与房间列表创建分组（不自动包含私聊）。
    pub fn new(name: impl Into<String>, rooms: Vec<RoomId>) -> Self {
        Self {
            name: name.into(),
            rooms,
            include_all_personal: false,
        }
    }

    /// 创建只有名称的空分组。
    pub fn new_empty(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            rooms: Vec::new(),
            include_all_personal: false,
        }
    }

    /// 取分组名（克隆，避免调用方持有内部引用）。
    pub fn name(&self) -> String {
        self.name.clone()
    }
}
