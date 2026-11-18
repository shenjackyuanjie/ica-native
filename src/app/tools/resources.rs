//! QQ 收藏和闪传的只读窗口。公共操作无需输入 JSON；资源正文按纯文本渲染，
//! 不执行收藏中的 HTML，也不请求外链、下载凭据或上传。

#[cfg(test)]
#[path = "tests/resources.rs"]
mod tests;

use crate::app::{AuthState, IcaApp, SocketState};
use crate::ica::IcaCommand;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::oneshot::{self, error::TryRecvError};

const PAGE_SIZE: usize = 30;

#[derive(Clone, Copy, Default, PartialEq)]
enum Tab {
    #[default]
    Favorites,
    Flash,
}

#[derive(Deserialize)]
struct Favorite {
    cid: String,
    status: Option<u32>,
    #[serde(rename = "type")]
    kind: Option<u32>,
    star_mark: Option<bool>,
    #[serde(default)]
    summary: Value,
}

impl Favorite {
    fn visible(&self) -> bool {
        !self.cid.is_empty() && self.status.is_none_or(|status| status == 1)
    }

    fn summary(&self) -> &str {
        [
            "/text_summary/brief",
            "/link_summary/title",
            "/link_summary/brief",
            "/gallery_summary/notes",
            "/audio_summary/note",
            "/video_summary/title",
            "/file_summary/file_info/name",
            "/location_summary/name",
            "/rich_media_summary/title",
            "/rich_media_summary/brief",
        ]
        .iter()
        .find_map(|path| {
            self.summary
                .pointer(path)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("（无摘要）")
    }

    fn kind_text(&self) -> &'static str {
        match self.kind {
            Some(1) => "文本",
            Some(2) => "链接",
            Some(3) => "图片",
            Some(4 | 9) => "语音",
            Some(5) => "视频",
            Some(6) => "文件",
            Some(7) => "位置",
            Some(8) => "富媒体",
            _ => "其他",
        }
    }
}

#[derive(Deserialize)]
struct FavoritePage {
    // 空收藏夹有时省略重复字段；按本体行为视作空页。
    #[serde(default)]
    collection_list: Vec<Favorite>,
    total_count: Option<usize>,
    result: Option<u32>,
}

#[derive(Deserialize)]
struct FavoriteDetail {
    collection: Favorite,
    #[serde(default)]
    content: Value,
}

#[derive(Deserialize, Clone)]
struct FlashEntry {
    name: String,
    cli_fileid: String,
    is_dir: bool,
    #[serde(default)]
    file_size: Value,
}

#[derive(Clone)]
enum Query {
    Favorites(usize),
    Detail(String),
    Resolve(String),
    Directory {
        id: String,
        folders: Vec<(String, String)>,
    },
}

impl Query {
    fn request(&self) -> (&'static str, Value) {
        match self {
            Self::Favorites(offset) => (
                "favoritesRequest",
                json!({"action":"list","options":{
                    "page_size": PAGE_SIZE, "start_pos": offset
                }}),
            ),
            Self::Detail(cid) => ("favoritesRequest", json!({"action":"get","cid":cid})),
            Self::Resolve(input) => (
                "flashTransferRequest",
                json!({"action":"resolve","input":input}),
            ),
            Self::Directory { id, folders } => (
                "flashTransferRequest",
                json!({
                    "action":"list", "fileset_id":id,
                    "parent_id":folders.last().map_or("", |(id, _)| id.as_str())
                }),
            ),
        }
    }
}

/// 临时查询状态只绑定当前 Bridge 和登录账号。替换或关闭窗口时丢弃回执接收端，
/// 因此即使换回同账号，先前的迟到回包也不能填到新窗口里。
#[derive(Default)]
pub struct ResourceBrowser {
    owner: Option<(String, i64)>,
    tab: Tab,
    pending: Option<(Query, oneshot::Receiver<Result<Value, String>>)>,
    error: Option<String>,
    favorites: Vec<Favorite>,
    next_offset: usize,
    has_more: bool,
    last_page_ids: Vec<String>,
    detail: Option<FavoriteDetail>,
    input: String,
    fileset: Option<String>,
    folders: Vec<(String, String)>,
    files: Vec<FlashEntry>,
}

impl ResourceBrowser {
    fn set_owner(&mut self, owner: Option<(String, i64)>) {
        if self.owner != owner {
            *self = Self {
                owner,
                tab: self.tab,
                ..Self::default()
            };
        }
    }

