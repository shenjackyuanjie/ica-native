//! Bridge 事件的类型化封装。
//!
//! Bridge 与服务端之间传的是「事件名 + JSON 载荷」，本模块把它变成带编译期约束的
//! `BridgeEventKind`，让 GUI 侧可以穷尽匹配而不必到处写字符串比较：
//! - `BridgeEvent`：事件 + 来源 bridge 标识，GUI 主线程只消费这一种类型；
//! - `BridgeEventKind`：由 `bridge_event_kinds!` 宏按「变体 -> 协议事件名」清单生成，
//!   名字与载荷的映射集中在文件末尾的宏调用处维护；
//! - 未登记的协议事件落入 `Unknown` 变体保留原名与载荷，保证 Bridge 升级后
//!   新事件不会让客户端解析失败。
//!
//! 从线格式（JSON 对象）还原事件用 `BridgeEvent::from_wire_value`，
//! 字段为 `bridge` / `event` / `payload`。

#[cfg(test)]
#[path = "tests/event.rs"]
mod tests;

use serde_json::Value as JsonValue;

/// 携带来源 bridge 标识的事件。
///
/// 多 bridge 场景下同名事件需要按 `bridge_key` 分流，因此标识与事件内容一起传递，
/// 而不是依赖全局"当前 bridge"。
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeEvent {
    /// 事件来源 bridge 的标识（配置名，未配置时退回 url）。
    pub bridge_key: String,
    /// 事件种类与载荷。
    pub kind: BridgeEventKind,
}

impl BridgeEvent {
    /// 用 bridge 标识、协议事件名与载荷构造事件。
    ///
    /// 事件名不在已知清单中时归入 `BridgeEventKind::Unknown`。
    pub fn from_protocol(
        bridge_key: impl Into<String>,
        name: impl AsRef<str>,
        payload: JsonValue,
    ) -> Self {
        Self {
            bridge_key: bridge_key.into(),
            kind: BridgeEventKind::from_protocol(name.as_ref(), payload),
        }
    }

    /// 返回协议事件名（对 `Unknown` 变体即其原始名字）。
    pub fn name(&self) -> &str {
        self.kind.name()
    }

    /// 返回事件载荷的引用。
    pub fn payload(&self) -> &JsonValue {
        self.kind.payload()
    }

    /// 从线格式 JSON 还原事件，供跨进程/持久化场景使用。
    ///
    /// 期望形状为 `{"bridge": ..., "event": ..., "payload": ...}`；
    /// 缺少 `bridge` 或 `event` 字段时返回错误文本，`payload` 缺失时按 `Null` 处理。
    pub fn from_wire_value(value: JsonValue) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "bridge 事件必须是对象".to_string())?;
        let bridge_key = object
            .get("bridge")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| "bridge 事件缺少 bridge 字段".to_string())?;
        let name = object
            .get("event")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| "bridge 事件缺少 event 字段".to_string())?;
        Ok(Self::from_protocol(
            bridge_key,
            name,
            object.get("payload").cloned().unwrap_or(JsonValue::Null),
        ))
    }
}

macro_rules! bridge_event_kinds {
    ($( $variant:ident => $name:literal ),+ $(,)?) => {
        /// 应用当前会处理的所有协议事件和内部事件。
        #[derive(Debug, Clone, PartialEq)]
        pub enum BridgeEventKind {
            $( $variant(JsonValue), )+
            /// 当前版本尚不认识、但为向前兼容而保留的协议事件。
            Unknown { name: String, payload: JsonValue },
        }

        impl BridgeEventKind {
            /// 按协议事件名解析事件；未登记的 name 归入 `Unknown`。
            ///
            /// 这是 Bridge 向前兼容的关键入口：新事件只会多出一个 `Unknown`，
            /// 不会导致解析失败或事件丢失。
            pub fn from_protocol(name: &str, payload: JsonValue) -> Self {
                match name {
                    $( $name => Self::$variant(payload), )+
                    name => Self::Unknown { name: name.to_string(), payload },
                }
            }

            /// 返回协议事件名；`Unknown` 变体返回其原始名字。
            pub fn name(&self) -> &str {
                match self {
                    $( Self::$variant(_) => $name, )+
                    Self::Unknown { name, .. } => name,
                }
            }

            /// 返回事件载荷的引用。
            pub fn payload(&self) -> &JsonValue {
                match self {
                    $( Self::$variant(payload) => payload, )+
                    Self::Unknown { payload, .. } => payload,
                }
            }
        }
    };
}

