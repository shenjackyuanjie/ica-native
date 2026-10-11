//! 应用配置：持久化到 `ica_native.toml` 的 `IcaCfg` 及各分段设置。
//!
//! 本模块只定义「配置长什么样、默认值是什么、怎么校验」；文件的读取、写入与
//! 路径解析在 [`store`] 子模块（`ConfigStore`），界面编辑位于 `app::settings`。
//! 因为允许同时连接多个 bridge，顶层用 `bridges` 列表，其余按功能分段。

pub mod appearance;
pub mod chat_groups;
mod store;
#[cfg(test)]
mod tests;

use crate::noticer::{
    HARD_MAX_BODY_SIZE, HARD_MAX_IDEMPOTENCY_ENTRIES, HARD_MAX_IDEMPOTENCY_TTL_SECONDS,
    HARD_MAX_IMAGE_COUNT, HARD_MAX_IMAGE_SIZE, HARD_MAX_QUEUE_CAPACITY,
    HARD_MAX_QUEUED_IMAGE_BYTES, HARD_MAX_RETRY_ATTEMPTS, HARD_MAX_RETRY_DELAY_SECONDS,
    HARD_MAX_SEND_TIMEOUT_SECONDS, HARD_MAX_TOTAL_IMAGE_SIZE,
};
pub use appearance::ChatAppearanceSettings;
pub use chat_groups::ChatGroups;
use hex;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt::Display, net::IpAddr, path::PathBuf};
pub use store::{ConfigPaths, ConfigStore};

/// tokio 运行时工作线程数默认值。
fn tokio_rt_work_thread_default() -> u32 {
    4
}

/// 图片内存缓存默认上限（128MB）。
fn image_cache_max_bytes_default() -> u64 {
    128 * 1024 * 1024
}

/// 图片磁盘缓存默认上限（1GB）。
fn disk_image_cache_max_bytes_default() -> u64 {
    1024 * 1024 * 1024
}

/// 配置文件
///
/// 考虑到允许你同时连接多个 bridge, 所以这玩意做的有点复杂
///
/// `chat_groups` 是唯一不落盘的字段：分组由 bridge 下发并同步回 bridge，
/// 仅作为运行时状态存在，因此标了 `#[serde(skip)]`。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IcaCfg {
    /// bridge 列表
    #[serde(default)]
    pub bridges: Vec<IcaBridge>,
    /// 屏幕相关设置
    #[serde(default)]
    pub screen: Screen,
    /// 界面设置相关
    #[serde(default)]
    pub ui_setting: UiSetting,
    /// 聊天分组
    ///
    /// 仅作为运行时状态存在：分组由 bridge 下发并同步回 bridge，不属于本地配置。
    #[serde(skip)]
    pub chat_groups: ChatGroups,
    /// 定制聊天界面选项
    #[serde(default)]
    pub custom_chat: ChatAppearanceSettings,
    /// 收藏表情目录。未设置时优先共享 Icalingua++ 的 stickers 目录。
    #[serde(default)]
    pub sticker_path: Option<String>,
    /// 缓存路径（可选）。如果未设置，程序会使用默认缓存位置（例如临时目录或内置路径）。
    #[serde(default)]
    pub cache_path: Option<String>,
    /// 图片缓存最大内存（字节）
    #[serde(default = "image_cache_max_bytes_default")]
    pub image_cache_max_bytes: u64,
    /// 图片磁盘缓存最大字节数（默认 1GB）
    #[serde(default = "disk_image_cache_max_bytes_default")]
    pub disk_image_cache_max_bytes: u64,
    /// 异步 runtime 工作线程数
    /// tokio 运行线程数
    #[serde(default = "tokio_rt_work_thread_default")]
    pub tokio_rt_work_thread: u32,
    /// 内建 Noticer HTTP webhook 服务。
    #[serde(default)]
    pub noticer: NoticerConfig,
    /// 本机 Agent 聊天上下文读取服务，与 Noticer 的发送权限隔离。
    #[serde(default)]
    pub agent_context: crate::agent_context::AgentContextConfig,
}

/// `Display` 实现：把配置序列化为 TOML 文本，供日志与调试输出复用。
impl Display for IcaCfg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = toml::to_string_pretty(self).expect("配置序列化为 TOML 失败");
        f.write_str(&text)
    }
}

/// `Default` 实现：全部字段取各自的 `*_default` 函数，bridge 列表为空。
impl Default for IcaCfg {
    fn default() -> Self {
        Self {
            bridges: Vec::new(),
            screen: Screen::default(),
            ui_setting: UiSetting::default(),
            chat_groups: ChatGroups::default(),
            custom_chat: ChatAppearanceSettings::default(),
            sticker_path: None,
            cache_path: None,
            image_cache_max_bytes: image_cache_max_bytes_default(),
            disk_image_cache_max_bytes: disk_image_cache_max_bytes_default(),
            tokio_rt_work_thread: tokio_rt_work_thread_default(),
            noticer: NoticerConfig::default(),
            agent_context: crate::agent_context::AgentContextConfig::default(),
        }
    }
}

