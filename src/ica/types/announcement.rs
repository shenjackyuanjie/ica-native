//! QQ 群公告（只读）的 Web CGI 协议层。
//!
//! Icalingua++ 并没有实现公告协议，它只是用 Electron 打开手Q 的 H5 页
//! （见 `icalingua/src/main/utils/groupWebApps.ts` 的 `openGroupAnnouncements`）。
//! ica-native 没有 WebView，因此这里直接复刻 oicq 使用的公告列表接口：
//! `https://web.qun.qq.com/cgi-bin/announce/get_t_list`，用 bridge 下发的
//! `qun.qq.com` Cookie 与 bkn 鉴权。

use serde_json::Value as JsonValue;

/// 公告列表 CGI。
///
/// 这里用的是手Q 群公告 H5 当前在用的 `list_announce`，而不是 oicq 里那个更老的
/// `get_t_list`：后者只返回 `feeds`，既没有置顶标记，也不返回“发给新成员”的公告。
const ANNOUNCEMENT_LIST_ENDPOINT: &str = "https://web.qun.qq.com/cgi-bin/announce/list_announce";

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
/// 参数取自 H5 自身的调用：`ft=23` 固定，`s=-1` 表示从最新一条开始，
/// `i=1` 是关键——只有带上它，响应里才会出现 `inst`（发给新成员的公告）。
pub fn announcement_list_form(bkn: i64, group_id: i64) -> String {
    format!("qid={group_id}&bkn={bkn}&ft=23&s=-1&n={ANNOUNCEMENT_PAGE_SIZE}&i=1")
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
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        GroupAnnouncement, GroupAnnouncementImage, bkn_from_skey, decode_announcement_text,
        parse_announcement_list, resolve_bkn, skey_from_cookie,
    };

    /// 线上真实公告的响应片段，用来锁住正文与配图的还原结果。
    fn real_world_feed() -> GroupAnnouncement {
        parse_announcement_list(&json!({
            "ec": 0,
            "feeds": [{
                "cn": 0,
                "fid": "c4957b3500000000a4403f61499b0800",
                "fn": 0,
                "is_all_confirm": 0,
                "is_read": 0,
                "msg": {
                    "pics": [{
                        "h": "654",
                        "id": "WtanI6jP7DxNPpicIQwCC0bYR9dVHsiaBQGfQPTPo0nwY",
                        "w": "1086"
                    }],
                    "text": "HWS的新网页地图上线啦！&#10;HWS.shenjack.top:5400&#10;欢迎来当云监工！&#10;\u{1}https://kaihei.co/pdkQBI\u{2}\r&#10;开黑啦的服务器链接（",
                    "text_face": "HWS的新网页地图上线啦！&#10;HWS.shenjack.top:5400&#10;欢迎来当云监工！&#10;\u{1}https://kaihei.co/pdkQBI\u{2}\r&#10;开黑啦的服务器链接（",
                    "title": "群公告"
                },
                "pubt": 1631535268,
                "read_num": 67,
                "settings": {
                    "confirm_required": 0,
                    "is_show_edit_card": 0,
                    "remind_ts": 0,
                    "tip_window_type": 0
                },
                "type": 6,
                "u": 3695888,
                "vn": 0
            }]
        }))
        .expect("真实样本应解析成功")
        .remove(0)
    }

    #[test]
    fn real_feed_text_keeps_crlf_written_as_cr_plus_entity_as_a_single_break() {
        let feed = real_world_feed();

        // 正文换行写作 &#10;，其中一处前面还带一个裸 \r。实体必须先解开，
        // 才能把 "\r" + "&#10;" 认成一个 CRLF；先规整换行会多出一个空行。
        assert_eq!(
            feed.text,
            "HWS的新网页地图上线啦！\nHWS.shenjack.top:5400\n欢迎来当云监工！\nhttps://kaihei.co/pdkQBI\n开黑啦的服务器链接（"
        );
        assert_eq!(feed.text.lines().count(), 5);

        // QQ 用 U+0001 / U+0002 包住正文里的链接，桌面端必须剥掉，否则渲染成豆腐块。
        assert!(
            !feed
                .text
                .chars()
                .any(|ch| ch.is_control() && ch != '\n' && ch != '\t'),
            "正文不应残留换行以外的控制字符"
        );

        assert_eq!(feed.sender_id, 3695888);
        assert_eq!(feed.read_count, Some(67));
        assert!(!feed.confirm_required);
        assert!(!feed.pinned);
    }

    #[test]
    fn image_urls_follow_the_h5_gdynamic_template() {
        let feed = real_world_feed();
        let [image] = feed.images.as_slice() else {
            panic!("真实样本应有且仅有一张配图");
        };

        // 与手Q H5 的 "//gdynamic.qpic.cn/gdynamic/{id}/628" 一致，且与群号、公告 id 无关。
        assert_eq!(
            image.thumbnail_url(),
            "https://gdynamic.qpic.cn/gdynamic/WtanI6jP7DxNPpicIQwCC0bYR9dVHsiaBQGfQPTPo0nwY/628"
        );
        assert_eq!(
            image.original_url(),
            "https://gdynamic.qpic.cn/gdynamic/WtanI6jP7DxNPpicIQwCC0bYR9dVHsiaBQGfQPTPo0nwY/0"
        );

        // 1086x654 的图按 300 宽展示时的占位高度。
        let height = image.display_height(300.0).expect("宽高齐全应能算出高度");
        assert!((height - 300.0 * 654.0 / 1086.0).abs() < f32::EPSILON);
    }

    #[test]
    fn images_without_usable_size_fall_back_to_the_caller_default() {
        let missing = GroupAnnouncementImage {
            id: "x".to_string(),
            width: None,
            height: Some(10),
        };
        assert_eq!(missing.display_height(100.0), None);

        // 宽度为 0 会让比例计算除零，必须挡在返回值里。
        let zero_width = GroupAnnouncementImage {
            id: "x".to_string(),
            width: Some(0),
            height: Some(10),
        };
        assert_eq!(zero_width.display_height(100.0), None);
    }

    #[test]
    fn pinned_and_new_member_announcements_come_from_separate_response_arrays() {
        // 老接口 get_t_list 只返回 feeds，既没有 pinned 也没有 inst；
        // 换成 list_announce 之后，置顶靠 feeds[].pinned，发给新成员的公告在 inst 里。
        let announcements = parse_announcement_list(&json!({
            "ec": 0,
            "feeds": [
                { "fid": "普通", "type": 6, "msg": { "text": "普通公告" } },
                { "fid": "置顶", "type": 6, "pinned": 1, "msg": { "text": "置顶公告" } }
            ],
            "inst": [
                { "fid": "新成员", "type": 20, "msg": { "text": "欢迎新同学" } }
            ]
        }))
        .expect("应解析成功");

        let flags = announcements
            .iter()
            .map(|item| (item.fid.as_str(), item.pinned, item.to_new))
            .collect::<Vec<_>>();
        assert_eq!(
            flags,
            [
                ("新成员", false, true),
                ("普通", false, false),
                ("置顶", true, false)
            ]
        );
    }

    #[test]
    fn type_twenty_marks_new_member_announcement_even_inside_feeds() {
        // H5 的判定是 tonew = (type === 20)，即便这条公告出现在 feeds 里也算。
        let announcements = parse_announcement_list(&json!({
            "ec": 0,
            "feeds": [{ "fid": "1", "type": 20, "msg": { "text": "x" } }]
        }))
        .expect("应解析成功");
        assert!(announcements[0].to_new);
    }

    #[test]
    fn real_pinned_feed_text_restores_nbsp_and_entity_newlines() {
        // 线上真实公告：换行写作 &#10;，空格写作 &nbsp;，正文里还带 ~~ 之类的普通字符。
        let announcements = parse_announcement_list(&json!({
            "ec": 0,
            "feeds": [{
                "fid": "c4957b3500000000732ede69fab40800",
                "type": 6,
                "pubt": 1776168563,
                "read_num": 38,
                "u": 3695888,
                "msg": {
                    "title": "群公告",
                    "text": "~~高考结束啦~~&#10;大学生活开始了.png&#10;flag2:&nbsp;大家记得监督&nbsp;msdn"
                }
            }]
        }))
        .expect("应解析成功");

        let feed = &announcements[0];
        assert_eq!(
            feed.text,
            "~~高考结束啦~~\n大学生活开始了.png\nflag2: 大家记得监督 msdn"
        );
        assert_eq!(feed.text.lines().count(), 3);
        assert_eq!(feed.read_count, Some(38));
    }

    #[test]
    fn bkn_derivation_keeps_oicq_32_bit_truncation_semantics() {
        // 参考值由 oicq `Client.bkn` 的原始 JS 实现算出。
        // 前两例还没溢出 32 位，用于确认基础累加；后两例的累加器已经越过
        // 2^31，只有同时复刻「移位前 ToInt32」和「结束时掩码 31 位」才能对上，
        // 直接用 i32 或 i64 累加都会得到不同结果。
        assert_eq!(bkn_from_skey(""), 5381);
        assert_eq!(bkn_from_skey("a"), 177670);
        assert_eq!(bkn_from_skey("AbCd1234ef"), 820830020);
        assert_eq!(bkn_from_skey("0123456789abcdefghijklmnop"), 121230810);
    }

    #[test]
    fn skey_lookup_ignores_p_skey_with_the_same_suffix() {
        // QQ 的 Cookie 同时带 skey 和 p_skey，子串匹配会取错值并算出错误 bkn。
        let cookie = "uin=o10001; p_uin=o10001; p_skey=WRONGVALUE; skey=RIGHTVALUE;";
        assert_eq!(skey_from_cookie(cookie), Some("RIGHTVALUE"));
        assert_eq!(skey_from_cookie("uin=o10001; p_skey=WRONGVALUE;"), None);
    }

    #[test]
    fn bkn_falls_back_to_cookie_only_when_online_data_has_none() {
        let cookie = "uin=o10001; skey=AbCd1234ef;";
        assert_eq!(resolve_bkn(1234, cookie), Some(1234));
        assert_eq!(resolve_bkn(0, cookie), Some(820830020));
        assert_eq!(resolve_bkn(-1, cookie), Some(820830020));
        assert_eq!(resolve_bkn(0, "uin=o10001;"), None);
        assert_eq!(resolve_bkn(0, "uin=o10001; skey=;"), None);
    }

    #[test]
    fn non_zero_error_code_becomes_error_even_though_http_status_is_ok() {
        // CGI 恒返回 HTTP 200，只有响应体的 ec 能区分成败。
        let error = parse_announcement_list(&json!({ "ec": 22, "em": "no&nbsp;privilege" }))
            .expect_err("ec 非 0 必须视为失败");
        assert!(error.contains("no privilege"), "应展示 em: {error}");
        assert!(error.contains("22"), "应保留错误码: {error}");

        let fallback = parse_announcement_list(&json!({ "ec": 7 })).expect_err("ec 非 0 必须失败");
        assert!(fallback.contains('7'));

        // 没有 feeds 表示该群没有公告，不是错误。
        assert_eq!(parse_announcement_list(&json!({ "ec": 0 })), Ok(Vec::new()));
    }

    #[test]
    fn feed_parsing_tolerates_string_numbers_and_missing_fields() {
        // CGI 会把部分数值字段以字符串下发，缺字段的历史公告也要能列出来。
        let announcements = parse_announcement_list(&json!({
            "ec": 0,
            "feeds": [
                {
                    "fid": "abc123",
                    "u": "10001",
                    "pubt": 1600000000,
                    "read_num": "12",
                    "pinned": 1,
                    "settings": { "confirm_required": 1 },
                    "msg": {
                        "title": "标&amp;题",
                        "text": "第一行&#10;第二行&nbsp;结束",
                        "pics": [{ "id": "pic-1", "w": "800", "h": "600" }, { "w": "1", "h": "1" }]
                    }
                },
                { "fid": "onlyfid" }
            ]
        }))
        .expect("ec 为 0 应解析成功");

        assert_eq!(announcements.len(), 2);
        let first = &announcements[0];
        assert_eq!(first.sender_id, 10001);
        assert_eq!(first.read_count, Some(12));
        assert!(first.confirm_required);
        assert!(first.pinned);
        assert_eq!(first.title, "标&题");
        assert_eq!(first.text, "第一行\n第二行 结束");
        // 缺 id 的配图无法定位，直接丢弃。
        assert_eq!(
            first.images,
            vec![GroupAnnouncementImage {
                id: "pic-1".to_string(),
                width: Some(800),
                height: Some(600),
            }]
        );

        let second = &announcements[1];
        assert_eq!(second.fid, "onlyfid");
        assert_eq!(second.publish_time, 0);
        assert_eq!(second.read_count, None);
        assert!(!second.confirm_required);
        assert!(!second.pinned);
        assert!(second.text.is_empty());
    }

    #[test]
    fn text_decoding_unescapes_one_layer_and_keeps_unknown_entities() {
        // 手Q H5 的 decodeText 会先把 &amp; 换成 &，导致 &amp;#10; 被二次解释成换行；
        // 单次扫描保证用户原文里的 &#10; 字面量原样保留。
        assert_eq!(decode_announcement_text("a&amp;amp;b"), "a&amp;b");
        assert_eq!(decode_announcement_text("a&amp;#10;b"), "a&#10;b");
        assert_eq!(decode_announcement_text("&unknown; tail"), "&unknown; tail");
        assert_eq!(
            decode_announcement_text("no semicolon &amp"),
            "no semicolon &amp"
        );
        assert_eq!(decode_announcement_text("&#65;&#x42;&#39;"), "AB'");
    }

    #[test]
    fn text_face_is_only_used_when_plain_text_is_absent() {
        let announcements = parse_announcement_list(&json!({
            "ec": 0,
            "feeds": [
                { "fid": "1", "msg": { "text": "纯文本", "text_face": "带表情" } },
                { "fid": "2", "msg": { "text": "", "text_face": "只有 text_face" } }
            ]
        }))
        .expect("应解析成功");
        assert_eq!(announcements[0].text, "纯文本");
        assert_eq!(announcements[1].text, "只有 text_face");
    }
}