// 协议事件名清单：左侧为 Rust 变体，右侧为 Bridge 实际下发的事件名。
// 新增 Bridge 事件时在这里加一行即可，`BridgeEventKind` 的三个方法会自动覆盖它。
// 连接状态类（socketConnecting 等）与登录流程类（login-*）事件虽然由本客户端自己
// 产生或消费，也统一登记在这里，便于 GUI 侧用同一套类型匹配。
bridge_event_kinds! {
    EditSendResult => "editSendResult",
    VoiceSendResult => "voiceSendResult",
    SocketConnecting => "socketConnecting",
    SocketReconnecting => "socketReconnecting",
    SocketConnected => "socketConnected",
    SocketDisconnected => "socketDisconnected",
    SocketConnectFailed => "socketConnectFailed",
    SocketRetryScheduled => "socketRetryScheduled",
    SocketReconnectExhausted => "socketReconnectExhausted",
    RequireAuth => "requireAuth",
    BridgeVersionInfo => "bridgeVersionInfo",
    BridgeProtocolMismatch => "bridgeProtocolMismatch",
    AuthSucceed => "authSucceed",
    AuthFailed => "authFailed",
    Message => "message",
    OnlineData => "onlineData",
    AddMessage => "addMessage",
    DeleteMessage => "deleteMessage",
    HideMessage => "hideMessage",
    RevealMessage => "revealMessage",
    SetAllRooms => "setAllRooms",
    SetAllChatGroups => "setAllChatGroups",
    SetMessages => "setMessages",
    AppendOlderMessages => "appendOlderMessages",
    HandleRequest => "handleRequest",
    SendAddRequest => "sendAddRequest",
    UpdateRoom => "updateRoom",
    SyncRead => "syncRead",
    RenewMessage => "renewMessage",
    RenewMessageUrl => "renewMessageURL",
    SetOnline => "setOnline",
    SetOffline => "setOffline",
    SetShutUp => "setShutUp",
    MessageSuccess => "messageSuccess",
    MessageError => "messageError",
    AddMessageText => "addMessageText",
    NotifyMessage => "notifyMessage",
    CloseLoading => "closeLoading",
    NotifyError => "notifyError",
    DbUpgradeProgress => "dbUpgradeProgress",
    RequestSetup => "requestSetup",
    Fatal => "fatal",
    LoginVerify => "login-verify",
    LoginQrcode => "login-qrcodeLogin",
    LoginSmsCode => "login-smsCodeVerify",
    LoginError => "login-error",
    LoginSlider => "login-slider",
    SetSystemMessages => "setSystemMessages",
    ContactsPartResponse => "contactsPartResponse",
    ContactsPartFailed => "contactsPartFailed",
    CommandFailed => "commandFailed",
    SearchMessagesResponse => "searchMessagesResponse",
    ForwardMessagesResponse => "forwardMessagesResponse",
    ForwardMessagesFailed => "forwardMessagesFailed",
    ForwardSendRequested => "forwardSendRequested",
    GroupMembersResponse => "groupMembersResponse",
    GroupAnnouncementsResponse => "groupAnnouncementsResponse",
    GroupAnnouncementsFailed => "groupAnnouncementsFailed",
    GroupAnnouncementActionDone => "groupAnnouncementActionDone",
    GroupAnnouncementActionFailed => "groupAnnouncementActionFailed",
    GroupBanRequested => "groupBanRequested",
    SocketApiResponse => "socketApiResponse",
    FileManagerResponse => "fileManagerResponse",
}
