//! 配置的加载、保存与路径解析。
//!
//! `ConfigStore` 持有一份可克隆共享的内存配置快照与解析出的路径，
//! 取代了旧版的进程级全局单例（见 `config/mod.rs` 末尾的历史注释）。
//! 写盘走「临时文件 + 原子替换」，避免半截配置损坏用户文件；
//! 路径优先级：命令行 `--config` > 环境变量 `ICA_NATIVE_CONFIG` > 默认 `ica_native.toml`。

#[cfg(test)]
#[path = "tests/store.rs"]
mod tests;

use super::{CFG_ENV_VAR, DEFAULT_CFG_PATH, IcaCfg};
use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// 加载配置文件时解析出的所有路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    /// 配置文件自身的路径（读取与写回都指向它）。
    config_file: PathBuf,
    /// 缓存目录（图片缓存等可再生成的产物）。
    cache_dir: PathBuf,
    /// 数据目录（需要长期保留的数据）。
    data_dir: PathBuf,
}

impl ConfigPaths {
    /// 配置文件路径。
    pub fn config_file(&self) -> &Path {
        &self.config_file
    }

    /// 缓存目录路径。
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// 数据目录路径。
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

/// 可注入、可克隆的配置存储。
///
/// 克隆实例共享同一份内存快照和已解析路径。磁盘访问只能通过
/// [`save`](Self::save) 和 [`reload`](Self::reload) 显式执行。
#[derive(Debug, Clone)]
pub struct ConfigStore {
    /// 内存中的配置；读写锁保护快照式访问，GUI 线程持锁时间极短。
    config: Arc<RwLock<IcaCfg>>,
    /// 已解析的各路径，构造时一次定下。
    paths: Arc<ConfigPaths>,
}

impl ConfigStore {
    /// 从进程参数解析路径并加载；命令行指定的路径必须已存在。
    pub fn load() -> Result<Self> {
        let (path, source) = resolve_config_path(std::env::args().skip(1));
        if source == ConfigPathSource::CommandLine {
            anyhow::ensure!(
                path.exists(),
                "命令行指定的配置文件不存在: {}",
                path.display()
            );
        }
        Self::load_from_path(path)
    }

    /// 从指定路径加载；文件不存在时写入一份默认配置（顺带建好父目录）。
    pub fn load_from_path(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let config = if path.exists() {
            anyhow::ensure!(path.is_file(), "配置路径不是文件: {}", path.display());
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("读取配置文件失败: {}", path.display()))?;
            let config: IcaCfg = toml::from_str(&content)
                .with_context(|| format!("解析 TOML 配置失败: {}", path.display()))?;
            config.validate_private_keys()?;
            config
        } else {
            let config = IcaCfg::default();
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("创建配置文件目录失败: {}", parent.display()))?;
            }
            write_config(&path, &config)?;
            config
        };

        Ok(Self::from_config(config, path))
    }

    /// 用给定配置与路径构造存储（供测试注入内存配置，跳过磁盘解析）。
    pub fn from_config(config: IcaCfg, config_file: impl Into<PathBuf>) -> Self {
        let config_file = config_file.into();
        let paths = ConfigPaths {
            config_file,
            cache_dir: config.get_cache_path(),
            data_dir: default_data_dir(),
        };
        Self {
            config: Arc::new(RwLock::new(config)),
            paths: Arc::new(paths),
        }
    }

    /// 取配置快照（完整克隆）；持读锁时间仅够克隆，不阻塞写入方。
    pub fn snapshot(&self) -> IcaCfg {
        self.config.read().expect("配置读锁被污染").clone()
    }

    /// 用闭包就地修改配置并返回其结果；写锁保证改动对其他线程原子可见。
    pub fn update<R>(&self, updater: impl FnOnce(&mut IcaCfg) -> R) -> R {
        let mut config = self.config.write().expect("配置写锁被污染");
        updater(&mut config)
    }

    /// 整体替换配置（先校验，失败则不改动内存中的旧值）。
    pub fn replace(&self, config: IcaCfg) -> Result<()> {
        config.validate_private_keys()?;
        self.update(|current| *current = config);
        Ok(())
    }

    /// 把当前配置写回磁盘（临时文件 + 原子替换，见 `write_config`）。
    pub fn save(&self) -> Result<()> {
        write_config(self.paths.config_file(), &self.snapshot())
    }

    /// 从磁盘重新读取并校验，成功后整体替换内存配置。
    pub fn reload(&self) -> Result<()> {
        let content = std::fs::read_to_string(self.paths.config_file())
            .with_context(|| format!("读取配置文件失败: {}", self.paths.config_file().display()))?;
        let config: IcaCfg = toml::from_str(&content).with_context(|| {
            format!("解析 TOML 配置失败: {}", self.paths.config_file().display())
        })?;
        config.validate_private_keys()?;
        self.update(|current| *current = config);
        Ok(())
    }

    /// 已解析的各路径（只读视图）。
    pub fn paths(&self) -> &ConfigPaths {
        &self.paths
    }
}

