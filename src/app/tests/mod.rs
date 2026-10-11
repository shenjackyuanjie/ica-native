use super::IcaApp;

#[test]
fn bundled_fonts_surround_system_fallbacks() {
    let ctx = egui::Context::default();
    IcaApp::setup_fonts(&ctx);
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        ui.fonts(|fonts| {
            let definitions = fonts.definitions();
            let proportional = &definitions.families[&egui::FontFamily::Proportional];
            assert_eq!(proportional.first().map(String::as_str), Some("notosans"));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                assert_eq!(
                    definitions.families[&family].last().map(String::as_str),
                    Some("unifont")
                );
            }
        });
    });
    output.textures_delta.clear();
}

#[test]
#[ignore = "需要系统安装中文字体和含思考表情轮廓的 emoji 字体；字体方案变更时在桌面环境显式运行"]
fn system_fonts_render_chinese_and_thinking_face() {
    let ctx = egui::Context::default();
    IcaApp::setup_fonts(&ctx);
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        ui.fonts_mut(|fonts| {
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                let font_id = egui::FontId::new(18.0, family);
                // egui 0.36 的 has_glyph 按字体身份判断是否命中替代字形，
                // 会误判与替代字形同属一套字体的正常字符。直接比较实际网格 UV。
                let missing = fonts.layout_no_wrap(
                    "\u{10ffff}".into(),
                    font_id.clone(),
                    egui::Color32::WHITE,
                );
                let missing_uv: Vec<_> = missing
                    .rows
                    .iter()
                    .flat_map(|row| row.visuals.mesh.vertices.iter().map(|vertex| vertex.uv))
                    .collect();
                for character in ['中', 'A', '1', '🤔'] {
                    let galley = fonts.layout_no_wrap(
                        character.to_string(),
                        font_id.clone(),
                        egui::Color32::WHITE,
                    );
                    let glyph_uv: Vec<_> = galley
                        .rows
                        .iter()
                        .flat_map(|row| row.visuals.mesh.vertices.iter().map(|vertex| vertex.uv))
                        .collect();
                    assert_ne!(glyph_uv, missing_uv, "字符落到了缺字替代图形: {character}");
                    assert!(
                        galley
                            .rows
                            .iter()
                            .any(|row| !row.visuals.mesh.vertices.is_empty()),
                        "字形没有生成可绘制轮廓: {character}"
                    );
                }
            }
        });
    });
    output.textures_delta.clear();
}