/// 默认你写上去就是启用喽
fn ica_bridge_enable_default() -> bool {
    true
}

impl IcaCfg {
    /// 校验全部启用中 bridge 的私钥，并联动校验 Noticer 与 Agent 上下文配置。
    ///
    /// 私钥必须是 32 字节的十六进制字符串；启用 Agent 上下文时还要求其 Token
    /// 与 Noticer 的发送 / 直发 Token 均不重复（读取权限与发送权限隔离）。
    pub fn validate_private_keys(&self) -> anyhow::Result<()> {
        for (idx, bridge) in self.bridges.iter().enumerate() {
            if !bridge.enable {
                continue;
            }
            if bridge.private_key.trim().is_empty() {
                anyhow::bail!(
                    "bridge private_key 为空: index={} name={} url={}",
                    idx,
                    bridge.name,
                    bridge.url
                );
            }
            let bytes = match hex::decode(&bridge.private_key) {
                Ok(b) => b,
                Err(e) => {
                    anyhow::bail!(
                        "bridge private_key 解析失败: index={} name={} url={} err={}",
                        idx,
                        bridge.name,
                        bridge.url,
                        e
                    );
                }
            };
            if bytes.len() != 32 {
                anyhow::bail!(
                    "bridge private_key 长度不是32字节: index={} name={} url={} len={}",
                    idx,
                    bridge.name,
                    bridge.url,
                    bytes.len()
                );
            }
        }
        self.noticer.validate(&self.bridges)?;
        self.agent_context.validate()?;
        if self.agent_context.enabled {
            anyhow::ensure!(
                self.agent_context.auth_token != self.noticer.auth_token
                    && self.agent_context.auth_token != self.noticer.direct_token,
                "Agent 读取 Token 不能与 Noticer 发送 Token 复用"
            );
        }
        Ok(())
    }

    /// 获取缓存路径
    pub fn get_cache_path(&self) -> PathBuf {
        // 如果配置中指定了路径，优先使用
        if let Some(ref path) = self.cache_path {
            return PathBuf::from(path);
        }

        // 根据平台选择默认缓存路径
        #[cfg(windows)]
        {
            std::env::temp_dir().join("ica_native")
        }

        #[cfg(target_os = "linux")]
        {
            // Linux: 检查 XDG_CACHE_HOME，否则使用 ~/.cache
            if let Ok(cache_home) = std::env::var("XDG_CACHE_HOME") {
                PathBuf::from(cache_home).join("ica_native")
            } else if let Ok(home) = std::env::var("HOME") {
                PathBuf::from(home).join(".cache").join("ica_native")
            } else {
                PathBuf::from("/tmp").join("ica_native")
            }
        }

        #[cfg(target_os = "macos")]
        {
            // macOS: 使用 ~/Library/Caches
            if let Ok(home) = std::env::var("HOME") {
                PathBuf::from(home)
                    .join("Library")
                    .join("Caches")
                    .join("ica_native")
            } else {
                PathBuf::from("/tmp").join("ica_native")
            }
        }

        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            // 其他未知平台，使用当前目录
            PathBuf::from("./ica_native")
        }
    }

    /// 获取图片缓存路径
    ///
    /// 返回图片缓存应该使用的目录路径
    ///
    /// 优先级：
    /// 1. 配置中的 `image_cache_path`（如果有）
    /// 2. 平台特定的默认缓存目录
    ///    - Windows: 用户临时目录
    ///    - Linux/macOS: 系统默认缓存目录
    /// 3. 回退到 `./ica_native_image_cache`
    pub fn get_image_cache_path(&self) -> PathBuf {
        self.get_cache_path().join("image_cache")
    }
}

/// 具体 bridge 的配置
///
/// ## 登录功能
///
/// 理论上应该可以支持你去使用 ica native 让 bridge 登录
///
/// 但是考虑到会需要解析一些网页之类的, 还是请使用 icalingua 本体进行登录
///
/// 因此其实这玩意挺简洁的就是了
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IcaBridge {
    /// bridge 名称（用于区分多 bridge）
    pub name: String,
    /// Socket.IO 服务器 URL
    pub url: String,
    /// Socket.IO 的 Ed25519 私钥
    pub private_key: String,
    /// 是否启用该 bridge
    #[serde(default = "ica_bridge_enable_default")]
    pub enable: bool,
    /// 允许连接协议版本与客户端预期不一致的 Bridge。
    #[serde(default)]
    pub allow_protocol_mismatch: bool,
}

impl IcaBridge {
    /// bridge 标识：优先取名称，名称为空时回退为 URL（保证非空且稳定）。
    ///
    /// Noticer 房间、Bridge 注册表等都以该标识为键。
    pub fn key(&self) -> &str {
        if self.name.is_empty() {
            &self.url
        } else {
            &self.name
        }
    }
}

/// Noticer 默认监听地址：仅回环，避免未配令牌时暴露到局域网。
fn noticer_host_default() -> String {
    "127.0.0.1".to_string()
}

/// Noticer 默认监听端口。
fn noticer_port_default() -> u16 {
    10020
}

