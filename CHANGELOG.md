# Changelog

本文件记录 ica-native 的变更，遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 风格。

## [0.1.4]

### Added
- 支持通过配置调整 QQ 关系网渲染节点数、连线数、标签数和自动降级阈值。

## [0.1.3]

### Added
- 新增「QQ 关系网」原生窗口，可从顶部「选项」菜单打开。
- 支持基于现有好友会话、群会话和群成员缓存构建关系网节点与边。
- 支持按节点类型筛选自己、好友、共同群好友、仅同群和群节点，并显示节点数量统计。
- 支持批量加载群成员、重建关系网、搜索昵称/QQ/群号，以及点击节点查看一跳关系和节点详情。

### Changed
- 关系网随 `onlineData`、`setAllRooms` 和 `groupMembersResponse` 自动重建，并在大图场景下自动关闭部分高成本显示项。
- 使用轻量原生径向布局渲染关系图，避免引入 Flask/ECharts/NapCat 侧服务依赖。

## [0.1.2]

### Changed
- 降低默认图片内存缓存预算到 128MiB，减少图片密集会话下的常驻内存。
- 表情 APNG 改为按需从 `assets/face` 读取，并使用小型 LRU 缓存，避免启动时常驻全部表情字节。
- GIF 预览增加帧数和解码后字节预算，避免长动图一次性解码所有帧造成内存峰值。
- 聊天消息增加渲染用时间文本缓存，减少消息列表重绘时的重复格式化。
- 会话列表过滤/排序结果增加 revision 缓存，减少每帧重复分配、过滤和排序。

### Fixed
- 限制房间消息、搜索结果和消息布局缓存增长，切换会话时清理非活跃布局缓存。
- 裁剪历史消息缓存时区分新消息追加和旧历史 prepend，避免加载旧历史后错误丢弃刚拉到的旧消息。
- 原始消息链只在可用于原样转发时保留，降低历史消息结构的重复 JSON 占用。

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
