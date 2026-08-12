# ica-native 从 egui 迁移至 GPUI

## 目标

在独立分支 `refactor/gpui` 中一次性将 ica-native 的 UI 从 egui/eframe 重写为 GPUI。

最终版本：

- 完全删除 egui、eframe、egui_extras。
- 保留现有 Socket.IO、Bridge、配置和消息处理逻辑。
- UI 基础结构参考 Icalingua 本体。
- 颜色、主题、按钮、菜单和弹层采用 Zed 风格。
- Windows 为主要目标平台。
- 不维护 egui/GPUI 双前端。
- 不新增 UI 自动化测试。

## 依赖策略

固定使用 Zed 提交：

```text
c7537bdf463a998e7ec636adff33b198891e69ed
```

从该提交引用 `gpui`、`gpui_platform`、`gpui_tokio`、`ui`、`theme` 和 `assets`。所有依赖使用 Git `rev`，禁止引用本机 `D:\githubs\zed`，确保项目换机器后仍能构建。

增加 `rust-toolchain.toml`，锁定 Zed 当前使用的 Rust `1.97.1`。不引入 Zed `editor`、`project`、`workspace`、LSP 等重量级模块。聊天输入框基于 GPUI 输入协议单独实现。

## 应用架构

将现有集中式 `IcaApp` 拆成以下部分：

- `AppModel`：保存 Bridge、会话、消息、联系人和配置等领域状态，不引用 GPUI 类型。
- `RuntimeService`：持有 Tokio runtime，启动和停止 Socket.IO Bridge，将网络事件发送给 GPUI 前台任务。
- `AppShell`：管理主窗口、导航、面板、Modal 和辅助窗口。
- `ChatView`：管理当前会话、消息列表、输入区和聊天操作。
- `ChatInput`：实现轻量多行文本编辑器。
- `MediaCache`：将现有图片下载、磁盘缓存和解码逻辑接入 GPUI。
- `ToolPanel`：承载设置、状态和低频管理工具。

数据流统一为：

```text
Socket.IO/Tokio
    -> AppEvent
    -> GPUI 前台任务
    -> reducer/AppAction
    -> AppModel
    -> Entity notify
    -> UI 重绘
```

网络任务不能直接操作窗口，渲染函数也不能直接发起 Socket.IO 请求。UI 操作转换为 `AppAction`，再由统一副作用层调用现有 Bridge 命令。

## 界面结构

主窗口参考 Icalingua：

- 65px 左侧栏：当前账号、多 Bridge 切换、所有聊天、群聊、私聊、自定义分组和未读提示。
- 会话栏：默认 300px，可调整到 140–720px，显示搜索、头像、摘要、时间、静音、置顶、@ 和未读状态。
- 聊天区：64px 会话头部、不等高虚拟消息列表、回复/转发状态栏和底部输入区。
- 表情面板：默认位于右侧，默认 320px，可调整到 300–500px；窄窗口下显示为浮层。

不复制 Icalingua 的 Element UI 配色。背景、文本、边框、选中、悬浮、危险和强调色全部取自 Zed Theme。默认跟随系统明暗，深色使用 One Dark，浅色使用 One Light，并允许在设置中覆盖。

使用组合 `AssetSource` 同时加载 Zed 主题、图标、字体和 ica-native 自有图标、QQ 表情、图片资源、Noto Sans CJK 与 Unifont 字体。

## 聊天功能

### 消息列表

使用 GPUI 不等高 `List`：

- 从底部开始显示消息，位于底部时跟随新消息。
- 浏览旧消息时不强制跳到底部。
- 向前加载历史后保持当前视觉位置。
- 图片加载完成或窗口宽度改变时更新消息高度。
- 支持日期分隔、回复定位、未读跳转和 @ 跳转。

消息气泡参考 Icalingua：自己的消息靠右，其他消息靠左；显示头像、发送者、群身份、时间和撤回状态；最大宽度为聊天区的 85% 且不超过 800px；支持文本、链接、图片、文件、回复和合并转发预览，并保留现有右键操作。

### 聊天输入框

独立实现 `ChatInput`，支持：