/// 待发送队列容量默认值（按任务个数计）。
fn noticer_queue_capacity_default() -> usize {
    256
}

/// 单次发送超时默认值（秒）。
fn noticer_send_timeout_default() -> f64 {
    60.0
}

/// 发送重试次数默认值。
fn noticer_retry_attempts_default() -> usize {
    3
}

/// 重试间隔默认值（秒）。
fn noticer_retry_delay_default() -> f64 {
    1.0
}

/// 请求体大小默认上限（72MB）。
fn noticer_max_body_size_default() -> usize {
    72 * 1024 * 1024
}

/// 单张图片大小默认上限（8MB）。
fn noticer_max_image_size_default() -> usize {
    8 * 1024 * 1024
}

/// 单次请求图片数量默认上限（对齐 QQ 常用多图上限）。
fn noticer_max_image_count_default() -> usize {
    9
}

/// 单次请求图片总大小默认上限（48MB）。
fn noticer_max_total_image_size_default() -> usize {
    48 * 1024 * 1024
}

/// 队列中待发送图片字节总量默认上限（128MB）。
fn noticer_max_queued_image_bytes_default() -> usize {
    128 * 1024 * 1024
}

/// 幂等记录存活时间默认值（秒，10 分钟）。
fn noticer_idempotency_ttl_default() -> u64 {
    10 * 60
}

/// 幂等记录条数默认上限。
fn noticer_idempotency_entries_default() -> usize {
    2048
}

/// 内建 Noticer HTTP webhook 服务的持久化配置。
///
/// 各项上限必须同时满足 `validate` 中的区间与 `noticer` 模块的硬上限；
/// 房间表把「名字」映射到具体 bridge + 房间，供 `/v1/send` 按名路由。
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct NoticerConfig {
    /// 是否启用内建 HTTP 服务。
    #[serde(default)]
    pub enabled: bool,
    /// 监听地址；非回环地址必须配置 `auth_token`（见 `validate`）。
    #[serde(default = "noticer_host_default")]
    pub host: String,
    /// 监听端口。
    #[serde(default = "noticer_port_default")]
    pub port: u16,
    /// `/send` 与 `/v1/send` 的 Bearer 令牌；为空表示关闭鉴权（仅限回环）。
    #[serde(default)]
    pub auth_token: String,
    /// `/v1/send/direct` 的独立 Bearer 令牌；为空表示关闭直发端点。
    #[serde(default)]
    pub direct_token: String,
    /// 直发端点与未指定 bridge 时使用的默认 bridge 标识。
    #[serde(default)]
    pub default_bridge: String,
    /// 待发送队列容量（任务个数）。
    #[serde(default = "noticer_queue_capacity_default")]
    pub queue_capacity: usize,
    /// 单次发送超时（秒）；超时后结果未知，返回 504。
    #[serde(default = "noticer_send_timeout_default")]
    pub send_timeout_seconds: f64,
    /// 发送失败时的最大重试次数。
    #[serde(default = "noticer_retry_attempts_default")]
    pub retry_attempts: usize,
    /// 重试间隔（秒）。
    #[serde(default = "noticer_retry_delay_default")]
    pub retry_delay_seconds: f64,
    /// 请求体大小上限（字节）。
    #[serde(default = "noticer_max_body_size_default")]
    pub max_body_size_bytes: usize,
    /// 单张图片大小上限（字节）。
    #[serde(default = "noticer_max_image_size_default")]
    pub max_image_size_bytes: usize,
    /// 单次请求图片数量上限。
    #[serde(default = "noticer_max_image_count_default")]
    pub max_image_count: usize,
    /// 单次请求图片总大小上限（字节）。
    #[serde(default = "noticer_max_total_image_size_default")]
    pub max_total_image_size_bytes: usize,
    /// 队列中待发送图片字节总量上限（字节）。
    #[serde(default = "noticer_max_queued_image_bytes_default")]
    pub max_queued_image_bytes: usize,
    /// 幂等记录存活时间（秒）。
    #[serde(default = "noticer_idempotency_ttl_default")]
    pub idempotency_ttl_seconds: u64,
    /// 幂等记录条数上限。
    #[serde(default = "noticer_idempotency_entries_default")]
    pub idempotency_max_entries: usize,
    /// 房间名 → 房间定义；名字即 `/v1/send` 的 `room` 字段取值。
    #[serde(default)]
    pub rooms: BTreeMap<String, NoticerRoom>,
}

/// `Default` 实现：默认关闭，监听回环 10020，无令牌，限额取上述默认值。
impl Default for NoticerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: noticer_host_default(),
            port: noticer_port_default(),
            auth_token: String::new(),
            direct_token: String::new(),
            default_bridge: String::new(),
            queue_capacity: noticer_queue_capacity_default(),
            send_timeout_seconds: noticer_send_timeout_default(),
            retry_attempts: noticer_retry_attempts_default(),
            retry_delay_seconds: noticer_retry_delay_default(),
            max_body_size_bytes: noticer_max_body_size_default(),
            max_image_size_bytes: noticer_max_image_size_default(),
            max_image_count: noticer_max_image_count_default(),
            max_total_image_size_bytes: noticer_max_total_image_size_default(),
            max_queued_image_bytes: noticer_max_queued_image_bytes_default(),
            idempotency_ttl_seconds: noticer_idempotency_ttl_default(),
            idempotency_max_entries: noticer_idempotency_entries_default(),
            rooms: BTreeMap::new(),
        }
    }
}

