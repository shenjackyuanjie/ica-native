use serde_json::Value as JsonValue;

/// 携带来源 bridge 标识的事件。
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeEvent {
    pub bridge_key: String,
    pub kind: BridgeEventKind,
}

impl BridgeEvent {
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

    pub fn name(&self) -> &str {
        self.kind.name()
    }

    pub fn payload(&self) -> &JsonValue {
        self.kind.payload()
    }

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
            pub fn from_protocol(name: &str, payload: JsonValue) -> Self {
                match name {
                    $( $name => Self::$variant(payload), )+
                    name => Self::Unknown { name: name.to_string(), payload },
                }
            }

            pub fn name(&self) -> &str {
                match self {
                    $( Self::$variant(_) => $name, )+
                    Self::Unknown { name, .. } => name,
                }
            }

            pub fn payload(&self) -> &JsonValue {
                match self {
                    $( Self::$variant(payload) => payload, )+
                    Self::Unknown { payload, .. } => payload,
                }
            }
        }
    };
}

bridge_event_kinds! {
    SocketConnecting => "socketConnecting",
    SocketReconnecting => "socketReconnecting",
    SocketConnected => "socketConnected",
    SocketDisconnected => "socketDisconnected",
    SocketConnectFailed => "socketConnectFailed",
    SocketRetryScheduled => "socketRetryScheduled",
    SocketReconnectExhausted => "socketReconnectExhausted",
    RequireAuth => "requireAuth",
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{BridgeEvent, BridgeEventKind};

    #[test]
    fn known_wire_event_becomes_typed_variant() {
        let event = BridgeEvent::from_wire_value(json!({
            "bridge": "primary",
            "event": "setAllRooms",
            "payload": [[{"roomId": 1}]],
        }))
        .unwrap();
        assert_eq!(event.bridge_key, "primary");
        assert!(matches!(event.kind, BridgeEventKind::SetAllRooms(_)));
    }

    #[test]
    fn unknown_wire_event_keeps_name_and_payload() {
        let payload = json!({"future": true});
        let event = BridgeEvent::from_protocol("primary", "futureEvent", payload.clone());
        assert!(matches!(
            event.kind,
            BridgeEventKind::Unknown { ref name, payload: ref kept }
                if name == "futureEvent" && kept == &payload
        ));
    }
}