    fn accept(&mut self, query: Query, value: Value) -> Result<Option<Query>, &'static str> {
        match query {
            Query::Favorites(offset) => {
                let page = serde_json::from_value::<FavoritePage>(value)
                    .map_err(|_| "收藏列表格式无效")?;
                self.apply_favorite_page(offset, page);
            }
            Query::Detail(cid) => {
                let detail = serde_json::from_value::<FavoriteDetail>(value)
                    .map_err(|_| "收藏详情格式无效")?;
                if detail.collection.cid != cid || !detail.collection.visible() {
                    return Err("该收藏已不可用，请刷新列表");
                }
                self.detail = Some(detail);
            }
            Query::Resolve(_) => {
                let id = value
                    .as_str()
                    .filter(|id| !id.trim().is_empty() && id.len() <= 2048)
                    .ok_or("闪传文件集 ID 格式无效")?;
                return Ok(Some(Query::Directory {
                    id: id.to_owned(),
                    folders: Vec::new(),
                }));
            }
            Query::Directory { id, folders } => {
                let files = serde_json::from_value::<Vec<FlashEntry>>(value)
                    .map_err(|_| "闪传目录格式无效")?;
                self.fileset = Some(id);
                self.folders = folders;
                self.files = files;
            }
        }
        Ok(None)
    }

    fn apply_favorite_page(&mut self, offset: usize, page: FavoritePage) {
        if offset == 0 {
            self.favorites.clear();
            self.detail = None;
        }
        let len = page.collection_list.len();
        let ids: Vec<_> = page
            .collection_list
            .iter()
            .map(|item| item.cid.clone())
            .collect();
        // 游标按原始同步记录前进，删除标记也占位置。部分 Bridge 重复返回同页，
        // 发现未前进就停止，避免无穷加载或悄悄显示重复数据。
        if offset != 0 && ids == self.last_page_ids {
            self.has_more = false;
            return;
        }
        self.last_page_ids = ids;
        self.next_offset = offset.saturating_add(len);
        self.has_more = len > 0
            && (page.result == Some(215000)
                || page
                    .total_count
                    .map_or(len == PAGE_SIZE, |total| self.next_offset < total));
        for item in page.collection_list.into_iter().filter(Favorite::visible) {
            if let Some(existing) = self
                .favorites
                .iter_mut()
                .find(|entry| entry.cid == item.cid)
            {
                *existing = item;
            } else {
                self.favorites.push(item);
            }
        }
    }

