use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};

use super::{CFG_ENV_VAR, DEFAULT_CFG_PATH, IcaCfg};

/// 加载配置文件时解析出的所有路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    config_file: PathBuf,
    cache_dir: PathBuf,
    data_dir: PathBuf,
}

impl ConfigPaths {
    pub fn config_file(&self) -> &Path {
        &self.config_file
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

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
    config: Arc<RwLock<IcaCfg>>,
    paths: Arc<ConfigPaths>,
}

impl ConfigStore {
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

    pub fn snapshot(&self) -> IcaCfg {
        self.config.read().expect("配置读锁被污染").clone()
    }

    pub fn update<R>(&self, updater: impl FnOnce(&mut IcaCfg) -> R) -> R {
        let mut config = self.config.write().expect("配置写锁被污染");
        updater(&mut config)
    }

    pub fn replace(&self, config: IcaCfg) -> Result<()> {
        config.validate_private_keys()?;
        self.update(|current| *current = config);
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        write_config(self.paths.config_file(), &self.snapshot())
    }

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

    pub fn paths(&self) -> &ConfigPaths {
        &self.paths
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigPathSource {
    CommandLine,
    Environment,
    Default,
}

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

    #[cfg(not(windows))]
    std::fs::rename(&temporary, path)
        .with_context(|| format!("提交配置文件失败: {}", path.display()))?;

    #[cfg(windows)]
    replace_config_on_windows(path, &temporary)?;
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_custom_chat_key_stays_compatible() {
        let parsed: IcaCfg = toml::from_str(
            r#"
                [custom_chat]
                hide_chat_img = true
                sort_stickers_by_time = false
            "#,
        )
        .unwrap();
        assert!(parsed.custom_chat.hide_chat_img);
        assert!(!parsed.custom_chat.sort_stickers_by_time);
        assert!(!parsed.custom_chat.high_contrast_chat);

        let encoded = toml::to_string(&parsed).unwrap();
        assert!(encoded.contains("[custom_chat]"));
        assert!(!encoded.contains("chat_appearance"));
    }

    #[test]
    fn high_contrast_chat_setting_round_trips() {
        let mut config = IcaCfg::default();
        config.custom_chat.high_contrast_chat = true;

        let encoded = toml::to_string(&config).unwrap();
        let decoded: IcaCfg = toml::from_str(&encoded).unwrap();

        assert!(decoded.custom_chat.high_contrast_chat);
    }

    #[test]
    fn server_owned_chat_groups_are_not_persisted_or_loaded() {
        let mut config = IcaCfg::default();
        config
            .chat_groups
            .groups
            .push(super::super::chat_groups::ChatGroup::new(
                "服务端分组",
                vec![123],
            ));

        let encoded = toml::to_string(&config).unwrap();
        assert!(!encoded.contains("chat_groups"));

        let decoded: IcaCfg = toml::from_str(
            r#"
                [[chat_groups.groups]]
                name = "旧本地分组"
                rooms = [123]
            "#,
        )
        .unwrap();
        assert!(decoded.chat_groups.groups.is_empty());
    }

    #[test]
    fn clones_share_updates() {
        let store = ConfigStore::from_config(IcaCfg::default(), "test.toml");
        let clone = store.clone();
        store.update(|config| config.screen.width = 777.0);
        assert_eq!(clone.snapshot().screen.width, 777.0);
    }

    #[test]
    fn command_line_config_path_has_priority() {
        let (path, source) = resolve_config_path(["--config", "custom.toml"]);
        assert_eq!(path, PathBuf::from("custom.toml"));
        assert_eq!(source, ConfigPathSource::CommandLine);
    }
}
