//! QQ 群公告（只读）的 Web CGI 协议层。
//!
//! Icalingua++ 并没有实现公告协议，它只是用 Electron 打开手Q 的 H5 页
//! （见 `icalingua/src/main/utils/groupWebApps.ts` 的 `openGroupAnnouncements`）。
//! ica-native 没有 WebView，因此这里直接复刻 oicq 使用的公告列表接口：
//! `https://web.qun.qq.com/cgi-bin/announce/get_t_list`，用 bridge 下发的
//! `qun.qq.com` Cookie 与 bkn 鉴权。

use serde_json::Value as JsonValue;

/// 公告列表 CGI；参数与 oicq `getGroupNotice` 保持一致。
const ANNOUNCEMENT_LIST_ENDPOINT: &str = "https://web.qun.qq.com/cgi-bin/announce/get_t_list";

/// 拉取公告时使用的 Cookie 域名，必须在 Bridge 的允许列表内。
pub const ANNOUNCEMENT_COOKIE_DOMAIN: &str = "qun.qq.com";

/// 单次拉取的公告条数，与手Q H5 的默认分页一致。
pub const ANNOUNCEMENT_PAGE_SIZE: u32 = 20;

/// 拼接公告列表请求 URL。
pub fn announcement_list_url(bkn: i64, group_id: i64) -> String {
    format!(
        "{ANNOUNCEMENT_LIST_ENDPOINT}?bkn={bkn}&qid={group_id}&ft=23&s=-1&n={ANNOUNCEMENT_PAGE_SIZE}"
    )
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

/// 公告正文里的配图。
///
/// 目前只读取图片标识与尺寸：手Q H5 是自己拼接图片 URL 的，公开资料里没有
/// 稳定可靠的拼接规则，先如实保留原始字段，等实际响应样本确认后再补渲染。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupAnnouncementImage {
    pub id: String,
    pub width: String,
    pub height: String,
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

/// 还原公告正文。
///
/// CGI 返回的正文是 HTML 片段：换行是 `\r\n`，空格和符号被转义成 HTML 实体。
/// 这里统一成界面可直接显示的纯文本，只解一层实体，避免把用户原文二次解释。
pub fn decode_announcement_text(value: &str) -> String {
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.contains('&') {
        return normalized;
    }

    let mut decoded = String::with_capacity(normalized.len());
    let mut remaining = normalized.as_str();
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

fn parse_images(message: &JsonValue) -> Vec<GroupAnnouncementImage> {
    message["pics"]
        .as_array()
        .map(|pictures| {
            pictures
                .iter()
                .map(|picture| GroupAnnouncementImage {
                    id: json_string(&picture["id"]),
                    width: json_string(&picture["w"]),
                    height: json_string(&picture["h"]),
                })
                .filter(|picture| !picture.id.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_feed(feed: &JsonValue) -> GroupAnnouncement {
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

    Ok(value["feeds"]
        .as_array()
        .map(|feeds| feeds.iter().map(parse_feed).collect())
        .unwrap_or_default())
}
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        GroupAnnouncementImage, bkn_from_skey, decode_announcement_text, parse_announcement_list,
        resolve_bkn, skey_from_cookie,
    };

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
                    "settings": { "confirm_required": 1 },
                    "msg": {
                        "title": "标&amp;题",
                        "text": "第一行\r\n第二行&nbsp;结束",
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
        assert_eq!(first.title, "标&题");
        assert_eq!(first.text, "第一行\n第二行 结束");
        // 缺 id 的配图无法定位，直接丢弃。
        assert_eq!(
            first.images,
            vec![GroupAnnouncementImage {
                id: "pic-1".to_string(),
                width: "800".to_string(),
                height: "600".to_string(),
            }]
        );

        let second = &announcements[1];
        assert_eq!(second.fid, "onlyfid");
        assert_eq!(second.publish_time, 0);
        assert_eq!(second.read_count, None);
        assert!(!second.confirm_required);
        assert!(second.text.is_empty());
    }

    #[test]
    fn text_decoding_unescapes_one_layer_and_keeps_unknown_entities() {
        assert_eq!(decode_announcement_text("a&amp;amp;b"), "a&amp;b");
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
