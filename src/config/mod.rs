use std::{collections::BTreeMap, fmt::Display, net::IpAddr, path::PathBuf};

use hex;
use serde::{Deserialize, Serialize};

use crate::noticer::{
    HARD_MAX_BODY_SIZE, HARD_MAX_IDEMPOTENCY_ENTRIES, HARD_MAX_IDEMPOTENCY_TTL_SECONDS,
    HARD_MAX_IMAGE_COUNT, HARD_MAX_IMAGE_SIZE, HARD_MAX_QUEUE_CAPACITY,
    HARD_MAX_QUEUED_IMAGE_BYTES, HARD_MAX_RETRY_ATTEMPTS, HARD_MAX_RETRY_DELAY_SECONDS,
    HARD_MAX_SEND_TIMEOUT_SECONDS, HARD_MAX_TOTAL_IMAGE_SIZE,
};

pub mod appearance;
pub mod chat_groups;
mod store;

pub use appearance::ChatAppearanceSettings;
pub use chat_groups::ChatGroups;
pub use store::{ConfigPaths, ConfigStore};

fn tokio_rt_work_thread_default() -> u32 {
    4
}

fn image_cache_max_bytes_default() -> u64 {
    128 * 1024 * 1024
}

fn disk_image_cache_max_bytes_default() -> u64 {
    1024 * 1024 * 1024
}

/// 配置文件
///
/// 考虑到允许你同时连接多个 bridge, 所以这玩意做的有点复杂
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

impl Display for IcaCfg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = toml::to_string_pretty(self).expect("配置序列化为 TOML 失败");
        f.write_str(&text)
    }
}

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
    pub fn key(&self) -> &str {
        if self.name.is_empty() {
            &self.url
        } else {
            &self.name
        }
    }
}

fn noticer_host_default() -> String {
    "127.0.0.1".to_string()
}

fn noticer_port_default() -> u16 {
    10020
}

fn noticer_queue_capacity_default() -> usize {
    256
}

fn noticer_send_timeout_default() -> f64 {
    60.0
}

fn noticer_retry_attempts_default() -> usize {
    3
}

fn noticer_retry_delay_default() -> f64 {
    1.0
}

fn noticer_max_body_size_default() -> usize {
    72 * 1024 * 1024
}

fn noticer_max_image_size_default() -> usize {
    8 * 1024 * 1024
}

fn noticer_max_image_count_default() -> usize {
    9
}

fn noticer_max_total_image_size_default() -> usize {
    48 * 1024 * 1024
}

fn noticer_max_queued_image_bytes_default() -> usize {
    128 * 1024 * 1024
}

fn noticer_idempotency_ttl_default() -> u64 {
    10 * 60
}