impl NoticerConfig {
    /// 校验配置合法性；未启用时直接放行。
    ///
    /// 检查分四组：各项限额落在 `1..=硬上限` 且层层不小于下级上限
    /// （单图 ≤ 多图总量 ≤ 队列总量）、监听非回环地址必须配令牌、
    /// 启用中的 bridge 标识不得重复、房间必须引用已启用 bridge 且 `room_id` 非零、
    /// `default_bridge` 必填且已启用。
    fn validate(&self, bridges: &[IcaBridge]) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        anyhow::ensure!(!self.host.trim().is_empty(), "noticer host 不能为空");
        anyhow::ensure!(
            self.queue_capacity > 0 && self.queue_capacity <= HARD_MAX_QUEUE_CAPACITY,
            "noticer queue_capacity 必须在 1..={HARD_MAX_QUEUE_CAPACITY}"
        );
        anyhow::ensure!(
            self.send_timeout_seconds.is_finite()
                && (1.0..=HARD_MAX_SEND_TIMEOUT_SECONDS).contains(&self.send_timeout_seconds),
            "noticer send_timeout_seconds 必须在 1..={HARD_MAX_SEND_TIMEOUT_SECONDS}"
        );
        anyhow::ensure!(
            self.retry_attempts > 0 && self.retry_attempts <= HARD_MAX_RETRY_ATTEMPTS,
            "noticer retry_attempts 必须在 1..={HARD_MAX_RETRY_ATTEMPTS}"
        );
        anyhow::ensure!(
            self.retry_delay_seconds.is_finite()
                && (0.0..=HARD_MAX_RETRY_DELAY_SECONDS).contains(&self.retry_delay_seconds),
            "noticer retry_delay_seconds 必须在 0..={HARD_MAX_RETRY_DELAY_SECONDS}"
        );
        anyhow::ensure!(
            (1..=HARD_MAX_BODY_SIZE).contains(&self.max_body_size_bytes),
            "noticer max_body_size_bytes 必须在 1..={HARD_MAX_BODY_SIZE}"
        );
        anyhow::ensure!(
            (1..=HARD_MAX_IMAGE_SIZE).contains(&self.max_image_size_bytes),
            "noticer max_image_size_bytes 必须在 1..={HARD_MAX_IMAGE_SIZE}"
        );
        anyhow::ensure!(
            (1..=HARD_MAX_IMAGE_COUNT).contains(&self.max_image_count),
            "noticer max_image_count 必须在 1..={HARD_MAX_IMAGE_COUNT}"
        );
        anyhow::ensure!(
            self.max_total_image_size_bytes >= self.max_image_size_bytes
                && self.max_total_image_size_bytes <= HARD_MAX_TOTAL_IMAGE_SIZE,
            "noticer max_total_image_size_bytes 必须不小于单图上限且不超过 {HARD_MAX_TOTAL_IMAGE_SIZE}"
        );
        anyhow::ensure!(
            self.max_queued_image_bytes >= self.max_total_image_size_bytes
                && self.max_queued_image_bytes <= HARD_MAX_QUEUED_IMAGE_BYTES,
            "noticer max_queued_image_bytes 必须不小于多图总上限且不超过 {HARD_MAX_QUEUED_IMAGE_BYTES}"
        );
        anyhow::ensure!(
            (1..=HARD_MAX_IDEMPOTENCY_TTL_SECONDS).contains(&self.idempotency_ttl_seconds),
            "noticer idempotency_ttl_seconds 必须在 1..={HARD_MAX_IDEMPOTENCY_TTL_SECONDS}"
        );
        anyhow::ensure!(
            (1..=HARD_MAX_IDEMPOTENCY_ENTRIES).contains(&self.idempotency_max_entries),
            "noticer idempotency_max_entries 必须在 1..={HARD_MAX_IDEMPOTENCY_ENTRIES}"
        );

        // localhost 与环回 IP（127.0.0.1 / ::1）视为本机访问，允许不配令牌。
        let host = self.host.trim();
        let loopback = host.eq_ignore_ascii_case("localhost")
            || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
        anyhow::ensure!(
            loopback || !self.auth_token.trim().is_empty(),
            "noticer 监听非回环地址时必须配置 auth_token"
        );

