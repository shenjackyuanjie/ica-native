use serde_json::Value as JsonValue;

use crate::ica::IcaCommand;

use crate::app::IcaApp;

pub struct SocketApiPreset {
    pub label: &'static str,
    pub event: &'static str,
    pub args: &'static str,
    pub expect_ack: bool,
    pub note: &'static str,
}

const SOCKET_API_PRESETS: &[SocketApiPreset] = &[
    SocketApiPreset {
        label: "自定义",
        event: "",
        args: "[]",
        expect_ack: true,
        note: "手动输入事件名和 JSON 参数数组。",
    },
    SocketApiPreset {
        label: "获取群成员",
        event: "getGroupMembers",
        args: "[123456]",
        expect_ack: true,
        note: "参数: 群号 gin。",
    },
    SocketApiPreset {
        label: "获取群成员资料",
        event: "getGroupMemberInfo",
        args: "[123456, 10000, true]",
        expect_ack: true,
        note: "参数: 群号 gin, QQ, noCache。",
    },
    SocketApiPreset {
        label: "获取好友资料",
        event: "getFriend",
        args: "[10000]",
        expect_ack: true,
        note: "参数: QQ。",
    },
    SocketApiPreset {
        label: "获取群资料",
        event: "getGroup",
        args: "[123456]",
        expect_ack: true,
        note: "参数: 群号 gin。",
    },
    SocketApiPreset {
        label: "获取房间",
        event: "getRoom",
        args: "[-123456]",
        expect_ack: true,
        note: "参数: roomId，群聊为负数。",
    },
    SocketApiPreset {
        label: "获取群列表",
        event: "getGroups",
        args: "[]",
        expect_ack: true,
        note: "无参数。",
    },
    SocketApiPreset {
        label: "获取好友 fallback",
        event: "getFriendsFallback",
        args: "[]",
        expect_ack: true,
        note: "无参数。",
    },
    SocketApiPreset {
        label: "获取忽略会话",
        event: "getIgnoredChats",
        args: "[]",
        expect_ack: true,
        note: "无参数。",
    },
    SocketApiPreset {
        label: "获取登录设备",
        event: "getLoginDevices",
        args: "[]",
        expect_ack: true,
        note: "无参数。",
    },
    SocketApiPreset {
        label: "删除登录设备",
        event: "deleteLoginDevice",
        args: "[\"flag\"]",
        expect_ack: false,
        note: "参数: 设备 flag。",
    },
    SocketApiPreset {
        label: "获取系统消息",
        event: "getSystemMsg",
        args: "[]",
        expect_ack: true,
        note: "好友/群验证消息。",
    },
    SocketApiPreset {
        label: "处理验证消息",
        event: "handleRequest",
        args: "[\"group\", \"flag\", true]",
        expect_ack: false,
        note: "参数: friend/group, flag, accept。",
    },
    SocketApiPreset {
        label: "获取漫游表情",
        event: "getRoamingStamp",
        args: "[false]",
        expect_ack: true,
        note: "参数: no_cache。",
    },
    SocketApiPreset {
        label: "获取转发消息",
        event: "getForwardMsg",
        args: "[\"resId\", \"fileName\"]",
        expect_ack: true,
        note: "参数: resId, 可选 fileName。",
    },
    SocketApiPreset {
        label: "构造合并转发",
        event: "makeForward",
        args: "[[], false, -123456, -123456]",
        expect_ack: false,
        note: "参数: fakes, dm, origin, target。",
    },
    SocketApiPreset {
        label: "搜索消息",
        event: "searchMessages",
        args: "[-123456, \"keyword\", 0]",
        expect_ack: true,
        note: "参数: roomId, keyword, offset。",
    },
    SocketApiPreset {
        label: "取会话消息",
        event: "fetchMessages",
        args: "[-123456, 0]",
        expect_ack: true,
        note: "参数: roomId, offset。",
    },
    SocketApiPreset {
        label: "按发送者取消息",
        event: "fetchMessagesBySender",
        args: "[-123456, 10000, 0]",
        expect_ack: true,
        note: "参数: roomId, senderId, offset。",
    },
    SocketApiPreset {
        label: "取图片消息",
        event: "fetchImageMessages",
        args: "[-123456, 0, null]",
        expect_ack: true,
        note: "参数: roomId, offset, endTime。",
    },
    SocketApiPreset {
        label: "拉取历史",
        event: "fetchHistory",
        args: "[\"messageId\", -123456, 20]",
        expect_ack: false,
        note: "参数: messageId(base64), roomId, currentLoadedMessagesCount。",
    },
    SocketApiPreset {
        label: "拉取 7 天历史",
        event: "fetch7DaysHistory",
        args: "[]",
        expect_ack: false,
        note: "无参数。",
    },
    SocketApiPreset {
        label: "围绕消息取上下文",
        event: "fetchMessagesAround",
        args: "[-123456, \"messageId\", 20, 20]",
        expect_ack: true,
        note: "参数: roomId, messageId, before, after。",
    },
    SocketApiPreset {
        label: "获取首次未读",
        event: "getFirstUnreadRoom",
        args: "[3]",
        expect_ack: true,
        note: "参数: priority。",
    },
    SocketApiPreset {
        label: "获取未读数",
        event: "getUnreadCount",
        args: "[]",
        expect_ack: true,
        note: "无参数。",
    },
    SocketApiPreset {
        label: "更新房间字段",
        event: "updateRoom",
        args: "[-123456, {\"unreadCount\": 0, \"at\": false}]",
        expect_ack: false,
        note: "参数: roomId, partial room。",
    },
    SocketApiPreset {
        label: "添加房间",
        event: "addRoom",
        args: "[{\"roomId\": -123456, \"roomName\": \"群聊\", \"index\": 0, \"unreadCount\": 0, \"priority\": 3, \"utime\": 0, \"at\": false, \"lastMessage\": {}}]",
        expect_ack: false,
        note: "参数: Room 对象。",
    },
    SocketApiPreset {
        label: "添加聊天分组",
        event: "addChatGroup",
        args: "[{\"name\": \"分组\", \"rooms\": [], \"includeAllPersonal\": false}]",
        expect_ack: false,
        note: "参数: ChatGroup 对象。",
    },
    SocketApiPreset {
        label: "更新聊天分组",
        event: "updateChatGroup",
        args: "[\"分组\", {\"name\": \"分组\", \"rooms\": [], \"includeAllPersonal\": false}]",
        expect_ack: false,
        note: "参数: 原分组名, ChatGroup 对象。",
    },
    SocketApiPreset {
        label: "删除聊天分组",
        event: "removeChatGroup",
        args: "[\"分组\"]",
        expect_ack: false,
        note: "参数: 分组名。",
    },
    SocketApiPreset {
        label: "忽略会话",
        event: "ignoreChat",
        args: "[{\"id\": -123456, \"name\": \"群聊\"}]",
        expect_ack: false,
        note: "参数: IgnoreChatInfo。",
    },
    SocketApiPreset {
        label: "移除忽略会话",
        event: "removeIgnoredChat",
        args: "[-123456]",
        expect_ack: false,
        note: "参数: roomId。",
    },
    SocketApiPreset {
        label: "移除会话",
        event: "removeChat",
        args: "[-123456]",
        expect_ack: false,
        note: "参数: roomId。",
    },
    SocketApiPreset {
        label: "置顶会话",
        event: "pinRoom",
        args: "[-123456, true]",
        expect_ack: false,
        note: "参数: roomId, pin。",
    },
    SocketApiPreset {
        label: "设置会话优先级",
        event: "setRoomPriority",
        args: "[-123456, 3]",
        expect_ack: false,
        note: "参数: roomId, priority(1-5)。",
    },
    SocketApiPreset {
        label: "更新消息字段",
        event: "updateMessage",
        args: "[-123456, \"messageId\", {\"content\": \"text\"}]",
        expect_ack: false,
        note: "参数: roomId, messageId, partial message。",
    },
    SocketApiPreset {
        label: "发送消息",
        event: "sendMessage",
        args: "[{\"content\": \"text\", \"roomId\": -123456, \"replyMessage\": null, \"at\": []}]",
        expect_ack: false,
        note: "参数: SendMessageParams。",
    },
    SocketApiPreset {
        label: "撤回消息",
        event: "deleteMessage",
        args: "[-123456, \"messageId\"]",
        expect_ack: false,
        note: "参数: roomId, messageId。",
    },
    SocketApiPreset {
        label: "隐藏消息",
        event: "hideMessage",
        args: "[-123456, \"messageId\"]",
        expect_ack: false,
        note: "参数: roomId, messageId。",
    },
    SocketApiPreset {
        label: "显示隐藏消息",
        event: "revealMessage",
        args: "[-123456, \"messageId\"]",
        expect_ack: false,
        note: "参数: roomId, messageId。",
    },
    SocketApiPreset {
        label: "刷新消息内容",
        event: "renewMessage",
        args: "[-123456, \"messageId\", null]",
        expect_ack: false,
        note: "参数: roomId, messageId, message。",
    },
    SocketApiPreset {
        label: "刷新消息 URL",
        event: "renewMessageURL",
        args: "[-123456, \"messageId\", \"https://example.com/image.png\"]",
        expect_ack: false,
        note: "参数: roomId, messageId, URL。",
    },
    SocketApiPreset {
        label: "设置群名片",
        event: "setGroupNick",
        args: "[123456, \"nick\"]",
        expect_ack: false,
        note: "参数: 群号 gin, nick。",
    },
    SocketApiPreset {
        label: "设置群备注",
        event: "setGroupRemark",
        args: "[123456, \"remark\"]",
        expect_ack: false,
        note: "参数: 群号 gin, remark。",
    },
    SocketApiPreset {
        label: "设置好友备注",
        event: "setFriendRemark",
        args: "[10000, \"remark\"]",
        expect_ack: false,
        note: "参数: QQ, remark。",
    },
    SocketApiPreset {
        label: "群禁言",
        event: "setGroupBan",
        args: "[123456, 10000, 600]",
        expect_ack: false,
        note: "参数: 群号 gin, QQ, 秒数。",
    },
    SocketApiPreset {
        label: "匿名禁言",
        event: "setGroupAnonymousBan",
        args: "[123456, \"flag\", 600]",
        expect_ack: false,
        note: "参数: 群号 gin, anonymous flag, 秒数。",
    },
    SocketApiPreset {
        label: "踢出群成员",
        event: "setGroupKick",
        args: "[123456, 10000]",
        expect_ack: false,
        note: "参数: 群号 gin, QQ。",
    },
    SocketApiPreset {
        label: "退出群",
        event: "setGroupLeave",
        args: "[123456]",
        expect_ack: false,
        note: "参数: 群号 gin。",
    },
    SocketApiPreset {
        label: "获取群文件元信息",
        event: "getGroupFileMeta",
        args: "[123456, \"fid\"]",
        expect_ack: true,
        note: "参数: 群号 gin, fid。",
    },
    SocketApiPreset {
        label: "获取私聊文件 URL",
        event: "getPrivateFileUrl",
        args: "[\"fileId\"]",
        expect_ack: true,
        note: "参数: fileId。",
    },
    SocketApiPreset {
        label: "获取群文件 token",
        event: "requestGfsToken",
        args: "[123456]",
        expect_ack: true,
        note: "参数: 群号 gin。",
    },
    SocketApiPreset {
        label: "请求图片上传 token",
        event: "requestToken",
        args: "[]",
        expect_ack: true,
        note: "无参数。",
    },
    SocketApiPreset {
        label: "请求分块上传",
        event: "requestUpload",
        args: "[\"file.bin\", \"sha256\", 1024]",
        expect_ack: true,
        note: "参数: fileName, hash, fileSize。",
    },
    SocketApiPreset {
        label: "上传文件分块",
        event: "uploadFile",
        args: "[\"sha256\", 0, [1, 2, 3], \"chunkSha256\"]",
        expect_ack: true,
        note: "参数: fileHash, offset, bytes, chunkHash。",
    },
    SocketApiPreset {
        label: "获取 cookies",
        event: "getCookies",
        args: "[\"qq.com\"]",
        expect_ack: true,
        note: "参数: domain。",
    },
    SocketApiPreset {
        label: "获取禁用功能",
        event: "getDisabledFeatures",
        args: "[]",
        expect_ack: true,
        note: "无参数。",
    },
    SocketApiPreset {
        label: "获取新图片 URL",
        event: "getMsgNewURL",
        args: "[\"messageId\"]",
        expect_ack: true,
        note: "参数: messageId。",
    },
    SocketApiPreset {
        label: "发送底层包",
        event: "sendPacket",
        args: "[\"type\", \"cmd\", {}]",
        expect_ack: true,
        note: "危险操作: type, cmd, body。",
    },
    SocketApiPreset {
        label: "重新登录",
        event: "reLogin",
        args: "[]",
        expect_ack: false,
        note: "请求 bridge 重连账号。",
    },
    SocketApiPreset {
        label: "远端登录",
        event: "login",
        args: "[{\"username\": 10000, \"password\": \"\", \"platform\": 5}]",
        expect_ack: false,
        note: "参数: LoginForm。建议优先用 Icalingua++ 完成登录。",
    },
    SocketApiPreset {
        label: "验证窗口关闭后重登",
        event: "login-verify-reLogin",
        args: "[]",
        expect_ack: false,
        note: "无参数。",
    },
    SocketApiPreset {
        label: "随机设备",
        event: "randomDevice",
        args: "[10000]",
        expect_ack: false,
        note: "参数: 账号 QQ。",
    },
    SocketApiPreset {
        label: "提交短信验证码",
        event: "submitSmsCode",
        args: "[\"123456\"]",
        expect_ack: false,
        note: "参数: smsCode。",
    },
    SocketApiPreset {
        label: "提交滑块 ticket",
        event: "login-slider-ticket",
        args: "[\"ticket\"]",
        expect_ack: false,
        note: "参数: ticket。",
    },
    SocketApiPreset {
        label: "群签到",
        event: "sendGroupSign",
        args: "[123456]",
        expect_ack: false,
        note: "参数: 群号 gin。",
    },
    SocketApiPreset {
        label: "戳一戳",
        event: "sendGroupPoke",
        args: "[123456, 10000]",
        expect_ack: false,
        note: "参数: 群号 gin, QQ。",
    },
    SocketApiPreset {
        label: "设置在线状态",
        event: "setOnlineStatus",
        args: "[11]",
        expect_ack: false,
        note: "参数: 在线状态值，在线 11 / 离开 31 / 隐身 41 / 忙碌 50 / Q我吧 60 / 请勿打扰 70。",
    },
    SocketApiPreset {
        label: "设置自动下载",
        event: "setRoomAutoDownload",
        args: "[-123456, true]",
        expect_ack: false,
        note: "参数: roomId, autoDownload。",
    },
    SocketApiPreset {
        label: "设置自动下载路径",
        event: "setRoomAutoDownloadPath",
        args: "[-123456, \"D:/Downloads\"]",
        expect_ack: false,
        note: "参数: roomId, downloadPath。",
    },
];

