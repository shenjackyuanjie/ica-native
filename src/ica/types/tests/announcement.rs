use super::{
    GroupAnnouncement, GroupAnnouncementImage, announcement_list_form, bkn_from_skey,
    decode_announcement_text, parse_announcement_list, resolve_bkn, skey_from_cookie,
};
use serde_json::json;

#[test]
fn first_page_request_asks_for_new_member_announcements() {
    // 手Q首屏除了 i=1 还会带 ni=1；漏掉后服务端只返回 feeds，
    // 即便群内设有发给新成员的公告，响应也没有 inst 可供展示。
    assert_eq!(
        announcement_list_form(5381, 123),
        "qid=123&bkn=5381&ft=23&s=-1&n=20&i=1&ni=1"
    );
}

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
fn publish_form_encodes_text_settings_and_picks_endpoint_by_kind() {
    use super::{
        GroupAnnouncementDraft, announcement_delete_form, announcement_publish_form,
        announcement_publish_url, parse_announcement_action,
    };

    let draft = GroupAnnouncementDraft {
        text: "第一行\r\n第二行&符".to_string(),
        pinned: true,
        to_new: false,
        confirm_required: true,
        show_edit_card: false,
        tip_window_type: 1,
        fid: None,
    };

    // 普通公告走 add_qun_notice，换行统一成 %0A，正文里的 & 要转义成 %26。
    assert!(announcement_publish_url(false).ends_with("add_qun_notice"));
    let form = announcement_publish_form(5381, -123, &draft);
    assert!(form.contains("qid=-123"));
    assert!(form.contains("bkn=5381"));
    assert!(form.contains("pinned=1"));
    assert!(form.contains("type=1"));
    assert!(
        form.contains("text=%E7%AC%AC%E4%B8%80%E8%A1%8C%0A%E7%AC%AC%E4%BA%8C%E8%A1%8C%26%E7%AC%A6")
    );
    // settings 里的布尔以 1/0 编码（i32::from(bool)），与手Q 一致。
    assert!(form.contains("confirm_required%22%3A1"));
    assert!(!form.contains("fid="), "新建公告不带 fid");

    // 发给新成员走 add_qun_instruction，type 变成 20。
    let mut instruction = draft.clone();
    instruction.to_new = true;
    instruction.fid = Some("abc123".to_string());
    assert!(announcement_publish_url(true).ends_with("add_qun_instruction"));
    let form = announcement_publish_form(5381, -123, &instruction);
    assert!(form.contains("type=20"));
    assert!(form.contains("fid=abc123"), "编辑已有公告要带 fid");

    // 删除表单只要 qid/bkn/fid。
    assert_eq!(
        announcement_delete_form(5381, -123, "abc123"),
        "qid=-123&bkn=5381&fid=abc123"
    );

    // 写操作的成败同样只看 ec。
    assert_eq!(parse_announcement_action(&json!({ "ec": 0 })), Ok(()));
    let err = parse_announcement_action(&json!({ "ec": 22, "em": "no&nbsp;privilege" }))
        .expect_err("ec 非 0 应失败");
    assert!(err.contains("no privilege") && err.contains("22"));
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