        let mut bridge_keys = std::collections::HashSet::new();
        for bridge in bridges.iter().filter(|bridge| bridge.enable) {
            anyhow::ensure!(
                bridge_keys.insert(bridge.key().to_string()),
                "启用的 bridge 标识重复: {}",
                bridge.key()
            );
        }
        for (name, room) in &self.rooms {
            anyhow::ensure!(!name.trim().is_empty(), "noticer 房间名不能为空");
            anyhow::ensure!(
                bridge_keys.contains(room.bridge.trim()),
                "noticer 房间 {name} 引用了未启用的 bridge: {}",
                room.bridge
            );
            anyhow::ensure!(room.room_id != 0, "noticer 房间 {name} 的 room_id 不能为 0");
        }
        anyhow::ensure!(
            !self.default_bridge.trim().is_empty(),
            "启用 noticer 时必须配置 default_bridge"
        );
        anyhow::ensure!(
            bridge_keys.contains(self.default_bridge.trim()),
            "noticer default_bridge 未启用: {}",
            self.default_bridge
        );
        Ok(())
    }
}

/// Noticer 中的一个房间：投递到哪个 bridge 的哪个会话。
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct NoticerRoom {
    /// 承担发送的 bridge 标识，必须对应一个启用中的 bridge。
    pub bridge: String,
    /// 房间（群 / 私聊）ID；0 视为未配置。
    pub room_id: i64,
    /// 可选说明，仅用于 `/status` 展示。
    #[serde(default)]
    pub description: String,
}

/// 窗口与渲染相关的屏幕设置。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Screen {
    /// 窗口初始宽度（逻辑像素）。
    #[serde(default = "screen_width_default")]
    pub width: f32,
    /// 窗口初始高度（逻辑像素）。
    #[serde(default = "screen_height_default")]
    pub height: f32,
    /// 是否启用垂直同步；关闭可降低延迟但可能提高功耗。
    #[serde(default = "screen_vsync_default")]
    pub vsync: bool,
    /// 渲染后端。
    #[serde(default)]
    pub renderer: RendererBackend,
    /// WGPU 使用的图形 API；仅在 renderer = "wgpu" 时生效。
    #[serde(default)]
    pub wgpu_backend: WgpuBackend,
    /// 初始化时是否窗口居中。
    #[serde(default = "screen_centered_default")]
    pub centered: bool,
}

/// 渲染后端选择。
#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RendererBackend {
    /// 低内存的 OpenGL 后端；部分 Windows/NVIDIA 组合在 VSync 下可能偶发闪屏。
    #[default]
    Glow,
    /// WGPU 后端；使用 wgpu_backend 指定图形 API，避免回退到 OpenGL。
    Wgpu,
}

/// WGPU 图形 API 选择；默认值按平台不同，Windows 上选 DX12 以规避 OpenGL 闪屏。
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WgpuBackend {
    /// Direct3D 12（Windows 默认）。
    Dx12,
    /// Vulkan（Linux 默认）。
    Vulkan,
    /// Metal（macOS 默认）。
    Metal,
}

/// `Default` 实现：按编译目标平台选择，未覆盖的平台回退 Vulkan。
impl Default for WgpuBackend {
    fn default() -> Self {
        #[cfg(target_os = "windows")]
        return Self::Dx12;
        #[cfg(target_os = "linux")]
        return Self::Vulkan;
        #[cfg(target_os = "macos")]
        return Self::Metal;
        // 三个平台 cfg 必命中其一，此行仅为满足编译器的返回值检查。
        #[allow(unreachable_code)]
        Self::Vulkan
    }
}

/// 窗口初始宽度默认值。
fn screen_width_default() -> f32 {
    1024.0
}
/// 窗口初始高度默认值。
fn screen_height_default() -> f32 {
    768.0
}
/// 默认开启垂直同步。
fn screen_vsync_default() -> bool {
    true
}
/// 默认不居中（沿用平台行为）。
fn screen_centered_default() -> bool {
    false
}

/// `Default` 实现：1024×768、开 VSync、不居中，后端取各自默认值。
impl Default for Screen {
    fn default() -> Self {
        Self {
            width: screen_width_default(),
            height: screen_height_default(),
            vsync: screen_vsync_default(),
            renderer: RendererBackend::default(),
            wgpu_backend: WgpuBackend::default(),
            centered: screen_centered_default(),
        }
    }
}

/// 已撤回消息重新编辑时，遇到输入框已有草稿的处理策略。
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReEditDraftConflictMode {
    /// 直接用撤回内容覆盖现有草稿（默认）。
    Overwrite,
    /// 把撤回内容追加到现有草稿之后。
    Append,
    /// 已有草稿时跳过恢复，保留用户正在写的内容。
    SkipIfNonEmpty,
}

/// 默认策略：覆盖。
fn reedit_draft_conflict_mode_default() -> ReEditDraftConflictMode {
    ReEditDraftConflictMode::Overwrite
}

/// 界面交互设置：会话切换、发送行为与关系网渲染。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UiSetting {
    /// 选择会话后是否自动清空聊天列表搜索框
    #[serde(default = "clear_search_on_room_select_default")]
    pub clear_search_on_room_select: bool,
    /// 切换到已经加载过的会话时，是否再次拉取最新的历史消息
    #[serde(default = "auto_fetch_history_on_room_select_default")]
    pub auto_fetch_history_on_room_select: bool,
    /// 发送消息后是否自动滚动到底部
    #[serde(default = "scroll_to_bottom_after_send_default")]
    pub scroll_to_bottom_after_send: bool,
    /// 已撤回消息重新编辑时，遇到已有草稿如何处理
    #[serde(default = "reedit_draft_conflict_mode_default")]
    pub reedit_draft_conflict_mode: ReEditDraftConflictMode,
    /// QQ 关系网渲染设置
    #[serde(default)]
    pub relation_network: RelationNetworkSetting,
}

