//! QQ 群公告（只读）的 Web CGI 协议层。
//!
//! Icalingua++ 并没有实现公告协议，它只是用 Electron 打开手Q 的 H5 页
//! （见 `icalingua/src/main/utils/groupWebApps.ts` 的 `openGroupAnnouncements`）。
//! ica-native 没有 WebView，因此这里直接复刻 oicq 使用的公告列表接口：
//! `https://web.qun.qq.com/cgi-bin/announce/get_t_list`，用 bridge 下发的
//! `qun.qq.com` Cookie 与 bkn 鉴权。

#[cfg(test)]
#[path = "tests/announcement.rs"]
mod tests;

use serde_json::Value as JsonValue;

/// 公告列表 CGI。
///
/// 这里用的是手Q 群公告 H5 当前在用的 `list_announce`，而不是 oicq 里那个更老的
/// `get_t_list`：后者只返回 `feeds`，既没有置顶标记，也不返回“发给新成员”的公告。
const ANNOUNCEMENT_LIST_ENDPOINT: &str = "https://web.qun.qq.com/cgi-bin/announce/list_announce";

/// 发布与编辑普通公告的 CGI。带上 `fid` 即为编辑已有公告。
const ANNOUNCEMENT_ADD_ENDPOINT: &str = "https://web.qun.qq.com/cgi-bin/announce/add_qun_notice";

/// 发布与编辑「发给新成员」公告的 CGI。
///
/// 手Q H5 按 `toNew` 在 `addQunNotice` 与 `addQunInstruction` 之间二选一，
/// 两者参数一致，只是入口不同。
const ANNOUNCEMENT_ADD_INSTRUCTION_ENDPOINT: &str =
    "https://web.qun.qq.com/cgi-bin/announce/add_qun_instruction";

/// 删除公告的 CGI。
const ANNOUNCEMENT_DELETE_ENDPOINT: &str = "https://web.qun.qq.com/cgi-bin/announce/del_feed";

/// 拉取公告时使用的 Cookie 域名，必须在 Bridge 的允许列表内。
pub const ANNOUNCEMENT_COOKIE_DOMAIN: &str = "qun.qq.com";

/// 单次拉取的公告条数，与手Q H5 的默认分页一致。
pub const ANNOUNCEMENT_PAGE_SIZE: u32 = 20;

/// 公告列表请求地址。
pub fn announcement_list_url() -> &'static str {
    ANNOUNCEMENT_LIST_ENDPOINT
}

/// 拼接公告列表的表单请求体。
///
/// 参数取自 H5 自身的首屏调用：`ft=23` 固定，`s=-1` 表示从最新一条开始。
/// `i=1` 允许返回公告说明，`ni=1` 才会要求服务端一并返回 `inst`
/// （发给新成员的公告）；两者缺一不可。
pub fn announcement_list_form(bkn: i64, group_id: i64) -> String {
    format!("qid={group_id}&bkn={bkn}&ft=23&s=-1&n={ANNOUNCEMENT_PAGE_SIZE}&i=1&ni=1")
}

/// 复刻 JavaScript 的 `ToInt32`：按 2^32 取模后再按有符号 32 位解释。
fn to_int32(value: i64) -> i32 {
    value as u32 as i32
}

/// 由 `skey` 推导 bkn。
///
/// 算法与 oicq 的 `Client.bkn` 逐位一致：累加器是 JS 的双精度数，但每轮的
/// `bkn << 5` 会先做 `ToInt32` 再截断到 32 位，最后整体掩码到 31 位。
/// 这里用 i64 承载累加器（skey 长度有限，绝不会超过 f64 的精确整数范围），
/// 并显式复刻两处 32 位截断，避免直接用 i32 累加得到不同结果。
pub fn bkn_from_skey(skey: &str) -> i64 {
    let mut bkn: i64 = 5381;
    for byte in skey.bytes() {
        let shifted = to_int32(bkn).wrapping_shl(5);
        bkn = bkn + i64::from(shifted) + i64::from(byte);
    }
    i64::from(to_int32(bkn) & 0x7fff_ffff)
}