fn noticer_idempotency_entries_default() -> usize {
    2048
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct NoticerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "noticer_host_default")]
    pub host: String,
    #[serde(default = "noticer_port_default")]
    pub port: u16,
    #[serde(default)]
    pub auth_token: String,
    #[serde(default)]
    pub direct_token: String,
    #[serde(default)]
    pub default_bridge: String,
    #[serde(default = "noticer_queue_capacity_default")]
    pub queue_capacity: usize,
    #[serde(default = "noticer_send_timeout_default")]
    pub send_timeout_seconds: f64,
    #[serde(default = "noticer_retry_attempts_default")]
    pub retry_attempts: usize,
    #[serde(default = "noticer_retry_delay_default")]
    pub retry_delay_seconds: f64,
    #[serde(default = "noticer_max_body_size_default")]
    pub max_body_size_bytes: usize,
    #[serde(default = "noticer_max_image_size_default")]
    pub max_image_size_bytes: usize,
    #[serde(default = "noticer_max_image_count_default")]
    pub max_image_count: usize,
    #[serde(default = "noticer_max_total_image_size_default")]
    pub max_total_image_size_bytes: usize,
    #[serde(default = "noticer_max_queued_image_bytes_default")]
    pub max_queued_image_bytes: usize,
    #[serde(default = "noticer_idempotency_ttl_default")]
    pub idempotency_ttl_seconds: u64,
    #[serde(default = "noticer_idempotency_entries_default")]
    pub idempotency_max_entries: usize,
    #[serde(default)]
    pub rooms: BTreeMap<String, NoticerRoom>,
}

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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct NoticerRoom {
    pub bridge: String,
    pub room_id: i64,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Screen {
    /// 宽
    #[serde(default = "screen_width_default")]
    pub width: f32,
    /// 高
    #[serde(default = "screen_height_default")]
    pub height: f32,
    /// 垂直同步
    #[serde(default = "screen_vsync_default")]
    pub vsync: bool,
    /// 渲染后端。
    #[serde(default)]
    pub renderer: RendererBackend,
    /// WGPU 使用的图形 API；仅在 renderer = "wgpu" 时生效。
    #[serde(default)]
    pub wgpu_backend: WgpuBackend,
    /// 初始化时是否窗口居中
    #[serde(default = "screen_centered_default")]
    pub centered: bool,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RendererBackend {
    /// 低内存的 OpenGL 后端；部分 Windows/NVIDIA 组合在 VSync 下可能偶发闪屏。
    #[default]
    Glow,
    /// WGPU 后端；使用 wgpu_backend 指定图形 API，避免回退到 OpenGL。
    Wgpu,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WgpuBackend {
    Dx12,
    Vulkan,
    Metal,
}

impl Default for WgpuBackend {
    fn default() -> Self {
        #[cfg(target_os = "windows")]
        return Self::Dx12;
        #[cfg(target_os = "linux")]
        return Self::Vulkan;
        #[cfg(target_os = "macos")]
        return Self::Metal;
        #[allow(unreachable_code)]
        Self::Vulkan
    }
}

fn screen_width_default() -> f32 {
    1024.0
}
fn screen_height_default() -> f32 {
    768.0
}
fn screen_vsync_default() -> bool {
    true
}
fn screen_centered_default() -> bool {
    false
}

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

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReEditDraftConflictMode {
    Overwrite,
    Append,
    SkipIfNonEmpty,
}

fn reedit_draft_conflict_mode_default() -> ReEditDraftConflictMode {
    ReEditDraftConflictMode::Overwrite
}

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

fn clear_search_on_room_select_default() -> bool {
    true
}

fn auto_fetch_history_on_room_select_default() -> bool {
    // 与 Icalingua++ 的默认行为保持一致：首次打开只读取 bridge 缓存，
    // 只有用户明确开启该选项后，切换会话才会额外请求协议端漫游历史。
    false
}

fn scroll_to_bottom_after_send_default() -> bool {
    true
}

fn relation_network_max_visible_nodes_default() -> usize {
    2_500
}

fn relation_network_max_visible_nodes_focused_default() -> usize {
    6_000
}

fn relation_network_max_drawn_links_default() -> usize {
    2_500
}

fn relation_network_max_drawn_links_focused_default() -> usize {
    6_000
}

fn relation_network_max_labels_default() -> usize {
    350
}

fn relation_network_auto_hide_labels_node_threshold_default() -> usize {
    2_000
}

fn relation_network_auto_hide_acquaintance_node_threshold_default() -> usize {
    10_000
}

fn relation_network_auto_hide_stranger_node_threshold_default() -> usize {
    50_000
}

fn relation_network_force_repulsion_strength_default() -> f32 {
    0.32
}

fn relation_network_force_friend_link_length_default() -> f32 {
    1.05
}

fn relation_network_force_group_link_length_default() -> f32 {
    2.40
}

fn relation_network_force_group_member_link_length_default() -> f32 {
    0.28
}

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

/// 默认配置文件路径
pub const DEFAULT_CFG_PATH: &str = "ica_native.toml";

/// 环境变量名称
pub const CFG_ENV_VAR: &str = "ICA_NATIVE_CONFIG";

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge(name: &str) -> IcaBridge {
        IcaBridge {
            name: name.to_string(),
            url: "ws://127.0.0.1:9999".to_string(),
            private_key: String::new(),
            enable: true,
            allow_protocol_mismatch: false,
        }
    }

    #[test]
    fn noticer_non_loopback_listener_requires_authentication() {
        let mut noticer = NoticerConfig {
            enabled: true,
            host: "0.0.0.0".to_string(),
            default_bridge: "main".to_string(),
            ..NoticerConfig::default()
        };
        let bridges = [bridge("main")];
        let error = noticer.validate(&bridges).unwrap_err();
        assert!(error.to_string().contains("必须配置 auth_token"));

        noticer.auth_token = "configured".to_string();
        noticer.validate(&bridges).unwrap();
    }

    #[test]
    fn noticer_routes_must_reference_an_enabled_bridge() {
        let mut noticer = NoticerConfig {
            enabled: true,
            default_bridge: "main".to_string(),
            ..NoticerConfig::default()
        };
        noticer.rooms.insert(
            "warning".to_string(),
            NoticerRoom {
                bridge: "secondary".to_string(),
                room_id: -456,
                description: String::new(),
            },
        );
        let error = noticer.validate(&[bridge("main")]).unwrap_err();
        assert!(error.to_string().contains("未启用的 bridge"));

        noticer
            .validate(&[bridge("main"), bridge("secondary")])
            .unwrap();
    }
}

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