    fn poll(&mut self, ctx: &egui::Context) -> Option<Query> {
        let (query, mut rx) = self.pending.take()?;
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Closed) => Err("Bridge 连接已断开，请重试".into()),
            Err(TryRecvError::Empty) => {
                self.pending = Some((query, rx));
                ctx.request_repaint_after(Duration::from_millis(100));
                return None;
            }
        };
        match result.and_then(|data| self.accept(query, data).map_err(str::to_owned)) {
            Ok(next) => next,
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }

    fn show_favorites(&mut self, ui: &mut egui::Ui, ready: bool) -> Option<Query> {
        let mut query = None;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(ready, egui::Button::new("读取／刷新收藏"))
                .clicked()
            {
                query = Some(Query::Favorites(0));
            }
            if self.has_more
                && ui
                    .add_enabled(ready, egui::Button::new("加载更多"))
                    .clicked()
            {
                query = Some(Query::Favorites(self.next_offset));
            }
            if self.next_offset > 0 {
                ui.weak(format!("已列出 {} 条", self.favorites.len()));
            }
        });
        if self.favorites.is_empty() && self.pending.is_none() {
            ui.weak("没有可显示的收藏；点击上方按钮获取列表。");
        }
        let selected = self
            .detail
            .as_ref()
            .map(|detail| detail.collection.cid.as_str());
        egui::ScrollArea::vertical()
            .id_salt("favorite_rows")
            .max_height(210.0)
            .show_rows(
                ui,
                ui.spacing().interact_size.y,
                self.favorites.len(),
                |ui, range| {
                    for item in &self.favorites[range] {
                        let star = if item.star_mark == Some(true) {
                            "★ "
                        } else {
                            ""
                        };
                        let label = format!("{star}[{}] {}", item.kind_text(), item.summary());
                        if ui
                            .add_enabled(
                                ready,
                                egui::Button::selectable(selected == Some(&item.cid), label)
                                    .truncate(),
                            )
                            .clicked()
                        {
                            query = Some(Query::Detail(item.cid.clone()));
                        }
                    }
                },
            );
        if let Some(detail) = &self.detail {
            ui.separator();
            ui.strong(format!(
                "{} · {}",
                detail.collection.kind_text(),
                detail.collection.summary()
            ));
            let text = detail
                .content
                .pointer("/text_content/data")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty());
            if let Some(text) = text {
                egui::ScrollArea::vertical()
                    .id_salt("favorite_text")
                    .max_height(160.0)
                    .show(ui, |ui| {
                        ui.label(text);
                    });
                if ui.button("复制正文").clicked() {
                    ui.ctx().copy_text(text.to_owned());
                }
            } else {
                ui.weak("此条目没有可显示的纯文本正文；当前不会打开外链或下载附件。");
            }
        }
        query
    }

    fn show_flash(&mut self, ui: &mut egui::Ui, ready: bool) -> Option<Query> {
        let mut query = None;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.input)
                    .char_limit(2048)
                    .desired_width((ui.available_width() - 90.0).max(100.0))
                    .hint_text("分享链接、分享码或文件集 ID"),
            );
            let input = self.input.trim();
            if ui
                .add_enabled(ready && !input.is_empty(), egui::Button::new("打开闪传"))
                .clicked()
            {
                self.fileset = None;
                self.files.clear();
                self.folders.clear();
                query = Some(Query::Resolve(input.to_owned()));
            }
        });
        let Some(id) = &self.fileset else {
            return query;
        };
        ui.horizontal_wrapped(|ui| {
            let folder = self
                .folders
                .last()
                .map_or("根目录", |(_, name)| name.as_str());
            ui.label(format!("{folder} · {} 项", self.files.len()));
            if ui
                .add_enabled(
                    ready && !self.folders.is_empty(),
                    egui::Button::new("上一级"),
                )
                .clicked()
            {
                query = Some(Query::Directory {
                    id: id.clone(),
                    folders: self.folders[..self.folders.len() - 1].to_vec(),
                });
            }
            if ui
                .add_enabled(ready, egui::Button::new("刷新目录"))
                .clicked()
            {
                query = Some(Query::Directory {
                    id: id.clone(),
                    folders: self.folders.clone(),
                });
            }
        });
        if self.files.is_empty() {
            ui.weak("当前目录为空。");
        }
        egui::ScrollArea::vertical()
            .id_salt("flash_rows")
            .max_height(300.0)
            .show_rows(
                ui,
                ui.spacing().interact_size.y,
                self.files.len(),
                |ui, range| {
                    for file in &self.files[range] {
                        if file.is_dir {
                            let valid = !file.cli_fileid.is_empty()
                                && !self.folders.iter().any(|(id, _)| id == &file.cli_fileid);
                            if ui
                                .add_enabled(
                                    ready && valid,
                                    egui::Button::new(format!("目录 › {}", file.name)).truncate(),
                                )
                                .clicked()
                            {
                                let mut folders = self.folders.clone();
                                folders.push((file.cli_fileid.clone(), file.name.clone()));
                                query = Some(Query::Directory {
                                    id: id.clone(),
                                    folders,
                                });
                            }
                        } else {
                            let bytes = file
                                .file_size
                                .as_u64()
                                .or_else(|| file.file_size.as_str()?.parse().ok());
                            let size =
                                bytes.map_or_else(String::new, |bytes| format!(" · {bytes} 字节"));
                            ui.add_sized(
                                [ui.available_width(), ui.spacing().interact_size.y],
                                egui::Label::new(format!("文件 · {}{size}", file.name)).truncate(),
                            );
                        }
                    }
                },
            );
        query
    }
}