impl IcaApp {
    pub fn socket_api_presets() -> &'static [SocketApiPreset] {
        SOCKET_API_PRESETS
    }

    pub fn apply_socket_api_preset(&mut self, index: usize) {
        let Some(preset) = SOCKET_API_PRESETS.get(index) else {
            return;
        };
        self.socket_api_preset_idx = index;
        if !preset.event.is_empty() {
            self.socket_api_event = preset.event.to_string();
            self.socket_api_args = preset.args.to_string();
            self.socket_api_expect_ack = preset.expect_ack;
        }
    }

    pub fn send_socket_api_call(&mut self) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };

        let event = self.socket_api_event.trim().to_string();
        if event.is_empty() {
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some("Socket API 事件名不能为空".to_string());
            }
            return;
        }

        let args_value: JsonValue = match serde_json::from_str(self.socket_api_args.trim()) {
            Ok(value) => value,
            Err(e) => {
                if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                    state.last_error = Some(format!("Socket API 参数不是合法 JSON: {}", e));
                }
                return;
            }
        };
        let args = match args_value {
            JsonValue::Array(values) => values,
            value => vec![value],
        };

        self.send_socket_api_event(event, args, self.socket_api_expect_ack);
    }

    pub fn send_socket_api_event(
        &mut self,
        event: impl Into<String>,
        args: Vec<JsonValue>,
        expect_ack: bool,
    ) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };

        let command = IcaCommand::SocketApiCall {
            event: event.into(),
            args,
            expect_ack,
        };
        if let Err(e) = self.bridge_states[bridge_idx].send(command) {
            tracing::warn!("send socket api command failed: {}", e);
            if let Some(state) = self.bridge_states.get_mut(bridge_idx) {
                state.last_error = Some("Socket API 命令发送失败".to_string());
            }
        }
    }
}
