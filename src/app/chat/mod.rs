//! 聊天界面模块。
//!
//! 按职责聚合聊天区的子模块：`actions`（发送与消息操作）、`central_panel`
//! （中央消息区）、`clipboard`（剪贴板）、`composer`（输入区）、`forward`
//! （转发多选）、`group_announcements`/`group_files`/`group_members`（群公告、
//! 群文件与群成员）、`image_viewer`（图片查看器）、`message_card`（消息卡片）、
//! `message_ops`/`message_search`（消息操作与消息内搜索）、`notices`（顶部
//! 通知栏）、`room_ops`（房间操作）与 `search`（会话搜索）。
//!
//! 本文件同时提供跨子模块共用的无状态工具：消息正文的 @ 提及解码与格式化、
//! GIF 纹理探测与帧选择、输入区文本与光标编辑，以及虚拟消息列表的行高估算
//! 与可见范围计算。渲染相关常量（overscan、气泡宽度等）与 `message_card`
//! 的实际渲染保持同一套数值，改动时需两边同步。

mod actions;
mod central_panel;
mod clipboard;
mod composer;
mod forward;
mod group_announcements;
mod group_files;
mod group_members;
mod image_viewer;
mod message_card;
mod message_ops;
mod message_search;
mod notices;
mod room_ops;
mod search;
#[cfg(test)]
mod tests;

use crate::app::MessageRowLayout;
use std::borrow::Cow;
use std::ops::Range;
use std::time::Duration;

/// 把 egui 的字符下标转换为普通 `usize`，便于与 `String` 的字符计数比较。
fn char_index_to_usize(index: egui::text::CharIndex) -> usize {
    index.into()
}

/// 把消息正文里的 @ 提及标记替换为可读昵称，供消息卡片与输入框共用。
///
/// 同时兼容旧版 `<IcalinguaAt qq=..>`（URL 编码昵称）与新版 `<IcaAt qq=..>`
/// （XML 实体编码昵称）：每次取剩余文本中位置最靠前的开放标签，解码其中的
/// 昵称后拼接结果；标签不完整（缺少 `>` 或闭合标签）时按原样输出剩余文本，
/// 不猜测、不补全。不含任何提及标记时返回借用切片，避免分配。
pub fn format_message_content(content: &str) -> Cow<'_, str> {
    if !content.contains("<IcalinguaAt qq=") && !content.contains("<IcaAt qq=") {
        return Cow::Borrowed(content);
    }

    let mut result = String::with_capacity(content.len());
    let mut remaining = content;

    while let Some((start_idx, _open_tag, close_tag, xml_escaped)) = [
        ("<IcalinguaAt qq=", "</IcalinguaAt>", false),
        ("<IcaAt qq=", "</IcaAt>", true),
    ]
    .into_iter()
    .filter_map(|(open_tag, close_tag, xml_escaped)| {
        remaining
            .find(open_tag)
            .map(|start_idx| (start_idx, open_tag, close_tag, xml_escaped))
    })
    .min_by_key(|(start_idx, ..)| *start_idx)
    {
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
        if xml_escaped {
            result.push_str(&decode_xml_entities(encoded_name));
        } else {
            match urlencoding::decode(encoded_name) {
                Ok(decoded) => result.push_str(decoded.as_ref()),
                Err(_) => result.push_str(encoded_name),
            }
        }

        remaining = &tag_body[close_idx + close_tag.len()..];
    }

    result.push_str(remaining);
    Cow::Owned(result)
}

/// 判断附件类型是否为图片。
///
/// 兼容两种写法：Bridge 可能给出裸 `image` 或完整 MIME `image/...`；
/// 比较忽略大小写。
pub fn is_image_file_type(file_type: &str) -> bool {
    file_type.eq_ignore_ascii_case("image")
        || file_type
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("image/"))
}

/// 解码 ICA 新版 `<IcaAt>` 中使用的 XML 文本实体；只解码一层，避免把用户原文二次解释。
pub fn decode_xml_entities(value: &str) -> Cow<'_, str> {
    if !value.contains('&') {
        return Cow::Borrowed(value);
    }
    let mut decoded = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(start) = remaining.find('&') {
        decoded.push_str(&remaining[..start]);
        let tail = &remaining[start..];
        let Some(end) = tail.find(';') else {
            decoded.push_str(tail);
            return Cow::Owned(decoded);
        };
        let entity = &tail[..=end];
        let replacement = match entity {
            "&amp;" => Some('&'),
            "&lt;" => Some('<'),
            "&gt;" => Some('>'),
            "&quot;" => Some('"'),
            "&apos;" => Some('\''),
            _ => entity
                .strip_prefix("&#x")
                .or_else(|| entity.strip_prefix("&#X"))
                .and_then(|number| number.strip_suffix(';'))
                .and_then(|number| u32::from_str_radix(number, 16).ok())
                .and_then(char::from_u32)
                .or_else(|| {
                    entity
                        .strip_prefix("&#")
                        .and_then(|number| number.strip_suffix(';'))
                        .and_then(|number| number.parse::<u32>().ok())
                        .and_then(char::from_u32)
                }),
        };
        if let Some(replacement) = replacement {
            decoded.push(replacement);
        } else {
            decoded.push_str(entity);
        }
        remaining = &tail[end + 1..];
    }
    decoded.push_str(remaining);
    Cow::Owned(decoded)
}

