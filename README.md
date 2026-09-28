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

## 内建 Noticer

`ica-native` 可以直接提供兼容 noticer `0.4.2` 的 HTTP webhook，并按配置的
`bridge + room_id` 把提醒稳定路由到对应 Bridge。默认关闭；配置示例：

```toml
[[bridges]]
name = "main"
url = "ws://127.0.0.1:9999"
private_key = "<bridge-private-key>"
enable = true
# 只建议在临时兼容旧 Bridge 时开启。
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

监听非回环地址时必须配置 `auth_token`。HTTP 成功响应只表示消息已提交给 Bridge，
不代表 QQ 服务端已经最终送达。
