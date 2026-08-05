use serde_json::{Value as JsonValue, json};

use crate::app::IcaApp;
use crate::ica::{IcaCommand, types::message::Message};

#[derive(Debug, Clone)]
pub(super) struct ForwardReference {
    pub res_id: String,
    pub file_name: Option<String>,
    pub fallback_res_id: Option<String>,
    pub inline_messages: Option<Vec<Message>>,
    pub preview: Vec<String>,
}

fn marker_value(content: &str, marker: &str) -> Option<String> {
    let start = content.find(marker)? + marker.len();
    let end = content[start..].find(']')? + start;
    let value = content[start..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn code_as_json(code: &JsonValue) -> Option<JsonValue> {
    match code {
        JsonValue::Null => None,
        JsonValue::String(value) => serde_json::from_str(value).ok(),
        value => Some(value.clone()),
    }
}

fn code_res_id(code: &JsonValue) -> Option<String> {
    code_as_json(code)?
        .pointer("/meta/detail/resid")?
        .as_str()
        .map(ToString::to_string)
}

fn code_file_name(code: &JsonValue) -> Option<String> {
    code_as_json(code)?
        .pointer("/meta/detail/uniseq")
        .and_then(JsonValue::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
}

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

fn forward_resource(code: &JsonValue) -> (Option<String>, Option<String>) {
    let res_id = code_res_id(code);
    let file_name = code_file_name(code);
    if res_id.is_some() || file_name.is_some() {
        return (res_id, file_name);
    }
    xml_forward_resource(code)
}

fn push_preview_line(lines: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() && lines.last().is_none_or(|line| line != value) {
        lines.push(value.to_string());
    }
}

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

fn xml_forward_preview(xml: &str) -> Vec<String> {
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

fn forward_preview(code: &JsonValue) -> Vec<String> {
    let json_preview = json_forward_preview(code);
    if !json_preview.is_empty() {
        return json_preview;
    }
    code.as_str().map(xml_forward_preview).unwrap_or_default()
}

pub(super) fn render_forward_preview(ui: &mut egui::Ui, reference: &ForwardReference) {
    const MAX_VISIBLE_LINES: usize = 4;

    for line in reference.preview.iter().take(MAX_VISIBLE_LINES) {
        ui.weak(line);
    }
    if reference.preview.len() > MAX_VISIBLE_LINES {
        ui.weak("...");
    }
}

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

pub(super) fn forward_reference(
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

fn raw_message_elements(message: &Message) -> Option<Vec<JsonValue>> {
    match message.raw_msg.as_deref()? {
        JsonValue::Array(values) if !values.is_empty() => Some(values.clone()),
        JsonValue::Object(map) if map.contains_key("type") => {
            Some(vec![JsonValue::Object(map.clone())])
        }
        _ => None,
    }
}

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

pub(super) fn fake_forward_node(message: &Message) -> JsonValue {
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
    pub(super) fn open_forward_reference(
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
                .open_inline(res_id, file_name, messages);
            return;
        }
        self.request_forward_messages(bridge_idx, res_id, file_name, fallback_res_id);
    }

    fn request_forward_messages(
        &mut self,
        bridge_idx: usize,
        res_id: String,
        file_name: Option<String>,
        fallback_res_id: Option<String>,
    ) {
        if res_id.trim().is_empty() {
            self.bridge_states[bridge_idx].forward_viewer.last_error =
                Some("Res ID 不能为空".to_string());
            return;
        }
        let request_id = self.bridge_states[bridge_idx].forward_viewer.begin_request(
            res_id.clone(),
            file_name.clone(),
            fallback_res_id.clone(),
        );
        if let Err(error) = self.bridge_states[bridge_idx].send(IcaCommand::FetchForwardMessages {
            request_id,
            res_id,
            file_name,
            fallback_res_id,
        }) {
            self.bridge_states[bridge_idx]
                .forward_viewer
                .fail(request_id, format!("查看合并转发命令发送失败: {error}"));
        }
    }

    pub(super) fn send_selected_messages_as_merged_forward(&mut self, target_room_id: i64) -> bool {
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

    pub(in crate::app) fn render_forward_viewer_window(&mut self, ctx: &egui::Context) {
        let Some(bridge_idx) = self.active_bridge_idx else {
            return;
        };
        if !self.bridge_states[bridge_idx].forward_viewer.open {
            return;
        }

        let mut open = true;
        let mut reload = false;
        let mut nested_reference = None;
        let high_contrast = self.custom_chat.high_contrast_chat;
        egui::Window::new("合并转发消息")
            .open(&mut open)
            .default_size(egui::vec2(560.0, 680.0))
            .min_size(egui::vec2(360.0, 360.0))
            .resizable(true)
            .show(ctx, |ui| {
                let viewer = &mut self.bridge_states[bridge_idx].forward_viewer;
                ui.horizontal(|ui| {
                    ui.label("Res ID");
                    ui.add_sized(
                        [ui.available_width() - 72.0, 0.0],
                        egui::TextEdit::singleline(&mut viewer.res_id),
                    );
                    if ui
                        .add_enabled(!viewer.loading, egui::Button::new("加载"))
                        .clicked()
                    {
                        reload = true;
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

                if viewer.messages.is_empty() && !viewer.loading && viewer.last_error.is_none() {
                    ui.weak("没有转发消息");
                }
                let parent_res_id = viewer.res_id.clone();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for message in &viewer.messages {
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.strong(&message.sender_name);
                                ui.weak(format!("{} · {}", message.sender_id, message.time_text));
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
                                    ui.add(
                                        egui::Image::from_uri(file.url.clone())
                                            .max_width(ui.available_width().min(420.0))
                                            .max_height(260.0),
                                    );
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
                                    response = response.on_hover_text(reference.preview.join("\n"));
                                }
                                if response.clicked() {
                                    nested_reference = Some(reference);
                                }
                            }
                        });
                        ui.add_space(6.0);
                    }
                });
            });

        self.bridge_states[bridge_idx].forward_viewer.open = open;
        if reload {
            let res_id = self.bridge_states[bridge_idx].forward_viewer.res_id.clone();
            let file_name = self.bridge_states[bridge_idx]
                .forward_viewer
                .file_name
                .trim()
                .to_string();
            self.request_forward_messages(
                bridge_idx,
                res_id,
                (!file_name.is_empty()).then_some(file_name),
                self.bridge_states[bridge_idx]
                    .forward_viewer
                    .fallback_res_id
                    .clone(),
            );
        }
        if let Some(reference) = nested_reference {
            self.open_forward_reference(
                reference.res_id,
                reference.file_name,
                reference.fallback_res_id,
                reference.inline_messages,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::{Value as JsonValue, json};

    use crate::ica::types::message::{At, Message};

    use super::{fake_forward_node, forward_reference};

    fn message(content: &str, code: JsonValue) -> Message {
        Message {
            msg_id: "m1".to_string(),
            sender_id: 10001,
            sender_name: "Alice".to_string(),
            content: content.to_string(),
            code,
            time: Utc::now(),
            time_text: "12:00:00".to_string(),
            role: String::new(),
            files: Vec::new(),
            reply: None,
            at: At::None,
            deleted: false,
            system: false,
            mirai: JsonValue::Null,
            reveal: false,
            flash: false,
            title: String::new(),
            anonymous_id: None,
            hide: false,
            bubble_id: 1,
            subid: 1,
            head_img: JsonValue::Null,
            raw_msg: None,
        }
    }

    #[test]
    fn parses_remote_and_nested_forward_references() {
        let remote = message("摘要\n[Forward: RES123]", JsonValue::Null);
        let nested = message("[NestedForward: MultiMsg]", JsonValue::Null);

        assert_eq!(forward_reference(&remote, None).unwrap().res_id, "RES123");
        let nested = forward_reference(&nested, Some("PARENT")).unwrap();
        assert_eq!(nested.res_id, "PARENT");
        assert_eq!(nested.file_name.as_deref(), Some("MultiMsg"));
    }

    #[test]
    fn prefers_resource_metadata_and_keeps_nested_fallback() {
        let remote = message(
            "[Forward: display-value]",
            json!({"meta": {"detail": {"resid": "CODE-RES"}}}),
        );
        assert_eq!(forward_reference(&remote, None).unwrap().res_id, "CODE-RES");

        let nested = message(
            "[NestedForward: marker-name]",
            json!({"meta": {"detail": {"resid": "INNER-RES", "uniseq": "CODE-NAME"}}}),
        );
        let reference = forward_reference(&nested, Some("PARENT-RES")).unwrap();
        assert_eq!(reference.res_id, "PARENT-RES");
        assert_eq!(reference.fallback_res_id.as_deref(), Some("INNER-RES"));
        assert_eq!(reference.file_name.as_deref(), Some("CODE-NAME"));
    }

    #[test]
    fn parses_xml_forward_resource_metadata() {
        let message = message(
            "[Forward: display-value]",
            JsonValue::String("<msg m_resid=\"XML-RES\" m_fileName=\"MultiMsg\" />".into()),
        );
        let reference = forward_reference(&message, None).unwrap();
        assert_eq!(reference.res_id, "XML-RES");
    }

    #[test]
    fn fake_node_prefers_original_message_chain() {
        let mut source = message("fallback", JsonValue::Null);
        source.raw_msg = Some(Box::new(json!([
            {"type": "text", "data": {"text": "raw"}},
            {"type": "face", "data": {"id": 14}}
        ])));

        let node = fake_forward_node(&source);
        assert_eq!(node["message"][0]["data"]["text"], "raw");
        assert_eq!(node["message"][1]["type"], "face");
        assert_eq!(node["consistent"], true);
    }
}
