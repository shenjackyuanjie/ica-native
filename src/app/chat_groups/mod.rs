//! 按会话维度的聊天分组功能与分组编辑器。
//!
//! 本模块把 `config::chat_groups` 里的分组数据模型重新导出给界面使用，并挂载
//! 分组编辑器子模块；分组数据的持久化与校验都在 config 侧完成。

mod editor;

pub use crate::config::chat_groups::{ChatGroup, ChatGroups};
pub use editor::ChatGroupEditor;