/// 默认开启：选中会话后清空搜索框，避免下一个会话被旧关键词过滤。
fn clear_search_on_room_select_default() -> bool {
    true
}

/// 默认关闭，理由见函数体内的兼容性说明。
fn auto_fetch_history_on_room_select_default() -> bool {
    // 与 Icalingua++ 的默认行为保持一致：首次打开只读取 bridge 缓存，
    // 只有用户明确开启该选项后，切换会话才会额外请求协议端漫游历史。
    false
}

/// 默认开启：发送后回到底部，符合即时聊天的预期。
fn scroll_to_bottom_after_send_default() -> bool {
    true
}

/// 普通视图最多参与渲染的节点数；超出部分在布局阶段被裁掉，保证帧率。
fn relation_network_max_visible_nodes_default() -> usize {
    2_500
}

/// 聚焦节点后一跳视图的节点上限（比普通视图宽，用于围绕焦点展开）。
fn relation_network_max_visible_nodes_focused_default() -> usize {
    6_000
}

/// 普通视图最多绘制的连线数。
fn relation_network_max_drawn_links_default() -> usize {
    2_500
}

/// 聚焦视图最多绘制的连线数。
fn relation_network_max_drawn_links_focused_default() -> usize {
    6_000
}

/// 最多绘制标签的节点数，防止密集图里文字互相重叠。
fn relation_network_max_labels_default() -> usize {
    350
}

/// 节点数超过该值时自动关闭标签（先于 max_labels 生效的整体开关）。
fn relation_network_auto_hide_labels_node_threshold_default() -> usize {
    2_000
}

/// 节点数超过该值时自动隐藏「共同群好友」这类中间关系，突出核心结构。
fn relation_network_auto_hide_acquaintance_node_threshold_default() -> usize {
    10_000
}

/// 节点数超过该值时自动隐藏「仅同群」的弱关系节点。
fn relation_network_auto_hide_stranger_node_threshold_default() -> usize {
    50_000
}

/// 力导向布局的近距离斥力强度；越大密集节点越容易被推开。
fn relation_network_force_repulsion_strength_default() -> f32 {
    0.32
}

/// “自己”到好友节点的基础弹簧长度。
fn relation_network_force_friend_link_length_default() -> f32 {
    1.05
}

/// “自己”到群节点的基础弹簧长度（比好友更长，形成外圈）。
fn relation_network_force_group_link_length_default() -> f32 {
    2.40
}

/// 群节点到普通成员节点的弹簧目标长度（较短，使成员聚在群周围）。
fn relation_network_force_group_member_link_length_default() -> f32 {
    0.28
}

/// QQ 关系网的渲染与布局设置；数值同时影响帧率与可读性，按机器性能调节。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RelationNetworkSetting {
    /// 普通视图最多参与渲染的节点数
    #[serde(default = "relation_network_max_visible_nodes_default")]
    pub max_visible_nodes: usize,
    /// 聚焦节点后一跳视图最多参与渲染的节点数
    #[serde(default = "relation_network_max_visible_nodes_focused_default")]
    pub max_visible_nodes_focused: usize,
    /// 普通视图最多绘制的连线数
    #[serde(default = "relation_network_max_drawn_links_default")]
    pub max_drawn_links: usize,
    /// 聚焦节点后一跳视图最多绘制的连线数
    #[serde(default = "relation_network_max_drawn_links_focused_default")]
    pub max_drawn_links_focused: usize,
    /// 节点数超过该值时不绘制标签
    #[serde(default = "relation_network_max_labels_default")]
    pub max_labels: usize,
    /// 图节点数超过该值时自动关闭标签
    #[serde(default = "relation_network_auto_hide_labels_node_threshold_default")]
    pub auto_hide_labels_node_threshold: usize,
    /// 图节点数超过该值时自动隐藏共同群好友
    #[serde(default = "relation_network_auto_hide_acquaintance_node_threshold_default")]
    pub auto_hide_acquaintance_node_threshold: usize,
    /// 图节点数超过该值时自动隐藏仅同群节点
    #[serde(default = "relation_network_auto_hide_stranger_node_threshold_default")]
    pub auto_hide_stranger_node_threshold: usize,
    /// 力导向近距离斥力强度；数值越大，密集节点之间的间距越明显
    #[serde(default = "relation_network_force_repulsion_strength_default")]
    pub force_repulsion_strength: f32,
    /// “自己”到好友节点的基础弹簧长度；实际长度会按节点稳定散开，默认形成外圈宽带
    #[serde(default = "relation_network_force_friend_link_length_default")]
    pub force_friend_link_length: f32,
    /// “自己”到群节点的基础弹簧长度；实际长度会按节点稳定散开，密集群默认使用更宽范围
    #[serde(default = "relation_network_force_group_link_length_default")]
    pub force_group_link_length: f32,
    /// 群节点到普通成员节点的弹簧目标长度
    #[serde(default = "relation_network_force_group_member_link_length_default")]
    pub force_group_member_link_length: f32,
}