/// 配置路径的来源，用于区分「用户显式指定」与「默认值」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigPathSource {
    /// 命令行 `--config` 指定；路径必须存在，否则直接报错。
    CommandLine,
    /// 环境变量 `ICA_NATIVE_CONFIG` 指定。
    Environment,
    /// 两者都未提供时的默认路径。
    Default,
}

/// 按优先级解析配置路径：命令行 > 环境变量 > 默认路径。
///
/// 返回路径及其来源，供调用方决定「不存在时报错」还是「写入默认配置」。
fn resolve_config_path<I, S>(args: I) -> (PathBuf, ConfigPathSource)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg.as_ref() == "--config"
            && let Some(path) = args.next()
        {
            return (PathBuf::from(path.as_ref()), ConfigPathSource::CommandLine);
        }
    }

    if let Some(path) = std::env::var_os(CFG_ENV_VAR).filter(|path| !path.is_empty()) {
        (PathBuf::from(path), ConfigPathSource::Environment)
    } else {
        (PathBuf::from(DEFAULT_CFG_PATH), ConfigPathSource::Default)
    }
}

/// 原子写配置：先写同目录临时文件并 `sync_all`，再替换目标路径。
///
/// 临时文件名带进程 ID 与自增 nonce，避免并发写互相覆盖；
/// 任一步失败都会清理临时文件，不会留下半截配置。
fn write_config(path: &Path, config: &IcaCfg) -> Result<()> {
    static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

    let content = toml::to_string_pretty(config)?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
    }
    // 临时文件名必须唯一：同进程并发保存时靠 nonce 区分，多进程时靠 PID 区分。
    let nonce = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .with_context(|| format!("创建临时配置失败: {}", temporary.display()))?;
    if let Err(error) = file
        .write_all(content.as_bytes())
        .and_then(|()| file.sync_all())
    {
        let _ = std::fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("写入临时配置失败: {}", temporary.display()));
    }
    drop(file);

    // POSIX 上 rename 直接覆盖目标，本身就是原子的。
    #[cfg(not(windows))]
    std::fs::rename(&temporary, path)
        .with_context(|| format!("提交配置文件失败: {}", path.display()))?;

    #[cfg(windows)]
    replace_config_on_windows(path, &temporary)?;
    Ok(())
}

/// Windows 上的配置替换：`rename` 不能覆盖已存在的文件，需先备份再替换。
///
/// 替换失败时回滚备份，最大限度保住旧配置；备份清理失败只告警不失败。
#[cfg(windows)]
fn replace_config_on_windows(path: &Path, temporary: &Path) -> Result<()> {
    if !path.exists() {
        return std::fs::rename(temporary, path)
            .with_context(|| format!("提交配置文件失败: {}", path.display()));
    }

    let backup = path.with_extension(format!("bak-{}", std::process::id()));
    if backup.exists() {
        std::fs::remove_file(&backup)
            .with_context(|| format!("清理旧配置备份失败: {}", backup.display()))?;
    }
    std::fs::rename(path, &backup)
        .with_context(|| format!("备份旧配置失败: {}", path.display()))?;
    if let Err(error) = std::fs::rename(temporary, path) {
        let _ = std::fs::rename(&backup, path);
        let _ = std::fs::remove_file(temporary);
        return Err(error).with_context(|| format!("提交配置文件失败: {}", path.display()));
    }
    if let Err(error) = std::fs::remove_file(&backup) {
        tracing::warn!("清理配置备份 {} 失败: {error}", backup.display());
    }
    Ok(())
}

/// 平台默认数据目录：Windows 用 `%APPDATA%`，Linux 用 XDG 数据目录，
/// macOS 用 `Application Support`；都取不到时回退临时目录。
fn default_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(app_data) = std::env::var_os("APPDATA") {
            return PathBuf::from(app_data).join("ica-native");
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(data_home).join("ica-native");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".local/share/ica-native");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join("Library/Application Support/ica-native");
        }
    }
    std::env::temp_dir().join("ica-native")
}