/// 从 Cookie 串中取出 `skey`。
///
/// 必须按 `;` 分段后精确匹配键名：QQ 的 Cookie 里同时存在 `skey` 与 `p_skey`，
/// 用子串查找会错取到 `p_skey`，进而算出错误的 bkn。
pub fn skey_from_cookie(cookie: &str) -> Option<&str> {
    cookie.split(';').find_map(|entry| {
        let (name, value) = entry.split_once('=')?;
        (name.trim() == "skey").then(|| value.trim())
    })
}

/// 决定本次请求使用的 bkn。
///
/// 优先用 `onlineData` 下发的 bkn；onebot / milky 适配器要等第一次 `getCookies`
/// 之后才会回填 bkn，这种情况下退回到用 Cookie 里的 skey 现算。
pub fn resolve_bkn(online_bkn: i64, cookie: &str) -> Option<i64> {
    if online_bkn > 0 {
        return Some(online_bkn);
    }
    let skey = skey_from_cookie(cookie)?;
    if skey.is_empty() {
        return None;
    }
    Some(bkn_from_skey(skey))
}

/// 公告配图的 CDN 前缀。
///
/// 取自手Q 群公告 H5 自身的实现（`index.bundle.js` 里
/// `"//gdynamic.qpic.cn/gdynamic/".concat(pics[0].id, "/628")`），
/// 路径末段是目标宽度，实测 628 为列表用尺寸、0 为原图。
const ANNOUNCEMENT_IMAGE_BASE: &str = "https://gdynamic.qpic.cn/gdynamic";

/// 列表缩略图宽度，与手Q H5 保持一致。
const ANNOUNCEMENT_THUMBNAIL_WIDTH: u32 = 628;

/// 公告正文里的配图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupAnnouncementImage {
    /// CDN 上的图片标识，URL 只由它拼成，与群号和公告 id 无关。
    pub id: String,
    /// CGI 声明的原始像素宽高，缺失或非数字时为 None。
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl GroupAnnouncementImage {
    /// 列表中展示用的缩略图。
    pub fn thumbnail_url(&self) -> String {
        format!(
            "{ANNOUNCEMENT_IMAGE_BASE}/{}/{ANNOUNCEMENT_THUMBNAIL_WIDTH}",
            self.id
        )
    }

    /// 原图；宽度位传 0 表示不缩放。
    pub fn original_url(&self) -> String {
        format!("{ANNOUNCEMENT_IMAGE_BASE}/{}/0", self.id)
    }

    /// 按声明的宽高比算出限定宽度下的显示高度。
    ///
    /// 图片解码完成前 egui 不知道真实尺寸，先用这个值占位可以避免列表在
    /// 图片陆续加载时反复跳动。宽高缺失时返回 None，交给调用方用默认高度。
    pub fn display_height(&self, display_width: f32) -> Option<f32> {
        let (width, height) = (self.width?, self.height?);
        if width == 0 {
            return None;
        }
        Some(display_width * height as f32 / width as f32)
    }
}

/// 一条群公告。
#[derive(Debug, Clone, PartialEq)]
pub struct GroupAnnouncement {
    /// 公告 id，删除和查看详情都用它。
    pub fid: String,
    /// 发布者 QQ。
    pub sender_id: i64,
    /// 发布时间（秒级时间戳）。
    pub publish_time: i64,
    pub title: String,
    pub text: String,
    pub images: Vec<GroupAnnouncementImage>,
    /// 已读人数；QQ 未下发时为 None。
    pub read_count: Option<i64>,
    /// 是否要求群成员确认收到。
    pub confirm_required: bool,
    /// 是否允许成员在公告里直接修改群名片。
    pub show_edit_card: bool,
    /// 提醒方式：0 = 弹窗提醒，1 = 仅发到群里。
    pub tip_window_type: i64,
    /// 是否置顶；`list_announce` 的 `feeds` 里用 `pinned == 1` 表示。
    pub pinned: bool,
    /// 是否为“发给新成员”的公告。
    ///
    /// 这类公告不在 `feeds` 里，而是单独放在响应的 `inst` 数组；H5 也把
    /// `type == 20` 当作同一种东西（发布时走 `add_qun_instruction`）。
    pub to_new: bool,
    /// 原始 feed，供界面复制排查用。
    pub raw: JsonValue,
}

