use std::{
    fmt::Display,
    path::Path,
    sync::{OnceLock, RwLock},
};

use serde::{Deserialize, Serialize};
use hex;

/// 全局配置
pub static CONFIG: OnceLock<RwLock<IcaCfg>> = OnceLock::new();

/// 配置的路径
pub static CONFIG_PATH: OnceLock<String> = OnceLock::new();

fn tokio_rt_work_thread_default() -> u32 {
    4
}

fn image_cache_max_bytes_default() -> u64 {
    256 * 1024 * 1024
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
    /// 图片缓存最大内存（字节）
    #[serde(default = "image_cache_max_bytes_default")]
    pub image_cache_max_bytes: u64,
    /// async runtime workthread count
    /// tokio 运行线程数
    #[serde(default = "tokio_rt_work_thread_default")]
    pub tokio_rt_work_thread: u32,
}

impl Display for IcaCfg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = toml::to_string_pretty(self).expect("faild to fmt self");
        f.write_str(&text)
    }
}

impl Default for IcaCfg {
    fn default() -> Self {
        Self {
            bridges: Vec::new(),
            screen: Screen::default(),
            ui_setting: UiSetting::default(),
            image_cache_max_bytes: image_cache_max_bytes_default(),
            tokio_rt_work_thread: tokio_rt_work_thread_default(),
        }
    }
}

/// 默认你写上去就是启用喽
fn ica_bridge_enable_default() -> bool {
    true
}

impl IcaCfg {
    fn validate_private_keys(&self) {
        for (idx, bridge) in self.bridges.iter().enumerate() {
            if !bridge.enable {
                continue;
            }
            if bridge.private_key.trim().is_empty() {
                panic!(
                    "bridge private_key 为空: index={} name={} url={}",
                    idx, bridge.name, bridge.url
                );
            }
            let bytes = match hex::decode(&bridge.private_key) {
                Ok(b) => b,
                Err(e) => {
                    panic!(
                        "bridge private_key 解析失败: index={} name={} url={} err={}",
                        idx, bridge.name, bridge.url, e
                    );
                }
            };
            if bytes.len() != 32 {
                panic!(
                    "bridge private_key 长度不是32字节: index={} name={} url={} len={}",
                    idx, bridge.name, bridge.url, bytes.len()
                );
            }
        }
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
    /// socketio 服务器的 url
    pub url: String,
    /// socketio 的 private key (ed25519)
    pub private_key: String,
    /// 是否启用该 bridge
    #[serde(default = "ica_bridge_enable_default")]
    pub enable: bool,
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
    /// 初始化时是否窗口居中
    #[serde(default = "screen_centered_default")]
    pub centered: bool,
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
            centered: screen_centered_default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct UiSetting {
    // todo
}

/// 默认配置文件路径
pub const DEFAULT_CFG_PATH: &str = "ica_native.toml";

/// 环境变量名称
pub const CFG_ENV_VAR: &str = "ICA_NATIVE_CONFIG";

/// 支持让你通过 --config 指定配置文件
///
/// 但也就到这了, 默认是 ica-native.toml
///
/// 然后如果环境变量有的话取个 ICA_NATIVE_CONFIG 也是可以的
///
/// 优先级: cli - env - default
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
