//! 合并转发的解析、预览与查看窗口。
//!
//! 覆盖三条链路：把消息里的 `code`（JSON 或 XML）解析出资源 id/文件名与预览
//! 文本（`forward_reference`）；把选中的消息重建成合并转发节点
//! （`fake_forward_node`，无 raw chain 时按文本/表情/图片降级）；以及独立
//! viewport 的转发内容查看窗口（支持嵌套转发与图片查看）。

#[cfg(test)]
#[path = "tests/forward.rs"]
mod tests;

use crate::app::media::{ImageAction, ImageSource};
use crate::app::{IcaApp, state::ForwardViewerAction};
use crate::ica::{IcaCommand, types::message::Message};
use serde_json::{Value as JsonValue, json};

#[derive(Debug, Clone)]
/// 一条合并转发的引用信息：定位资源所需的 id、可选文件名与预览行。
pub struct ForwardReference {
    /// 合并转发的资源 id（Bridge 侧 `resid`）。
    pub res_id: String,
    /// 资源文件名；嵌套转发定位可能需要。
    pub file_name: Option<String>,
    /// 主 id 取不到时的备用资源 id。
    pub fallback_res_id: Option<String>,
    /// 消息行内直接携带的转发内容，命中时无需再向 Bridge 请求。
    pub inline_messages: Option<Vec<Message>>,
    /// 预览文本行（来源、标题等），最多展示前几条。
    pub preview: Vec<String>,
}