fn json_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(text) => text.clone(),
        JsonValue::Null => String::new(),
        other => other.to_string(),
    }
}

fn json_i64(value: &JsonValue) -> Option<i64> {
    match value {
        JsonValue::Number(number) => number.as_i64(),
        JsonValue::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// 配图宽高在 CGI 里是字符串形式的数字。
fn json_u32(value: &JsonValue) -> Option<u32> {
    json_i64(value).and_then(|value| u32::try_from(value).ok())
}

/// 解一层 HTML 实体。
///
/// 手Q H5 的 `decodeText` 是按 `&amp;` → `&#10;` → `&nbsp;` 顺序做多次全局替换的，
/// 那种写法会把正文里的 `&amp;#10;` 先变成 `&#10;` 再变成换行，等于二次解释用户原文。
/// 这里改为单次扫描：每个实体只解一次，因此 `&amp;#10;` 会稳定还原成字面量 `&#10;`。
fn decode_entities(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }

    let mut decoded = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(start) = remaining.find('&') {
        decoded.push_str(&remaining[..start]);
        let tail = &remaining[start..];
        let Some(end) = tail.find(';') else {
            decoded.push_str(tail);
            return decoded;
        };
        let entity = &tail[..=end];
        let replacement = match entity {
            "&amp;" => Some('&'),
            "&lt;" => Some('<'),
            "&gt;" => Some('>'),
            "&quot;" => Some('"'),
            "&apos;" | "&#39;" => Some('\''),
            "&nbsp;" => Some(' '),
            _ => entity
                .strip_prefix("&#x")
                .or_else(|| entity.strip_prefix("&#X"))
                .and_then(|number| number.strip_suffix(';'))
                .and_then(|number| u32::from_str_radix(number, 16).ok())
                .or_else(|| {
                    entity
                        .strip_prefix("&#")
                        .and_then(|number| number.strip_suffix(';'))
                        .and_then(|number| number.parse::<u32>().ok())
                })
                .and_then(char::from_u32),
        };
        match replacement {
            Some(replacement) => decoded.push(replacement),
            None => decoded.push_str(entity),
        }
        remaining = &tail[end + 1..];
    }
    decoded.push_str(remaining);
    decoded
}

/// 剥离正文里的 C0 控制字符，只保留换行与制表符。
///
/// QQ 会用 U+0001 / U+0002 把正文中的链接包起来，这类字符在桌面端会渲染成豆腐块。
fn strip_control_characters(value: &str) -> String {
    if !value
        .chars()
        .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
    {
        return value.to_string();
    }
    value
        .chars()
        .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
        .collect()
}

/// 还原公告正文。
///
/// CGI 下发的是给手Q H5 用的富文本片段，需要三步才能变成可直接显示的纯文本：
/// 1. 解一层 HTML 实体：换行写作 `&#10;`，空格写作 `&nbsp;`；
/// 2. 统一换行：正文里会出现 `\r` 紧跟 `&#10;` 的组合，必须先解开实体，
///    才能把它识别成一个 CRLF 而不是两个换行；
/// 3. 剥离 C0 控制字符，去掉 QQ 包裹链接用的 U+0001 / U+0002。
pub fn decode_announcement_text(value: &str) -> String {
    let decoded = decode_entities(value);
    let normalized = decoded.replace("\r\n", "\n").replace('\r', "\n");
    strip_control_characters(&normalized)
}

fn parse_images(message: &JsonValue) -> Vec<GroupAnnouncementImage> {
    message["pics"]
        .as_array()
        .map(|pictures| {
            pictures
                .iter()
                .map(|picture| GroupAnnouncementImage {
                    id: json_string(&picture["id"]),
                    width: json_u32(&picture["w"]),
                    height: json_u32(&picture["h"]),
                })
                .filter(|picture| !picture.id.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// `type == 20` 是 H5 对“发给新成员”的判定（`tonew = 20 === type`）。
const ANNOUNCEMENT_TYPE_TO_NEW: i64 = 20;

fn parse_feed(feed: &JsonValue, force_to_new: bool) -> GroupAnnouncement {
    let message = &feed["msg"];
    // `text` 是纯文本正文，`text_face` 额外带表情占位；后者缺失时才回退。
    let text = match json_string(&message["text"]) {
        text if text.is_empty() => json_string(&message["text_face"]),
        text => text,
    };
    GroupAnnouncement {
        fid: json_string(&feed["fid"]),
        sender_id: json_i64(&feed["u"]).unwrap_or_default(),
        publish_time: json_i64(&feed["pubt"]).unwrap_or_default(),
        title: decode_announcement_text(&json_string(&message["title"])),
        text: decode_announcement_text(&text),
        images: parse_images(message),
        read_count: json_i64(&feed["read_num"]),
        confirm_required: json_i64(&feed["settings"]["confirm_required"]).unwrap_or_default() != 0,
        show_edit_card: json_i64(&feed["settings"]["is_show_edit_card"]).unwrap_or_default() != 0,
        tip_window_type: json_i64(&feed["settings"]["tip_window_type"]).unwrap_or_default(),
        pinned: json_i64(&feed["pinned"]).unwrap_or_default() == 1,
        to_new: force_to_new
            || json_i64(&feed["type"]).unwrap_or_default() == ANNOUNCEMENT_TYPE_TO_NEW,
        raw: feed.clone(),
    }
}

/// 解析公告列表响应。
///
/// CGI 恒定返回 HTTP 200，成败由响应体的 `ec` 决定；`ec != 0` 时 `em` 是
/// 可直接展示的中文原因（例如未登录、无权限）。缺少 `feeds` 视为该群没有公告。
pub fn parse_announcement_list(value: &JsonValue) -> Result<Vec<GroupAnnouncement>, String> {
    let code = json_i64(&value["ec"]).unwrap_or_default();
    if code != 0 {
        let reason = decode_announcement_text(&json_string(&value["em"]));
        return Err(if reason.trim().is_empty() {
            format!("群公告接口返回错误码 {code}")
        } else {
            format!("{reason} (ec={code})")
        });
    }

    // 响应把公告分放在两个数组里：`feeds` 是普通公告（自带 pinned 标记），
    // `inst` 是“发给新成员”的公告。只读 feeds 会同时丢掉置顶信息和新成员公告。
    let regular = value["feeds"]
        .as_array()
        .map(|feeds| {
            feeds
                .iter()
                .map(|feed| parse_feed(feed, false))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let to_new = value["inst"]
        .as_array()
        .map(|feeds| {
            feeds
                .iter()
                .map(|feed| parse_feed(feed, true))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(to_new.into_iter().chain(regular).collect())
}

/// 公告草稿：新建与编辑共用同一份数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupAnnouncementDraft {
    /// 为空表示新建；带上已有公告的 fid 即为编辑该条公告。
    pub fid: Option<String>,
    pub text: String,
    /// 是否置顶。
    pub pinned: bool,
    /// 是否作为「发给新成员」的公告发布。
    pub to_new: bool,
    /// 是否要求群成员确认收到。
    pub confirm_required: bool,
    /// 是否允许成员在公告里直接改群名片。
    pub show_edit_card: bool,
    /// 提醒方式：0 = 弹窗提醒，1 = 仅发到群里。
    pub tip_window_type: i64,
}

impl Default for GroupAnnouncementDraft {
    /// 新建公告的默认值，与手Q H5 编辑器的初始状态一致：
    /// 默认要求确认收到、只发到群里而不弹窗。
    fn default() -> Self {
        Self {
            fid: None,
            text: String::new(),
            pinned: false,
            to_new: false,
            confirm_required: true,
            show_edit_card: false,
            tip_window_type: 1,
        }
    }
}

impl GroupAnnouncementDraft {
    /// 由已有公告构造编辑草稿。
    pub fn from_announcement(announcement: &GroupAnnouncement) -> Self {
        Self {
            fid: (!announcement.fid.is_empty()).then(|| announcement.fid.clone()),
            text: announcement.text.clone(),
            pinned: announcement.pinned,
            to_new: announcement.to_new,
            confirm_required: announcement.confirm_required,
            show_edit_card: announcement.show_edit_card,
            tip_window_type: announcement.tip_window_type,
        }
    }

    pub fn is_edit(&self) -> bool {
        self.fid.is_some()
    }
}

/// 发布/编辑公告的请求地址。
pub fn announcement_publish_url(to_new: bool) -> &'static str {
    if to_new {
        ANNOUNCEMENT_ADD_INSTRUCTION_ENDPOINT
    } else {
        ANNOUNCEMENT_ADD_ENDPOINT
    }
}

/// 规整待发布的正文。
///
/// 只统一换行符：手Q H5 额外会把连续空格与连续空行各自压成一个，
/// 那是纯展示层的取舍，会悄悄改掉用户排版（比如缩进和空行分段），这里不跟随。
pub fn normalize_draft_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// 拼接发布/编辑公告的表单请求体。
pub fn announcement_publish_form(
    bkn: i64,
    group_id: i64,
    draft: &GroupAnnouncementDraft,
) -> String {
    let settings = format!(
        r#"{{"is_show_edit_card":{},"tip_window_type":{},"confirm_required":{}}}"#,
        i32::from(draft.show_edit_card),
        draft.tip_window_type,
        i32::from(draft.confirm_required),
    );
    // type 与入口一起决定公告种类：普通公告为 1，发给新成员为 20。
    let announcement_type = if draft.to_new {
        ANNOUNCEMENT_TYPE_TO_NEW
    } else {
        1
    };
    let mut form = format!(
        "qid={group_id}&bkn={bkn}&text={}&pinned={}&type={announcement_type}&settings={}",
        urlencoding::encode(&normalize_draft_text(&draft.text)),
        i32::from(draft.pinned),
        urlencoding::encode(&settings),
    );
    if let Some(fid) = draft.fid.as_deref().filter(|fid| !fid.is_empty()) {
        form.push_str(&format!("&fid={}", urlencoding::encode(fid)));
    }
    form
}

/// 删除公告的请求地址。
pub fn announcement_delete_url() -> &'static str {
    ANNOUNCEMENT_DELETE_ENDPOINT
}

/// 拼接删除公告的表单请求体。
pub fn announcement_delete_form(bkn: i64, group_id: i64, fid: &str) -> String {
    format!("qid={group_id}&bkn={bkn}&fid={}", urlencoding::encode(fid))
}

/// 解析发布/编辑/删除的响应。
///
/// 与列表接口一样恒返回 HTTP 200，成败只看响应体的 `ec`。
pub fn parse_announcement_action(value: &JsonValue) -> Result<(), String> {
    let code = json_i64(&value["ec"]).unwrap_or_default();
    if code == 0 {
        return Ok(());
    }
    let reason = decode_announcement_text(&json_string(&value["em"]));
    Err(if reason.trim().is_empty() {
        format!("群公告接口返回错误码 {code}")
    } else {
        format!("{reason} (ec={code})")
    })
}