- 中文 IME 组合输入。
- 光标、文本选择和鼠标定位。
- 复制、剪切、文本粘贴和撤销重做。
- 一至六行自适应高度。
- Enter 发送、Shift+Enter 换行，IME 提交期间不得误发送。
- 在当前选区插入表情或替换 `@` 触发符。
- 回复、重新编辑和草稿恢复。
- Ctrl+V 图片、系统文件拖放和待发送媒体预览。

### 日用功能

完整迁移多 Bridge、会话与分组、联系人、验证消息、群成员、历史加载、消息搜索、文本/图片/文件发送、回复、撤回、重新编辑、复制、+1、戳一戳、单条及多选/合并转发、@、QQ 表情、收藏表情和图片查看器。

## 图片系统

保留 HTTP 下载、磁盘缓存、容量控制、解码 worker、MIME 识别和过期 URL 处理。删除 egui loader、`TextureHandle` 和基于 egui repaint 的动画调度，改为 GPUI `ImageCache`、`RenderImage` 和动画图片刷新机制。

支持 PNG、JPEG、GIF、WebP、BMP、TIFF 和 SVG。

## 低频工具

群/成员管理、账号与登录设备、文件操作、消息检索、会话设置、自动签到、原始配置和 Socket.IO 调试继续保留，但统一使用简单的 Zed 风格表单与结果列表，不要求精细还原旧 UI。

QQ 关系网不迁移图形画布，只保留数据抓取、刷新进度、搜索、群筛选、关系分类、数量统计、节点详情和虚拟列表。删除力导向布局、节点/边绘制、缩放、平移和动画。

## 配置兼容

保留当前配置文件位置、Bridge 私钥、会话配置和原有字段。新增：

```rust
enum ThemeMode {
    System,
    Light,
    Dark,
}
```

以及：

```text
ui_setting.theme_mode          默认 system
ui_setting.room_panel_width    默认 300，限制 140–720
ui_setting.sticker_panel_width 默认 320，限制 300–500
```

`SelectedChatGroup` 增加 `Group`。旧的 `screen.vsync` 字段继续接受和保存；如果 GPUI 没有对应开关，则作为兼容字段忽略。迁移不得清空、覆盖或要求用户重建现有配置。

## 测试处理

不新增 GPUI UI 测试、快照测试、交互测试或性能测试。

删除 `src/app/**` 和 `src/image_loader/**` 中现有的 UI 测试模块，不为这些测试编写 GPUI 替代版本。保留 `src/ica/**` 和 `src/config/**` 中的非 UI 测试。

提交前仅运行：

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets
cargo test
cargo build --release
```

UI 只做 Windows 人工冒烟验收：启动、缩放、退出、Bridge 重连、聊天收发、中文输入、回复转发、图片粘贴、文件拖放、媒体查看、设置和低频工具。

## 实施顺序

1. 创建分支并提交本迁移文档。
2. 锁定 Rust 和 Zed 依赖，建立 GPUI 启动、主题与资产系统。
3. 将 Socket.IO runtime 与业务状态从 egui 上下文中分离。
4. 完成三栏 AppShell、Bridge 切换、分组和会话列表。
5. 完成消息虚拟列表、消息气泡和聊天操作。
6. 完成 ChatInput、中文 IME、剪贴板和文件拖放。
7. 迁移图片缓存、动画图片、表情和图片查看器。
8. 迁移联系人、验证消息、群成员、搜索和转发。
9. 用通用 ToolPanel 迁移低频工具和简化关系网。
10. 删除全部 UI 测试。
11. 删除 egui/eframe 代码及依赖。
12. 完成人工冒烟检查、构建检查和配置兼容检查。

## 完成标准

- `src/`、`Cargo.toml` 和 `Cargo.lock` 中不存在 egui、eframe、egui_extras。
- 构建不依赖本地 Zed clone。
- Windows release 构建成功。
- 原有 Socket.IO 和配置测试通过。
- 不存在新增的 GPUI UI 测试。
- 日用聊天功能可正常使用，所有低频工具仍有可操作入口。
- 关系网以列表与统计形式工作。
- 旧配置可以直接加载且不会丢失 Bridge 私钥或用户设置。