/// 判断附件类型是否为视频。
///
/// 只识别 `video/...` MIME，不像图片那样兼容裸 `video`。
pub fn is_video_file_type(file_type: &str) -> bool {
    file_type
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("video/"))
}

/// 按 URL 路径（忽略 query 与 fragment）猜测是否为 GIF。
///
/// 兼容 `.gif` 后缀与 `.gif/` 形式的路径段；仅用于静态加载失败后的格式
/// 重探测，不作为展示依据。
pub fn image_url_looks_like_gif(url: &str) -> bool {
    let url = url.to_ascii_lowercase();
    url.split(['?', '#'])
        .next()
        .is_some_and(|path| path.ends_with(".gif") || path.contains(".gif/"))
}

/// 判断静态图片加载失败后是否值得再按 GIF 探测一次。
///
/// 只有"没有匹配的加载器"与"格式不支持"两类错误说明格式判断可能出错；
/// 网络类错误重试同样没有结果，直接放过。
pub fn should_probe_gif_after_static_error(err: &egui::load::LoadError) -> bool {
    matches!(
        err,
        egui::load::LoadError::NoMatchingImageLoader { .. }
            | egui::load::LoadError::FormatNotSupported { .. }
    )
}

/// 尝试把 `url` 作为 GIF 加载，返回纹理加载结果；不是 GIF 时返回 `None`，
/// 让调用方回退到普通静态图片加载。
///
/// 优先复用 egui 已记录的帧时长元数据：命中时直接加载当前帧 URI，避免重复
/// 下载源字节；同 URI 被替换为静态图导致格式探测失败时，先清理过期元数据
/// 再重试。字节就绪后按 MIME 与 GIF 魔数双重判定，任一通过才交给帧加载。
pub fn try_load_gif_texture(
    ctx: &egui::Context,
    url: &str,
    texture_options: egui::TextureOptions,
    size_hint: egui::load::SizeHint,
) -> Option<egui::load::TextureLoadResult> {
    // 已成功解码的 GIF 有帧时长记录。源 bytes 会在上传后释放，不应每帧
    // 再探测/下载它；帧缓存未命中时，ImageLoader 自己会重新取源并后台解码。
    if ctx.data(|data| {
        data.get_temp::<egui::FrameDurations>(egui::Id::new(url))
            .is_some()
    }) {
        let frame_uri = gif_frame_uri(ctx, url);
        let result = ctx.try_load_texture(&frame_uri, texture_options, size_hint);
        if !result
            .as_ref()
            .is_err_and(should_probe_gif_after_static_error)
        {
            return Some(result);
        }
        // 同 URI 内容被替换为静态图时，不让过期的 GIF 元数据挡住格式重新探测。
        ctx.data_mut(|data| data.remove::<egui::FrameDurations>(egui::Id::new(url)));
    }
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

/// 计算当前应显示的帧 URI：按帧时长元数据推算帧序，拼成 `url#序号`。
///
/// 没有元数据时返回第 0 帧对应的 URI。
fn gif_frame_uri(ctx: &egui::Context, url: &str) -> String {
    let frame_index = ctx
        .data(|data| data.get_temp::<egui::FrameDurations>(egui::Id::new(url)))
        .map(|durations| gif_frame_index(ctx, &durations))
        .unwrap_or(0);
    format!("{url}#{frame_index}")
}

/// 按当前时间在帧时长序列中的位置计算帧序，并请求在下一帧前重绘。
///
/// 总时长为 0 时退化为第 0 帧，避免除零与空转。
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

/// 按字符区间替换文本内容，返回替换后光标应处的字符下标。
///
/// 区间按字符计数钳制到合法范围，内部换算为字节区间后替换；替换文本可能
/// 改变总字符数，返回值供调用方更新光标。
pub fn replace_text_char_range(text: &mut String, range: Range<usize>, replacement: &str) -> usize {
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

/// 在指定输入框保存的光标/选区处插入文本，并写回更新后的光标状态。
///
/// 读不到输入框状态时退化为追加到正文末尾；有选区时先替换整个选区。
pub fn insert_text_at_saved_cursor(
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

/// 判断指定输入框的光标前一个字符是否为 `expected`。
///
/// 读不到输入框状态或光标在开头时返回 `false`；用于 @ 触发等场景判断。
pub fn saved_cursor_preceded_by(
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

/// 在保存的光标处插入 @ 提及文本。
///
/// `replace_at_trigger` 为真且光标紧跟在 `@` 之后时，把触发字符一并替换，
/// 避免提及选择后残留一个孤立的 `@`。
pub fn insert_mention_at_saved_cursor(
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

/// 估算输入框文本占用的行数，结果钳制在 1 到 6 行。
///
/// 非 ASCII 字符按 2 列权重计算，按 `input_width` 折算每行字符数；结果只
/// 用于输入框高度预估，不与实际渲染逐像素对齐。
pub fn estimate_composer_rows(text: &str, input_width: f32) -> usize {
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

/// 把待发送附件的字节数格式化为 KB/MB 文本，供输入区附件列表展示。
pub fn format_pending_size(bytes: usize) -> String {
    let size = bytes as f64;
    if size >= 1024.0 * 1024.0 {
        format!("{:.1} MB", size / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", size / 1024.0)
    }
}

/// 消息列表可视范围计算时在视口上下各额外保留的像素数。
///
/// 预留滚动 overscan，减少快速滚动时出现空白与抖动。
const MESSAGE_LIST_OVERSCAN: f32 = 720.0;

/// 计算文本的加权长度：ASCII 计 1，其余字符计 2。
///
/// 与 `estimate_composer_rows` 使用同一套权重，保证行高与行数估算一致。
fn weighted_text_len(text: &str) -> usize {
    text.chars()
        .map(|ch| if ch.is_ascii() { 1 } else { 2 })
        .sum()
}

/// 按宽度与行高估算多行文本的渲染高度；纯空白文本返回 0。
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

/// 估算消息行高度所需的布局参数。
#[derive(Debug, Clone, Copy)]
pub struct MessageRowEstimateOptions {
    /// 是否处于纯文本模式（不用气泡背景，宽度折算不同）。
    pub pure_text_mode: bool,
    /// 是否显示消息头像（占用固定宽度）。
    pub show_message_avatar: bool,
    /// 转发多选模式是否激活（左侧预留勾选框宽度）。
    pub forward_mode_active: bool,
    /// 是否显示发送者名称。
    pub show_sender_name: bool,
    /// 上方是否有日期/分组分隔线（占一行高度）。
    pub show_separator_before: bool,
}

/// 估算一条消息在虚拟列表中占据的行高（像素）。
///
/// 与 `message_card` 的实际渲染保持同一套常量：系统消息、已删除/隐藏消息、
/// 引用、正文、图片附件各有固定或按文本折算的增量，气泡宽度按模式钳制。
/// 估算值与实测值的偏差由虚拟列表的冷行校准补偿，这里只求稳定可比。
pub fn estimate_message_row_height(
    message: &crate::ica::types::message::Message,
    row_width: f32,
    line_height: f32,
    options: MessageRowEstimateOptions,
) -> f32 {
    if message.system {
        return 36.0;
    }

    let selection_width = if options.forward_mode_active {
        24.0
    } else {
        0.0
    };
    let avatar_width = if options.show_message_avatar {
        40.0
    } else {
        0.0
    };
    let content_row_width = (row_width - selection_width - avatar_width).max(48.0);
    let bubble_width = if options.pure_text_mode {
        content_row_width
    } else {
        (content_row_width * 0.78)
            .clamp(72.0, 680.0)
            .min(content_row_width)
    };
    let content_width = if options.pure_text_mode {
        bubble_width
    } else {
        (bubble_width - 28.0).max(44.0)
    };

    let mut height = if options.pure_text_mode && options.show_separator_before {
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
        height += if options.pure_text_mode { 8.0 } else { 24.0 };
        if !options.pure_text_mode
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

    if options.show_sender_name {
        height += 0.0;
    }

    height += if options.pure_text_mode { 6.0 } else { 16.0 };
    height.max(56.0)
}

/// 按视口范围（含上下 overscan）计算消息列表的可见行区间 `[start, end)`。
///
/// 行数据已按 `top` 排序，用两次二分查找定位边界；`end` 钳制到行数。
pub fn message_visible_range(
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
