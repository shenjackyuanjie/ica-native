//! 关系网功能的主 viewport 协调逻辑。
//!
//! bridge 命令和延迟 viewport 动作目前由 view 模块中的 `IcaApp` 实现；
//! 此动作类型是稳定边界，让子 viewport 无需持有应用也能描述状态转换。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationAction {
    Rebuild,
    LoadGroups(Option<usize>),
    Close,
}