/// 从消息正文中提取 `[Marker: 值]` 形式标记里的值；空值或标记不闭合返回 `None`。
fn marker_value(content: &str, marker: &str) -> Option<String> {
    let start = content.find(marker)? + marker.len();
    let end = content[start..].find(']')? + start;
    let value = content[start..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// 把 `code` 字段规范为 JSON：字符串先尝试解析，其余原样返回；`null` 为 None。
fn code_as_json(code: &JsonValue) -> Option<JsonValue> {
    match code {
        JsonValue::Null => None,
        JsonValue::String(value) => serde_json::from_str(value).ok(),
        value => Some(value.clone()),
    }
}

/// 从 JSON 形态的 `code` 中取 `/meta/detail/resid`。
fn code_res_id(code: &JsonValue) -> Option<String> {
    code_as_json(code)?
        .pointer("/meta/detail/resid")?
        .as_str()
        .map(ToString::to_string)
}

/// 从 JSON 形态的 `code` 中取 `/meta/detail/uniseq` 作为文件名。
fn code_file_name(code: &JsonValue) -> Option<String> {
    code_as_json(code)?
        .pointer("/meta/detail/uniseq")
        .and_then(JsonValue::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
}

/// 从 XML 形态的 `code` 中找带 `m_resid`/`m_fileName` 属性的节点并取值。
///
/// 解析失败或找不到节点时返回 `(None, None)`。
fn xml_forward_resource(code: &JsonValue) -> (Option<String>, Option<String>) {
    let Some(xml) = code
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return (None, None);
    };
    let document = match roxmltree::Document::parse(xml) {
        Ok(document) => document,
        Err(_) => return (None, None),
    };
    let node = document
        .descendants()
        .find(|node| node.attribute("m_resid").is_some() || node.attribute("m_fileName").is_some());
    let Some(node) = node else {
        return (None, None);
    };
    (
        node.attribute("m_resid")
            .filter(|value| !value.trim().is_empty())
            .map(ToString::to_string),
        node.attribute("m_fileName")
            .filter(|value| !value.trim().is_empty())
            .map(ToString::to_string),
    )
}

/// 依次尝试 JSON 与 XML 两种形态提取转发资源 id 与文件名。
fn forward_resource(code: &JsonValue) -> (Option<String>, Option<String>) {
    let res_id = code_res_id(code);
    let file_name = code_file_name(code);
    if res_id.is_some() || file_name.is_some() {
        return (res_id, file_name);
    }
    xml_forward_resource(code)
}

/// 向预览行末尾追加一行；忽略空行并与末行去重。
fn push_preview_line(lines: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() && lines.last().is_none_or(|line| line != value) {
        lines.push(value.to_string());
    }
}

/// 从 JSON 形态提取预览行：`source` 与 `news[].text`。
fn json_forward_preview(code: &JsonValue) -> Vec<String> {
    let Some(value) = code_as_json(code) else {
        return Vec::new();
    };
    let Some(detail) = value.pointer("/meta/detail") else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    if let Some(source) = detail.get("source").and_then(JsonValue::as_str) {
        push_preview_line(&mut lines, source);
    }
    if let Some(news) = detail.get("news").and_then(JsonValue::as_array) {
        for item in news {
            if let Some(text) = item.get("text").and_then(JsonValue::as_str) {
                push_preview_line(&mut lines, text);
            }
        }
    }
    lines
}

/// 从 XML 形态提取预览行：收集所有 `<title>` 文本。
///
/// 直接解析失败时尝试包一层 `<item>` 再解析，兼容缺少根节点的片段。
fn xml_forward_preview(xml: &str) -> Vec<String> {
    /// 收集文档内所有 `<title>` 文本作为预览行。
    fn title_lines(document: &roxmltree::Document<'_>) -> Vec<String> {
        let mut lines = Vec::new();
        for title in document
            .descendants()
            .filter(|node| node.has_tag_name("title"))
        {
            if let Some(text) = title.text() {
                push_preview_line(&mut lines, text);
            }
        }
        lines
    }

    if let Ok(document) = roxmltree::Document::parse(xml) {
        return title_lines(&document);
    }
    let wrapped = format!("<item>{xml}</item>");
    roxmltree::Document::parse(&wrapped)
        .map(|document| title_lines(&document))
        .unwrap_or_default()
}

/// 汇总预览行：优先 JSON，取不到再按 XML 解析。
fn forward_preview(code: &JsonValue) -> Vec<String> {
    let json_preview = json_forward_preview(code);
    if !json_preview.is_empty() {
        return json_preview;
    }
    code.as_str().map(xml_forward_preview).unwrap_or_default()
}

/// 渲染转发预览：最多展示 4 行，超出部分以省略号收尾。
pub fn render_forward_preview(ui: &mut egui::Ui, reference: &ForwardReference) {
    const MAX_VISIBLE_LINES: usize = 4;

    for line in reference.preview.iter().take(MAX_VISIBLE_LINES) {
        ui.weak(line);
    }
    if reference.preview.len() > MAX_VISIBLE_LINES {
        ui.weak("...");
    }
}

/// 收集一批消息里的图片附件作为图片查看器的来源列表。
fn forward_image_sources(messages: &[Message]) -> Vec<ImageSource> {
    messages
        .iter()
        .flat_map(|message| message.files.iter())
        .filter(|file| super::is_image_file_type(&file.file_type) && !file.url.is_empty())
        .map(|file| ImageSource::url(file.url.clone()))
        .collect()
}

/// 尝试把 `code` 里的数组直接解析为内联消息列表；非数组或全部解析失败为 None。
fn inline_forward_messages(code: &JsonValue) -> Option<Vec<Message>> {
    let value = code_as_json(code)?;
    let JsonValue::Array(items) = value else {
        return None;
    };
    let messages = items
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect::<Vec<_>>();
    (!messages.is_empty()).then_some(messages)
}

/// 从一条消息解析出合并转发引用；没有转发标记时返回 `None`。
///
/// `[Forward: res_id]` 标记直接定位资源；`[NestedForward: 文件名]` 为嵌套
/// 转发，需要外层 `parent_res_id` 定位，并把 code 里的 res_id 作为备用。
pub fn forward_reference(
    message: &Message,
    parent_res_id: Option<&str>,
) -> Option<ForwardReference> {
    let (code_res_id, code_file_name) = forward_resource(&message.code);
    let inline_messages = inline_forward_messages(&message.code);
    if let Some(res_id) = marker_value(&message.content, "[Forward: ") {
        return Some(ForwardReference {
            res_id: code_res_id.unwrap_or(res_id),
            file_name: None,
            fallback_res_id: None,
            inline_messages,
            preview: forward_preview(&message.code),
        });
    }

    let marker_file_name = marker_value(&message.content, "[NestedForward: ")?;
    let res_id = parent_res_id
        .map(ToString::to_string)
        .or_else(|| code_res_id.clone())?;
    let fallback_res_id = parent_res_id
        .is_some()
        .then_some(code_res_id)
        .flatten()
        .filter(|fallback| fallback != &res_id);
    Some(ForwardReference {
        res_id,
        file_name: code_file_name.or(Some(marker_file_name)),
        fallback_res_id,
        inline_messages,
        preview: forward_preview(&message.code),
    })
}

/// 取消息的原始 raw chain（非空数组或带 `type` 的单对象），用于原样重建节点。
fn raw_message_elements(message: &Message) -> Option<Vec<JsonValue>> {
    match message.raw_msg.as_deref()? {
        JsonValue::Array(values) if !values.is_empty() => Some(values.clone()),
        JsonValue::Object(map) if map.contains_key("type") => {
            Some(vec![JsonValue::Object(map.clone())])
        }
        _ => None,
    }
}

/// 把正文（含 `[Face: id]` 表情标记）拆成 text/face 元素序列。
///
/// 先做 @ 提及格式化；表情 id 解析失败时按原文本保留，不丢内容。
fn push_text_and_faces(elements: &mut Vec<JsonValue>, content: &str) {
    let formatted = super::format_message_content(content);
    let mut remaining = formatted.as_ref();
    while let Some(start) = remaining.find("[Face: ") {
        if start > 0 {
            elements.push(json!({"type": "text", "data": {"text": &remaining[..start]}}));
        }
        let after = &remaining[start + 7..];
        let Some(end) = after.find(']') else {
            remaining = &remaining[start..];
            break;
        };
        if let Ok(id) = after[..end].parse::<u16>() {
            elements.push(json!({"type": "face", "data": {"id": id}}));
        } else {
            elements.push(json!({
                "type": "text",
                "data": {"text": &remaining[start..start + 8 + end]},
            }));
        }
        remaining = &after[end + 1..];
    }
    if !remaining.is_empty() {
        elements.push(json!({"type": "text", "data": {"text": remaining}}));
    }
}

/// 无 raw chain 时按文本/表情/引用/附件降级重建消息元素。
///
/// 图片用 URL、语音用 fid（并附文字提示可能无法合并转发）、其他附件只留
/// 名称提示；全部为空时补一条 `[空消息]`，保证节点非空。
fn fallback_message_elements(message: &Message) -> Vec<JsonValue> {
    let mut elements = Vec::new();
    if let Some(reply) = &message.reply {
        elements.push(json!({
            "type": "reply",
            "data": {"id": reply.msg_id, "text": reply.content},
        }));
    }
    push_text_and_faces(&mut elements, &message.content);

    for file in &message.files {
        let file_type = file.file_type.to_ascii_lowercase();
        if file_type == "image" || file_type.starts_with("image/") {
            if !file.url.trim().is_empty() {
                elements.push(json!({
                    "type": "image",
                    "data": {"file": file.url, "type": "image"},
                }));
            }
        } else if file_type.starts_with("audio/") {
            if let Some(fid) = file.fid.as_deref().filter(|value| !value.is_empty()) {
                elements.push(json!({"type": "record", "data": {"file": fid}}));
            }
            elements.push(json!({
                "type": "text",
                "data": {"text": "[语音] 语音可能无法合并转发"},
            }));
        } else {
            let name = file
                .name
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or("附件");
            elements.push(json!({
                "type": "text",
                "data": {"text": format!("[{name} 不支持合并转发]")},
            }));
        }
    }

    if elements.is_empty() {
        elements.push(json!({"type": "text", "data": {"text": "[空消息]"}}));
    }
    elements
}

/// 把一条消息重建成合并转发的发送节点（合并转发协议结构）。
///
/// 优先原样使用 raw chain，缺失时走降级重建。
pub fn fake_forward_node(message: &Message) -> JsonValue {
    json!({
        "user_id": message.sender_id,
        "message": raw_message_elements(message)
            .unwrap_or_else(|| fallback_message_elements(message)),
        "nickname": message.sender_name,
        "time": message.time.timestamp(),
        "id": message.msg_id,
        "consistent": true,
        "bubble_id": message.bubble_id,
    })
}

impl IcaApp {
    /// 打开一个合并转发引用：有内联消息直接展示，否则向 Bridge 请求。
    pub fn open_forward_reference(
        &mut self,
        res_id: String,
        file_name: Option<String>,
        fallback_res_id: Option<String>,
        inline_messages: Option<Vec<Message>>,
    ) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        if let Some(messages) = inline_messages {
            self.bridge_states[bridge_idx]
                .forward_viewer
                .lock()
                .unwrap()
                .open_inline(res_id, file_name, messages);
            return;
        }
        self.request_forward_messages(bridge_idx, res_id, file_name, fallback_res_id);
    }

    /// 向 Bridge 请求合并转发内容，并经 forward_viewer 记录请求状态。
    ///
    /// res_id 为空直接报错；命令发送失败时把同一 request 标记为失败，
    /// 迟到响应不会串到其他请求。
    fn request_forward_messages(
        &mut self,
        bridge_idx: usize,
        res_id: String,
        file_name: Option<String>,
        fallback_res_id: Option<String>,
    ) {
        if res_id.trim().is_empty() {
            self.bridge_states[bridge_idx]
                .forward_viewer
                .lock()
                .unwrap()
                .last_error = Some("Res ID 不能为空".to_string());
            return;
        }
        let request_id = self.bridge_states[bridge_idx]
            .forward_viewer
            .lock()
            .unwrap()
            .begin_request(res_id.clone(), file_name.clone(), fallback_res_id.clone());
        if let Err(error) = self.bridge_states[bridge_idx].send(IcaCommand::FetchForwardMessages {
            request_id,
            res_id,
            file_name,
            fallback_res_id,
        }) {
            tracing::warn!(
                target: "ica_native::forward",
                bridge = %self.bridge_states[bridge_idx].bridge_key,
                request_id,
                error = %error,
                "发送查看合并转发命令失败"
            );
            self.bridge_states[bridge_idx]
                .forward_viewer
                .lock()
                .unwrap()
                .fail(request_id, format!("查看合并转发命令发送失败: {error}"));
        }
    }

    /// 把当前选中的消息作为一条合并转发发到目标会话，返回是否提交成功。
    ///
    /// 目标为私聊时置 `direct_message`；群聊带上来源群号（取正）供 Bridge 标注。
    pub fn send_selected_messages_as_merged_forward(&mut self, target_room_id: i64) -> bool {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return false;
        };
        let Some(source_room_id) = self.bridge_states[bridge_idx].forward_room_id else {
            return false;
        };
        let messages = self.selected_forward_messages(bridge_idx, source_room_id);
        if messages.is_empty() {
            return false;
        }
        let nodes = messages.iter().map(fake_forward_node).collect::<Vec<_>>();
        let origin = (source_room_id < 0).then(|| source_room_id.abs());
        let command = IcaCommand::SendMergedForward {
            nodes,
            direct_message: target_room_id > 0,
            origin,
            target_room_id,
        };
        if let Err(error) = self.bridge_states[bridge_idx].send(command) {
            tracing::warn!(
                target: "ica_native::forward",
                bridge = %self.bridge_states[bridge_idx].bridge_key,
                target_room_id,
                error = %error,
                "发送合并转发命令失败"
            );
            self.bridge_states[bridge_idx].last_error =
                Some(format!("合并转发命令发送失败: {error}"));
            return false;
        }
        if self.scroll_to_bottom_after_send {
            self.bridge_states[bridge_idx]
                .conversation_mut(target_room_id)
                .pending_send_scroll_to_bottom = true;
        }
        true
    }

    /// 渲染"合并转发消息"独立查看窗口。
    ///
    /// 每帧先在主窗口消费查看器的待办动作（重载/图片/打开内层），再以
    /// deferred viewport 绘制内容；嵌套转发点击后转为新的 pending_action，
    /// 由主窗口接力打开，bridge 响应也经主窗口唤醒刷新。
    pub fn render_forward_viewer_window(&mut self, ctx: &egui::Context) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        let viewer_state = self.bridge_states[bridge_idx].forward_viewer.clone();
        let pending_action = viewer_state.lock().unwrap().pending_action.take();
        match pending_action {
            Some(ForwardViewerAction::Reload) => {
                let (res_id, file_name, fallback_res_id) = {
                    let viewer = viewer_state.lock().unwrap();
                    (
                        viewer.res_id.clone(),
                        viewer.file_name.trim().to_string(),
                        viewer.fallback_res_id.clone(),
                    )
                };
                self.request_forward_messages(
                    bridge_idx,
                    res_id,
                    (!file_name.is_empty()).then_some(file_name),
                    fallback_res_id,
                );
            }
            Some(ForwardViewerAction::Image { action, sources }) => match action {
                ImageAction::Open(source) => self.open_image_viewer_with_sources(source, sources),
                action => self.handle_image_action(ctx, bridge_idx, action),
            },
            Some(ForwardViewerAction::OpenReference {
                res_id,
                file_name,
                fallback_res_id,
                inline_messages,
            }) => {
                self.open_forward_reference(res_id, file_name, fallback_res_id, inline_messages);
            }
            None => {}
        }

        if !viewer_state.lock().unwrap().open {
            return;
        }

        let viewport_id = egui::ViewportId::from_hash_of((
            "forward_viewer",
            &self.bridge_states[bridge_idx].bridge_key,
        ));
        let parent_viewport_id = ctx.viewport_id();
        let viewport_builder = egui::ViewportBuilder::default()
            .with_title("合并转发消息")
            .with_inner_size([560.0, 680.0])
            .with_min_inner_size([360.0, 360.0]);
        let high_contrast = self.custom_chat.high_contrast_chat;
        let viewport_state = viewer_state.clone();
        ctx.show_viewport_deferred(
            viewport_id,
            viewport_builder,
            move |viewport_ctx, _class| {
                if viewport_ctx.input(|input| input.viewport().close_requested()) {
                    viewport_state.lock().unwrap().open = false;
                    viewport_ctx.request_repaint_of(parent_viewport_id);
                    return;
                }

                egui::CentralPanel::default().show(viewport_ctx, |ui| {
                    let mut nested_reference = None;
                    let mut image_action = None;
                    let mut viewer = viewport_state.lock().unwrap();
                    ui.horizontal(|ui| {
                        ui.label("资源 ID");
                        ui.add_sized(
                            [ui.available_width() - 72.0, 0.0],
                            egui::TextEdit::singleline(&mut viewer.res_id),
                        );
                        if ui
                            .add_enabled(!viewer.loading, egui::Button::new("加载"))
                            .clicked()
                        {
                            viewer.pending_action = Some(ForwardViewerAction::Reload);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("文件名");
                        ui.add_sized(
                            [ui.available_width(), 0.0],
                            egui::TextEdit::singleline(&mut viewer.file_name)
                                .hint_text("可选，嵌套转发需要"),
                        );
                    });
                    if viewer.loading {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("正在读取合并转发...");
                        });
                    }
                    if let Some(error) = &viewer.last_error {
                        ui.colored_label(egui::Color32::LIGHT_RED, error);
                    }
                    ui.separator();

                    if viewer.messages.is_empty() && !viewer.loading && viewer.last_error.is_none()
                    {
                        ui.weak("没有转发消息");
                    }
                    let parent_res_id = viewer.res_id.clone();
                    let image_sources = forward_image_sources(&viewer.messages);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for message in &viewer.messages {
                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.strong(&message.sender_name);
                                    ui.weak(format!(
                                        "{} · {}",
                                        message.sender_id, message.time_text
                                    ));
                                });
                                if let Some(reply) = &message.reply {
                                    egui::Frame::NONE
                                        .fill(ui.visuals().faint_bg_color)
                                        .inner_margin(6.0)
                                        .show(ui, |ui| {
                                            ui.weak(format!("回复 {}", reply.sender_name));
                                            super::message_card::render_rich_content(
                                                ui,
                                                &reply.content,
                                                high_contrast,
                                            );
                                        });
                                }
                                let has_visible_content =
                                    super::message_card::has_visible_rich_content(&message.content);
                                if has_visible_content {
                                    super::message_card::render_rich_content(
                                        ui,
                                        &message.content,
                                        high_contrast,
                                    );
                                }
                                for file in &message.files {
                                    if (file.file_type == "image"
                                        || file.file_type.starts_with("image/"))
                                        && !file.url.is_empty()
                                    {
                                        let source = ImageSource::url(file.url.clone());
                                        if let Some(action) =
                                            super::message_card::render_message_image(
                                                ui,
                                                &source,
                                                &file.file_type,
                                                ui.available_width().min(420.0),
                                                260.0,
                                            )
                                        {
                                            image_action = Some(action);
                                        }
                                    } else if !file.url.is_empty() {
                                        ui.hyperlink_to(
                                            file.name.as_deref().unwrap_or("打开附件"),
                                            &file.url,
                                        );
                                    }
                                }
                                if let Some(reference) =
                                    forward_reference(message, Some(&parent_res_id))
                                {
                                    if !has_visible_content {
                                        render_forward_preview(ui, &reference);
                                    }
                                    let mut response = ui.button("查看内层合并转发");
                                    if !reference.preview.is_empty() {
                                        response =
                                            response.on_hover_text(reference.preview.join("\n"));
                                    }
                                    if response.clicked() {
                                        nested_reference = Some(reference);
                                    }
                                }
                            });
                            ui.add_space(6.0);
                        }
                    });

                    if let Some(reference) = nested_reference {
                        viewer.pending_action = Some(ForwardViewerAction::OpenReference {
                            res_id: reference.res_id,
                            file_name: reference.file_name,
                            fallback_res_id: reference.fallback_res_id,
                            inline_messages: reference.inline_messages,
                        });
                    }
                    if let Some(action) = image_action {
                        viewer.pending_action = Some(ForwardViewerAction::Image {
                            action,
                            sources: image_sources,
                        });
                    }
                });

                if viewport_state.lock().unwrap().pending_action.is_some() {
                    viewport_ctx.request_repaint_of(parent_viewport_id);
                }
            },
        );

        // bridge 响应会先唤醒主窗口，再由主窗口通知独立查看器刷新内容。
        ctx.request_repaint_of(viewport_id);
    }
}
