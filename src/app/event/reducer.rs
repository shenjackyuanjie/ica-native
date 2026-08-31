//! Bridge 事件到本地状态的映射入口。
//!
//! 这里只做分发：事件按领域拆到同级模块，每个模块自行匹配它负责的事件名，
//! 返回 `false` 表示不认领，继续交给下一个模块。
//! 这一层故意不做任何 UI 逻辑，只维护「事件 -> 状态」的映射。

use serde_json::Value as JsonValue;

use crate::app::IcaApp;
use crate::app::state::BridgeState;
use crate::ica::BridgeEventKind;

use super::{
    announcement, connection, contacts, forward, group, login, message, misc, room, search,
};

/// 领域事件处理器：认领并处理了事件时返回 true。
type EventHandler = fn(&mut BridgeState, &str, &JsonValue) -> bool;

/// 各领域负责的事件名互不重叠，因此这里的顺序只影响匹配开销，不影响结果。
/// 高频事件排在前面，省去逐个模块比对事件名的开销。
const HANDLERS: &[EventHandler] = &[
    message::apply,
    room::apply,
    connection::apply,
    search::apply,
    forward::apply,
    group::apply,
    announcement::apply,
    contacts::apply,
    login::apply,
    misc::apply,
];

impl IcaApp {
    /// 把某个 bridge 发来的事件应用到对应的本地状态上。
    ///
    /// 没有任何模块认领的事件会被静默忽略，以便新版 Bridge 增加事件时旧客户端仍能运行。
    pub(in crate::app) fn apply_bridge_event(state: &mut BridgeState, event: &BridgeEventKind) {
        let event_name = event.name();
        let payload = event.payload();
        for handler in HANDLERS {
            if handler(state, event_name, payload) {
                return;
            }
        }
    }
}
