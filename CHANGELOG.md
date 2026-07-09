# Changelog

本文件记录 ica-native 的变更，遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 风格。

## [0.1.1]

### Added
- 新增「搜索聊天记录」功能
  - 在会话列表右键菜单、会话顶部工具栏均可打开搜索窗口
  - 通过 `IcaCommand::SearchMessages` 向 bridge 发起 `searchMessages` 请求，并解析 `searchMessagesResponse`
  - 支持分页加载更多结果（每页 20 条），结果去重后追加
  - 搜索结果支持回复、撤回、转发、预览图片、跳转原消息等已有消息操作
  - 新增 `MessageSearchState` 管理搜索窗口状态，新增 `message_search` 模块与对应渲染窗口

### Changed
- 将 Icalingua 兼容协议版本号 `ICA_PROTOCOL_VERSION` 从 `2.12.28` 更新到 `2.26.0`，
  以匹配 Icalingua-plus-plus 最新要求的 `EXCEPTED_PROTOCOL_VERSION`
- 将 `is_image_file_type` 可见性从 `pub(super)` 提升为 `pub(crate)`，供搜索结果图片预览复用

### Fixed
- 搜索失败时通过 `commandFailed` / `searchMessages` 错误事件回写错误信息，避免静默失败
