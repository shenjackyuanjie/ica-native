# ica-native

icalingua but native

> 其实就是 [shenbot](https://github.com/shenjackyuanjie/icalingua-bridge-bot) 的客户端版本
>
> 所以技术栈几乎就是把那边的东西 cp 了一份过来

- 非常感谢以下 AI 在开发过程中的贡献
- 排名不分先后（想起来谁是谁）
  - Kimi K2.5
  - GPT 5.2
  - GPT 5.5
  - GPT 5.6 Sol
  - GPT 5.6 Luna
  - DeepSeek V4 Flash Preview
  - DeepSeek V4 Flash 0731
  - Claude Opus 4.8
  - Claude Fable 5 (只用了一次)
  - Kimi K2 thinking

## 聊天交互（Socket.IO + oicq Bridge）

- 双击群消息头像或私聊顶栏头像戳一戳；群头像右键可 `@TA`、私聊、查看头像、发言记录及成员管理。
- 双击消息行空白/气泡背景回复；双击正文仍选词。`Ctrl+↑/↓` 选择回复并定位高亮。
- `Ctrl+Tab` / `Ctrl+Shift+Tab` 或 `Alt+↑/↓` 切换当前筛选内的会话；`Ctrl+1/2/3` 切换所有/群/私聊，`Ctrl+4..9` 切换前六个自定义分类。
- 主聊天区无控件焦点时，`Tab` 跳到当前筛选内优先级最高的未读会话；输入框/按钮仍保留 Tab 焦点导航。
- `Ctrl+E` 打开表情，`Ctrl+N` / `Ctrl+M` 打开群成员选择。在定制聊天设置中可选择 Enter、Ctrl+Enter 或 Shift+Enter 发送。
- 空草稿按 `↑` 编辑重发上一条自己的可恢复图文消息。只有 Bridge 明确接收后才请求撤回原消息；失败保留草稿，不自动重发。HTTP 接收不代表 QQ 已送达，网络超时后请先检查记录，避免重复发送。
- `Esc` 逐层取消弹层、草稿/回复/编辑、附件和转发选择，最后关闭会话；发送中的编辑草稿不允许取消。

QQ NT 图片加载失败时会向来源 Bridge 请求新链接并重试一次，包括回复、合并转发和图片预览。失败后冷却 60 秒；链接来源有歧义时不猜测账号，也不保证已经无法从 QQ 获取的图片能够恢复。

## 内建 Noticer

`ica-native` 可以直接提供兼容 noticer `0.4.2` 的 HTTP webhook，并按配置的
`bridge + room_id` 把提醒稳定路由到对应 Bridge。默认关闭；配置示例：

```toml
[[bridges]]
name = "main"
url = "ws://127.0.0.1:9999"
private_key = "<bridge-private-key>"
enable = true
# 可在服务端 Bridge 版本较旧时开启兼容连接；客户端会显示版本告警。
allow_protocol_mismatch = false

[noticer]
enabled = true
host = "127.0.0.1"
port = 10020
auth_token = "<random-bearer-token>"
direct_token = "<separate-direct-api-token>"
default_bridge = "main"
queue_capacity = 256
send_timeout_seconds = 60.0
retry_attempts = 3
retry_delay_seconds = 1.0

# 请求、图片、队列和幂等限制均可按部署需要调节。
max_body_size_bytes = 75497472
max_image_size_bytes = 8388608
max_image_count = 9
max_total_image_size_bytes = 50331648
max_queued_image_bytes = 134217728
idempotency_ttl_seconds = 600
idempotency_max_entries = 2048

[noticer.rooms.notice]
bridge = "main"
room_id = -1000000001
description = "一般提醒"

[noticer.rooms.warning]
bridge = "main"
room_id = -1000000002
description = "警告提醒"
```

常用接口：

- `POST /send`：兼容旧版 noticer 请求。
- `POST /v1/send`：按配置房间名发送，支持 `Idempotency-Key`。
- `POST /v1/send/direct`：使用独立 Token 按 `room_id` 发送。
- `GET /status`：查看 Bridge、房间和队列状态。
- `GET /config`：展示脱敏后的全部有效配置、支持的图片类型、接口列表，以及程序内仍存在的硬上限；不会返回 Token 原文。

客户端的「选项 → Noticer 设置」提供同一份配置的可视化编辑；勾选或取消「启用 Noticer」会立即启停服务，修改监听地址、Token、限制和房间路由后点击保存会立即重建服务。

监听非回环地址时必须配置 `auth_token`。HTTP 成功响应只表示消息已提交给 Bridge，
不代表 QQ 服务端已经最终送达。