/// `Default` 实现：全部取上述默认值函数。
impl Default for RelationNetworkSetting {
    fn default() -> Self {
        Self {
            max_visible_nodes: relation_network_max_visible_nodes_default(),
            max_visible_nodes_focused: relation_network_max_visible_nodes_focused_default(),
            max_drawn_links: relation_network_max_drawn_links_default(),
            max_drawn_links_focused: relation_network_max_drawn_links_focused_default(),
            max_labels: relation_network_max_labels_default(),
            auto_hide_labels_node_threshold:
                relation_network_auto_hide_labels_node_threshold_default(),
            auto_hide_acquaintance_node_threshold:
                relation_network_auto_hide_acquaintance_node_threshold_default(),
            auto_hide_stranger_node_threshold:
                relation_network_auto_hide_stranger_node_threshold_default(),
            force_repulsion_strength: relation_network_force_repulsion_strength_default(),
            force_friend_link_length: relation_network_force_friend_link_length_default(),
            force_group_link_length: relation_network_force_group_link_length_default(),
            force_group_member_link_length: relation_network_force_group_member_link_length_default(
            ),
        }
    }
}

/// `Default` 实现：清空搜索、发送后滚动、默认覆盖草稿，关系网取其默认值。
impl Default for UiSetting {
    fn default() -> Self {
        Self {
            clear_search_on_room_select: clear_search_on_room_select_default(),
            auto_fetch_history_on_room_select: auto_fetch_history_on_room_select_default(),
            scroll_to_bottom_after_send: scroll_to_bottom_after_send_default(),
            reedit_draft_conflict_mode: reedit_draft_conflict_mode_default(),
            relation_network: RelationNetworkSetting::default(),
        }
    }
}

/// 默认配置文件路径（相对于进程工作目录）。
pub const DEFAULT_CFG_PATH: &str = "ica_native.toml";

/// 指定配置文件路径的环境变量名；优先级低于命令行 `--config`。
pub const CFG_ENV_VAR: &str = "ICA_NATIVE_CONFIG";

