//! QQ 关系网可视化：从房间与成员数据构建关系图并交互渲染。
//!
//! 模块按职责拆分：`model` 定义图数据，`layout` 负责力导向布局与缓存，
//! `state` 持有界面状态，`controller` 把界面操作转成数据加载/变更命令，
//! `view` 下各文件负责画布、侧边栏与浮层等绘制，`theme` 集中配色。

mod controller;
mod layout;
mod model;
mod state;
mod theme;
mod view;

pub use state::RelationNetworkState;
