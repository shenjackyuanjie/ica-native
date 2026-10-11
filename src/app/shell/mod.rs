//! 应用外壳：负责顶层导航、面板和窗口调度。
//!
//! “外壳”指主窗口的固定框架：顶部菜单栏、最左侧的聊天分组栏、会话列表面板，
//! 以及各功能窗口的统一渲染入口。这里只放框架性代码，具体窗口内容由
//! `tools`、`chat` 等模块提供，经 `IcaApp` 的 `render_*` 方法在此调用。

mod about;
mod chat_list;
mod left_groups;
mod top_panel;
mod windows;
