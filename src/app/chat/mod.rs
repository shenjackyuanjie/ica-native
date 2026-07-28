mod actions;
mod central_panel;
mod clipboard;
mod composer;
mod composer_drop;
mod forward;
mod group_members;
mod image_viewer;
mod message_card;
mod message_ops;
mod message_search;
mod room_ops;
mod search;

use std::borrow::Cow;
use std::ops::Range;
use std::time::Duration;

use crate::app::MessageRowLayout;

fn char_index_to_usize(index: egui::text::CharIndex) -> usize {
    index.into()
}

pub(super) fn format_message_content(content: &str) -> Cow<'_, str> {
    let open_tag = "<IcalinguaAt qq=";
    let close_tag = "</IcalinguaAt>";
    if !content.contains(open_tag) {
        return Cow::Borrowed(content);
    }

    let mut result = String::with_capacity(content.len());
    let mut remaining = content;

    while let Some(start_idx) = remaining.find(open_tag) {
        let (before, after_start) = remaining.split_at(start_idx);
        result.push_str(before);

        let Some(tag_end_idx) = after_start.find('>') else {
            result.push_str(after_start);
            return Cow::Owned(result);
        };
        let tag_body = &after_start[tag_end_idx + 1..];
        let Some(close_idx) = tag_body.find(close_tag) else {
            result.push_str(after_start);
            return Cow::Owned(result);
        };

        let encoded_name = &tag_body[..close_idx];
        match urlencoding::decode(encoded_name) {
            Ok(decoded) => result.push_str(decoded.as_ref()),
            Err(_) => result.push_str(encoded_name),
        }

        remaining = &tag_body[close_idx + close_tag.len()..];
    }

    result.push_str(remaining);
    Cow::Owned(result)
}

pub(crate) fn is_image_file_type(file_type: &str) -> bool {
    file_type.eq_ignore_ascii_case("image")
        || file_type
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("image/"))
}

pub(super) fn image_url_looks_like_gif(url: &str) -> bool {
    let url = url.to_ascii_lowercase();
    url.split(['?', '#'])
        .next()
        .is_some_and(|path| path.ends_with(".gif") || path.contains(".gif/"))
}

pub(super) fn should_probe_gif_after_static_error(err: &egui::load::LoadError) -> bool {
    matches!(
        err,
        egui::load::LoadError::NoMatchingImageLoader { .. }
            | egui::load::LoadError::FormatNotSupported { .. }
    )
}

pub(super) fn try_load_gif_texture(
    ctx: &egui::Context,
    url: &str,
    texture_options: egui::TextureOptions,
    size_hint: egui::load::SizeHint,
) -> Option<egui::load::TextureLoadResult> {
    match ctx.try_load_bytes(url) {
        Ok(egui::load::BytesPoll::Pending { size }) => {
            Some(Ok(egui::load::TexturePoll::Pending { size }))
        }
        Ok(egui::load::BytesPoll::Ready { bytes, mime, .. }) => {
            let is_gif_mime = mime
                .as_deref()
                .is_some_and(|mime| mime.to_ascii_lowercase().contains("image/gif"));
            if !is_gif_mime && !egui::has_gif_magic_header(&bytes) {
                return None;
            }
            if !egui::has_gif_magic_header(&bytes) {
                return None;
            }

            let frame_uri = gif_frame_uri(ctx, url);
            Some(ctx.try_load_texture(&frame_uri, texture_options, size_hint))
        }
        Err(err) => Some(Err(err)),
    }
}

fn gif_frame_uri(ctx: &egui::Context, url: &str) -> String {
    let frame_index = ctx
        .data(|data| data.get_temp::<egui::FrameDurations>(egui::Id::new(url)))
        .map(|durations| gif_frame_index(ctx, &durations))
        .unwrap_or(0);
    format!("{url}#{frame_index}")
}

fn gif_frame_index(ctx: &egui::Context, durations: &egui::FrameDurations) -> usize {
    let now = ctx.input(|input| Duration::from_secs_f64(input.time));
    let total: Duration = durations.all().sum();
    let pos_ms = now.as_millis() % total.as_millis().max(1);
    let mut cumulative_ms = 0;

    for (index, duration) in durations.all().enumerate() {
        cumulative_ms += duration.as_millis();
        if pos_ms < cumulative_ms {
            let ms_until_next_frame = cumulative_ms - pos_ms;
            ctx.request_repaint_after(Duration::from_millis(ms_until_next_frame as u64));
            return index;
        }
    }

    0
}

