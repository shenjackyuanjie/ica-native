//! 设置相关的编辑器与外观选项 UI。
//!
//! 本目录承载“设置”功能的界面部分：`config_editor` 是直接编辑 TOML 的
//! 高级编辑器，`noticer` 是内建 Noticer 的常用选项编辑器，`appearance`
//! 则在 `ChatAppearanceSettings` 上扩展两组外观选项（沿用 ica 的与
//! ica-native 特有的）。编辑器只改内存草稿，持久化由调用方经
//! `ConfigStore` 决定，因此这里不直接写盘。

mod appearance;
mod config_editor;
mod noticer;

pub use config_editor::ConfigEditor;
pub use noticer::NoticerEditor;
