use std::{path::Path, sync::OnceLock};

use serde::{Deserialize, Serialize};

/// 全局配置
pub static CONFIG: OnceLock<IcaCfg> = OnceLock::new();

/// 配置的路径
pub static CONFIG_PATH: OnceLock<String> = OnceLock::new();

/// 配置文件
///
/// 考虑到允许你同时连接多个 bridge, 所以这玩意做的有点复杂
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct IcaCfg {
    /// bridge 列表
    #[serde(default)]
    pub bridges: Vec<IcaBridge>,
    /// 屏幕相关设置
    #[serde(default)]
    pub screen: Screen,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Screen {
    /// 宽
    #[serde(default)]
    pub width: f32,
    /// 高
    #[serde(default)]
    pub height: f32,
    /// 垂直同步
    #[serde(default)]
    pub vsync: bool,
    /// 初始化时是否窗口居中
    #[serde(default)]
    pub centered: bool,
}

impl Default for Screen {
    fn default() -> Self {
        Self {
            width: 1024.0,
            height: 768.0,
            vsync: true,
            centered: false,
        }
    }
}

/// 默认你写上去就是启用喽
fn ica_bridge_enable_default() -> bool {
    true
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
#[derive(Debug, Serialize, Deserialize)]
pub struct IcaBridge {
    /// socketio 服务器的 url
    pub url: String,
    /// socketio 的 private key (ed25519)
    pub private_key: String,
    /// 是否启用该 bridge
    #[serde(default = "ica_bridge_enable_default")]
    pub enable: bool,
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
pub fn init_cfg() -> &'static IcaCfg {
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
            CONFIG_PATH.get_or_init(|| p);
            return CONFIG.get_or_init(|| cfg);
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
                return CONFIG.get_or_init(|| default_cfg);
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
                    CONFIG_PATH.get_or_init(|| p);
                    return CONFIG.get_or_init(|| cfg);
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
        return CONFIG.get_or_init(|| default_cfg);
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
    CONFIG_PATH.get_or_init(|| DEFAULT_CFG_PATH.to_string());
    CONFIG.get_or_init(|| cfg)
}

pub fn write_back_cfg() -> anyhow::Result<()> {
    let cfg = CONFIG
        .get()
        .ok_or_else(|| anyhow::anyhow!("配置未初始化"))?;
    let content = toml::to_string_pretty(cfg)?;
    let path = Path::new(
        CONFIG_PATH
            .get()
            .ok_or_else(|| anyhow::anyhow!("配置路径未初始化"))?,
    );
    std::fs::write(path, content)?;
    Ok(())
}