/// 已知的旧版本没有注册资源事件，直接提示，避免等满 ACK 超时。
fn legacy_protocol(version: &str) -> bool {
    let mut parts = version.split('.').map(str::parse::<u64>);
    let (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch))) =
        (parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    (major, minor, patch) < (2, 26, 9)
}

impl IcaApp {
    /// 驱动只读资源窗口；未就绪的 Bridge 不发送请求，并清除前一次会话的内容。
    pub fn render_resource_browser(&mut self, ctx: &egui::Context) {
        if !self.open_page.resources {
            self.resource_browser = ResourceBrowser::default();
            return;
        }
        let connection = self
            .active_bridge_idx
            .and_then(|index| self.bridge_states.get(index))
            .ok_or("请先选择一个 Bridge")
            .and_then(|session| {
                if session.socket_state != SocketState::Connected
                    || session.auth_state != AuthState::Succeeded
                    || session.online_data.qqid <= 0
                {
                    return Err("等待 Bridge 认证和账号信息就绪");
                }
                if session
                    .bridge_protocol_version
                    .as_deref()
                    .is_some_and(legacy_protocol)
                {
                    return Err("收藏与闪传需要 oicq Bridge 协议 2.26.9 或更新版本");
                }
                let owner = (session.bridge_key.clone(), session.online_data.qqid);
                Ok((owner, session.command_sender()))
            });
        let view = &mut self.resource_browser;
        view.set_owner(connection.as_ref().ok().map(|(owner, _)| owner.clone()));
        let mut query = view.poll(ctx);
        let mut open = true;
        egui::Window::new("QQ 收藏与闪传")
            .open(&mut open)
            .default_width(530.0)
            .show(ctx, |ui| {
                if let Err(message) = &connection {
                    ui.label(*message);
                    return;
                }
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut view.tab, Tab::Favorites, "QQ 收藏");
                    ui.selectable_value(&mut view.tab, Tab::Flash, "QQ 闪传");
                    if view.pending.is_some() || query.is_some() {
                        ui.spinner();
                        ui.weak("正在查询…");
                    }
                });
                ui.separator();
                let ready = view.pending.is_none() && query.is_none();
                let clicked = match view.tab {
                    Tab::Favorites => view.show_favorites(ui, ready),
                    Tab::Flash => view.show_flash(ui, ready),
                };
                query = query.take().or(clicked);
                if let Some(error) = &view.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            });
        if !open {
            *view = ResourceBrowser::default();
        } else if let (Some(query), Ok((_, commands))) = (query, connection) {
            let (event, request) = query.request();
            let (tx, rx) = oneshot::channel();
            view.error = None;
            if commands
                .send(IcaCommand::QueryResource {
                    event,
                    request,
                    result_tx: tx,
                })
                .is_ok()
            {
                view.pending = Some((query, rx));
                ctx.request_repaint();
            } else {
                view.error = Some("Bridge 命令通道已关闭".into());
            }
        }
        self.open_page.resources = open;
    }
}
