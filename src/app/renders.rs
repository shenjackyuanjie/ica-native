mod central_panel;
mod chat_list;
mod left_groups;
mod message_card;
mod top_panel;
mod windows;

pub(super) fn format_message_content(content: &str) -> String {
    let open_tag = "<IcalinguaAt qq=";
    let close_tag = "</IcalinguaAt>";
    let mut result = String::with_capacity(content.len());
    let mut remaining = content;

    while let Some(start_idx) = remaining.find(open_tag) {
        let (before, after_start) = remaining.split_at(start_idx);
        result.push_str(before);

        let Some(tag_end_idx) = after_start.find('>') else {
            result.push_str(after_start);
            return result;
        };
        let tag_body = &after_start[tag_end_idx + 1..];
        let Some(close_idx) = tag_body.find(close_tag) else {
            result.push_str(after_start);
            return result;
        };

        let encoded_name = &tag_body[..close_idx];
        match urlencoding::decode(encoded_name) {
            Ok(decoded) => result.push_str(decoded.as_ref()),
            Err(_) => result.push_str(encoded_name),
        }

        remaining = &tag_body[close_idx + close_tag.len()..];
    }

    result.push_str(remaining);
    result
}

pub(super) fn estimate_composer_rows(text: &str, input_width: f32) -> usize {
    const MIN_ROWS: usize = 1;
    const MAX_ROWS: usize = 6;

    if text.is_empty() {
        return MIN_ROWS;
    }

    let chars_per_row = (input_width / 8.5).floor().max(12.0) as usize;
    text.split('\n')
        .map(|line| {
            let weighted_chars = line
                .chars()
                .map(|ch| if ch.is_ascii() { 1 } else { 2 })
                .sum::<usize>();
            weighted_chars.div_ceil(chars_per_row).max(1)
        })
        .sum::<usize>()
        .clamp(MIN_ROWS, MAX_ROWS)
}

pub(super) fn format_pending_size(bytes: usize) -> String {
    let size = bytes as f64;
    if size >= 1024.0 * 1024.0 {
        format!("{:.1} MB", size / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", size / 1024.0)
    }
}

const MESSAGE_LIST_OVERSCAN: f32 = 720.0;

#[derive(Clone, Copy)]
pub(super) struct MessageRowLayout {
    pub(super) top: f32,
    pub(super) height: f32,
    pub(super) show_sender_name: bool,
    pub(super) show_separator_before: bool,
}

fn weighted_text_len(text: &str) -> usize {
    text.chars()
        .map(|ch| if ch.is_ascii() { 1 } else { 2 })
        .sum()
}

fn estimate_text_height(text: &str, width: f32, line_height: f32) -> f32 {
    if text.trim().is_empty() {
        return 0.0;
    }

    let chars_per_line = (width / 8.5).floor().max(8.0) as usize;
    let lines = text
        .split('\n')
        .map(|line| weighted_text_len(line).div_ceil(chars_per_line).max(1))
        .sum::<usize>()
        .max(1);
    lines as f32 * line_height
}

pub(super) fn estimate_message_row_height(
    message: &crate::ica::types::message::Message,
    row_width: f32,
    line_height: f32,
    pure_text_mode: bool,
    forward_mode_active: bool,
    show_sender_name: bool,
    show_separator_before: bool,
) -> f32 {
    if message.system {
        return 36.0;
    }

    let selection_width = if forward_mode_active { 24.0 } else { 0.0 };
    let content_row_width = (row_width - selection_width).max(48.0);
    let bubble_width = if pure_text_mode {
        content_row_width
    } else {
        (content_row_width * 0.78)
            .clamp(72.0, 680.0)
            .min(content_row_width)
    };
    let content_width = if pure_text_mode {
        bubble_width
    } else {
        (bubble_width - 28.0).max(44.0)
    };

    let mut height = if pure_text_mode && show_separator_before {
        10.0
    } else {
        0.0
    };

    height += line_height + 8.0;

    let message_is_hidden = (message.deleted || message.hide) && !message.reveal;
    if message_is_hidden {
        height += line_height + 10.0;
        return height.max(48.0);
    }

    if let Some(reply) = &message.reply {
        height += line_height + estimate_text_height(&reply.content, content_width, line_height);
        height += if pure_text_mode { 8.0 } else { 24.0 };
    }

    if !message.content.is_empty() {
        height += estimate_text_height(&message.content, content_width, line_height) + 6.0;
    }

    if !message.files.is_empty() {
        for file in &message.files {
            let file_type = file.file_type.to_ascii_lowercase();
            let is_image = file_type == "image" || file_type.starts_with("image/");
            height += if is_image && !file.url.is_empty() {
                248.0
            } else {
                line_height + 8.0
            };
        }
    }

    if message.content.is_empty() && message.files.is_empty() {
        height += line_height + 6.0;
    }

    if show_sender_name {
        height += 0.0;
    }

    height += if pure_text_mode { 6.0 } else { 16.0 };
    height.max(56.0)
}

pub(super) fn message_visible_range(
    rows: &[MessageRowLayout],
    viewport_top: f32,
    viewport_bottom: f32,
) -> (usize, usize) {
    let start_cutoff = viewport_top - MESSAGE_LIST_OVERSCAN;
    let end_cutoff = viewport_bottom + MESSAGE_LIST_OVERSCAN;
    let start = rows.partition_point(|row| row.top + row.height < start_cutoff);
    let end = start + rows[start..].partition_point(|row| row.top < end_cutoff);

    (start, end.min(rows.len()))
}