// 旧版的 CLI、环境变量和默认路径解析曾放在这里，现在统一由 ConfigStore 管理。
/* The former global configuration API intentionally remains unavailable.
pub fn init_cfg() {
    // 处理 cli 参数
    {
        let args = std::env::args().collect::<Vec<_>>();
        let mut path = None;
        for i in 0..args.len() {
            if args[i] == "--config" && i + 1 < args.len() {
                path = Some(args[i + 1].clone());
                break;
            }
        }
        if let Some(p) = path {
            let path = Path::new(&p);
            // 检查是否存在 & 是否是文件
            if !path.exists() {
                panic!("命令行指定的配置文件不存在, 给一个存在的行不行");
            } else if !path.is_file() {
                panic!("命令行指定的配置文件不是文件, 给一个文件行不行");
            } else if path
                .metadata()
                .map(|m| m.permissions().readonly())
                .unwrap_or(false)
            {
                // 只读文件警告
                eprintln!(
                    "警告: 命令行指定的配置文件({})是一个只读文件, 请确保你知道自己在干什么",
                    path.display()
                );
            }
            // 读取
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("配置文件读取失败 {e}"))
                .unwrap();
            let cfg: IcaCfg = toml::from_str(&content)
                .map_err(|e| format!("配置文件解析为 toml 失败 {e}"))
                .unwrap();
            cfg.validate_private_keys();
            CONFIG_PATH.get_or_init(|| p);
            CONFIG.get_or_init(|| RwLock::new(cfg));
            return;
        }
    }
    // 尝试一下环境变量
    {
        if let Some(p) = std::env::var(CFG_ENV_VAR)
            .ok()
            .filter(|p| !p.trim().is_empty())
        {
            let path = Path::new(&p);
            // 检查是否存在 & 是否是文件
            if !path.exists() {
                eprintln!("警告: 环境变量({CFG_ENV_VAR})指定的配置文件不存在, 将写入默认配置");
                let default_cfg = IcaCfg::default();
                let content = toml::to_string_pretty(&default_cfg).unwrap();
                std::fs::write(path, content)
                    .map_err(|e| format!("默认配置文件写入失败 {e} {path:?}"))
                    .unwrap();
                CONFIG_PATH.get_or_init(|| p);
                CONFIG.get_or_init(|| RwLock::new(default_cfg));
                return;
            } else if !path.is_file() {
                eprintln!("警告: 环境变量({CFG_ENV_VAR})指定的配置文件不是文件, 给一个文件行不行");
            } else if path
                .metadata()
                .map(|m| m.permissions().readonly())
                .unwrap_or(false)
            {
                // 只读文件警告
                eprintln!(
                    "警告: 环境变量({CFG_ENV_VAR})指定的配置文件({})是一个只读文件, 请确保你知道自己在干什么",
                    path.display()
                );
                // 读取
                let content = std::fs::read_to_string(path)
                    .inspect_err(|e| eprintln!("配置文件读取失败 {e}"));

                if let Ok(content) = content {
                    let cfg: IcaCfg = toml::from_str(&content)
                        .map_err(|e| format!("配置文件解析为 toml 失败 {e} {path:?}"))
                        .unwrap();
                    cfg.validate_private_keys();
                    CONFIG_PATH.get_or_init(|| p);
                    CONFIG.get_or_init(|| RwLock::new(cfg));
                    return;
                }
            }
        }
    }
    // 默认路径
    let path = Path::new(DEFAULT_CFG_PATH);
    if !path.exists() {
        eprintln!("警告: 默认配置文件不存在, 将写入默认配置");
        let default_cfg = IcaCfg::default();
        let content = toml::to_string_pretty(&default_cfg).unwrap();
        std::fs::write(path, content)
            .map_err(|e| format!("默认配置文件写入失败 {e} {path:?}"))
            .unwrap();
        CONFIG_PATH.get_or_init(|| DEFAULT_CFG_PATH.to_string());
        CONFIG.get_or_init(|| RwLock::new(default_cfg));
        return;
    } else if !path.is_file() {
        panic!("默认配置文件路径 {} 不是文件", path.display());
    } else if path
        .metadata()
        .map(|m| m.permissions().readonly())
        .unwrap_or(false)
    {
        // 只读文件警告
        eprintln!(
            "警告: 默认配置文件{}是一个只读文件, 请确保你知道自己在干什么",
            path.display()
        );
    }

    // 读取
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("配置文件读取失败 {e}"))
        .unwrap();
    let cfg: IcaCfg = toml::from_str(&content)
        .map_err(|e| format!("配置文件解析为 toml 失败 {e}"))
        .unwrap();
    cfg.validate_private_keys();
    CONFIG_PATH.get_or_init(|| DEFAULT_CFG_PATH.to_string());
    CONFIG.get_or_init(|| RwLock::new(cfg));
}

/// 关闭时 写入 cfg
pub fn write_back_cfg() -> anyhow::Result<()> {
    let cfg = CONFIG
        .get()
        .ok_or_else(|| anyhow::anyhow!("配置未初始化"))?
        .read()
        .map_err(|_| anyhow::anyhow!("配置读锁被污染"))?;
    let content = toml::to_string_pretty(&*cfg)?;
    let path = Path::new(
        CONFIG_PATH
            .get()
            .ok_or_else(|| anyhow::anyhow!("配置路径未初始化"))?,
    );
    std::fs::write(path, content)?;
    Ok(())
}

/// 使用闭包更新配置
///
/// # Example
/// ```
/// update_cfg(|cfg| {
///     cfg.ui_setting.some_field = new_value;
/// });
/// ```
pub fn update_cfg<F>(updater: F)
where
    F: FnOnce(&mut IcaCfg),
{
    let config_lock = CONFIG.get().expect("配置未初始化");

    let mut cfg = config_lock.write().expect("配置写锁被污染");

    updater(&mut cfg);
}

/// 更新并保存配置
///
/// 这个函数会在更新配置后立即将其写入文件
pub fn update_and_save_cfg<F>(updater: F)
where
    F: FnOnce(&mut IcaCfg),
{
    update_cfg(updater);
    write_back_cfg().expect("配置写入失败");
}

/// 重新加载配置文件
///
/// 从磁盘重新读取配置文件并更新内存中的配置
pub fn reload_cfg() -> anyhow::Result<()> {
    let path = CONFIG_PATH.get().expect("配置路径未初始化");

    let content = std::fs::read_to_string(path)?;
    let new_cfg: IcaCfg = toml::from_str(&content)?;
    new_cfg.validate_private_keys();

    let config_lock = CONFIG.get().expect("配置未初始化");

    let mut cfg = config_lock.write().expect("配置写锁被污染");

    *cfg = new_cfg;

    Ok(())
}

/// 获取当前配置的快照
///
/// 这个函数会返回当前配置的一个完整克隆，适用于需要获取配置快照
/// 或者需要在不持有锁的情况下处理配置的场景
///
/// # Example
/// ```
/// let cfg_snapshot = get_cfg_snapshot();
/// // 现在可以随意使用 cfg_snapshot，不会阻塞其他线程
/// ```
pub fn get_cfg_snapshot() -> IcaCfg {
    let config_lock = CONFIG.get().expect("配置未初始化");

    let cfg = config_lock.read().expect("配置读锁被污染");

    cfg.clone()
}

/// 获取图片缓存路径
///
/// 这是一个便捷函数，直接从全局配置中获取图片缓存路径
///
/// # Example
/// ```
/// let cache_path = get_image_cache_path();
/// println!("图片缓存路径: {:?}", cache_path);
/// ```
pub fn get_image_cache_path() -> PathBuf {
    let config_lock = CONFIG.get().expect("配置未初始化");

    let cfg = config_lock.read().expect("配置读锁被污染");

    cfg.get_image_cache_path()
}
*/