pub(super) fn replace_text_char_range(
    text: &mut String,
    range: Range<usize>,
    replacement: &str,
) -> usize {
    let char_count = text.chars().count();
    let start = range.start.min(char_count);
    let end = range.end.clamp(start, char_count);
    let start_byte = text
        .char_indices()
        .nth(start)
        .map_or(text.len(), |(idx, _)| idx);
    let end_byte = text
        .char_indices()
        .nth(end)
        .map_or(text.len(), |(idx, _)| idx);
    text.replace_range(start_byte..end_byte, replacement);
    start + replacement.chars().count()
}

pub(super) fn insert_text_at_saved_cursor(
    ctx: &egui::Context,
    id: egui::Id,
    text: &mut String,
    replacement: &str,
) {
    let mut edit_state = egui::widgets::text_edit::TextEditState::load(ctx, id);
    let selected_range = edit_state
        .as_ref()
        .and_then(|state| state.cursor.char_range())
        .map(|range| range.as_sorted_char_range())
        .map(|range| char_index_to_usize(range.start)..char_index_to_usize(range.end));
    let new_cursor = if let Some(range) = selected_range {
        replace_text_char_range(text, range, replacement)
    } else {
        text.push_str(replacement);
        text.chars().count()
    };

    if let Some(mut state) = edit_state.take() {
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(new_cursor),
            )));
        state.store(ctx, id);
    }
}

pub(super) fn saved_cursor_preceded_by(
    ctx: &egui::Context,
    id: egui::Id,
    text: &str,
    expected: char,
) -> bool {
    let Some(cursor) = egui::widgets::text_edit::TextEditState::load(ctx, id)
        .and_then(|state| state.cursor.char_range())
    else {
        return false;
    };
    let cursor = char_index_to_usize(cursor.primary.index);
    cursor > 0 && text.chars().nth(cursor - 1) == Some(expected)
}

pub(super) fn insert_mention_at_saved_cursor(
    ctx: &egui::Context,
    id: egui::Id,
    text: &mut String,
    mention: &str,
    replace_at_trigger: bool,
) {
    let mut edit_state = egui::widgets::text_edit::TextEditState::load(ctx, id);
    let selected_range = edit_state
        .as_ref()
        .and_then(|state| state.cursor.char_range())
        .map(|range| range.as_sorted_char_range())
        .map(|range| char_index_to_usize(range.start)..char_index_to_usize(range.end));
    let range = selected_range.map(|range| {
        if replace_at_trigger
            && range.is_empty()
            && range.start > 0
            && text.chars().nth(range.start - 1) == Some('@')
        {
            range.start - 1..range.end
        } else {
            range
        }
    });
    let new_cursor = if let Some(range) = range {
        replace_text_char_range(text, range, mention)
    } else {
        text.push_str(mention);
        text.chars().count()
    };

    if let Some(mut state) = edit_state.take() {
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(new_cursor),
            )));
        state.store(ctx, id);
    }
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
        if !pure_text_mode
            && reply
                .file
                .iter()
                .chain(&reply.files)
                .any(|file| is_image_file_type(&file.file_type) && !file.url.is_empty())
        {
            height += 104.0;
        }
    }

    if !message.content.is_empty() {
        height += estimate_text_height(&message.content, content_width, line_height) + 6.0;
    }

    if !message.files.is_empty() {
        for file in &message.files {
            let is_image = is_image_file_type(&file.file_type);
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

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{format_message_content, replace_text_char_range};

    #[test]
    fn plain_message_uses_borrowed_fast_path() {
        let content = "普通消息 [Face: 1]";
        let formatted = format_message_content(content);

        assert!(matches!(formatted, Cow::Borrowed(_)));
        assert_eq!(formatted, content);
    }

    #[test]
    fn at_markup_is_decoded() {
        let content = "你好 <IcalinguaAt qq=10001>Alice%20A</IcalinguaAt>";

        assert_eq!(format_message_content(content), "你好 Alice A");
    }

    #[test]
    fn malformed_at_markup_is_preserved() {
        let content = "你好 <IcalinguaAt qq=10001>Alice";

        assert_eq!(format_message_content(content), content);
    }

    #[test]
    fn insertion_uses_character_cursor_for_unicode_text() {
        let mut content = "你ab好".to_string();

        let cursor = replace_text_char_range(&mut content, 1..1, "[Face: 1]");

        assert_eq!(content, "你[Face: 1]ab好");
        assert_eq!(cursor, 10);
    }

    #[test]
    fn insertion_replaces_selected_characters() {
        let mut content = "你ab好".to_string();

        let cursor = replace_text_char_range(&mut content, 1..3, "表情");

        assert_eq!(content, "你表情好");
        assert_eq!(cursor, 3);
    }
}
