//! 关系网窗口的配色主题。
//!
//! `RelationTheme` 集中定义关系网画布、面板、浮层与控件使用的全部颜色，
//! 均由当前 egui 主题的 `Visuals` 派生，随浅色/深色模式自动切换。视图代码
//! 不各自硬编码颜色，统一从这里取色，保证关系网窗口与整体界面风格一致。

/// 关系网界面使用的配色集合。
///
/// 所有颜色都从当前 egui 主题的 `Visuals` 派生，保证关系网窗口与整体界面风格一致。
#[derive(Clone, Copy)]
pub struct RelationTheme {
    /// 页面背景。
    pub page_bg: egui::Color32,
    /// 普通表面（面板）背景。
    pub surface: egui::Color32,
    /// 交替/弱化表面背景。
    pub surface_alt: egui::Color32,
    /// 画布背景。
    pub canvas: egui::Color32,
    /// 普通边框。
    pub border: egui::Color32,
    /// 主文本。
    pub text: egui::Color32,
    /// 次要文本。
    pub muted: egui::Color32,
    /// 更弱的提示文本。
    pub subtle: egui::Color32,
    /// 按钮填充。
    pub button_fill: egui::Color32,
    /// 按钮边框。
    pub button_border: egui::Color32,
    /// 按钮文本。
    pub control_text: egui::Color32,
    /// 画布网格线。
    pub grid: egui::Color32,
    /// 关系连线。
    pub edge: egui::Color32,
    /// 悬停高亮的连线。
    pub edge_hover: egui::Color32,
    /// 画布上的文本。
    pub canvas_text: egui::Color32,
    /// 画布上的提示文本。
    pub canvas_hint: egui::Color32,
    /// 浮层填充。
    pub overlay_fill: egui::Color32,
    /// 浮层边框。
    pub overlay_border: egui::Color32,
    /// 浮层投影。
    pub shadow: egui::Color32,
    /// 成功状态填充。
    pub success_fill: egui::Color32,
    /// 成功状态文本。
    pub success_text: egui::Color32,
    /// 警告色。
    pub warning: egui::Color32,
    /// 节点描边。
    pub node_outline: egui::Color32,
}

impl RelationTheme {
    /// 从 egui 的 `Ui` 派生一套关系网配色。
    pub fn from_ui(ui: &egui::Ui) -> Self {
        Self::from_visuals(ui.visuals())
    }

    /// 从 egui 的 `Visuals` 派生一套关系网配色。
    pub fn from_visuals(visuals: &egui::Visuals) -> Self {
        let text = visuals.text_color();
        let muted = visuals.weak_text_color();
        let inactive = &visuals.widgets.inactive;
        let noninteractive = &visuals.widgets.noninteractive;

        Self {
            page_bg: visuals.panel_fill,
            surface: visuals.panel_fill,
            surface_alt: visuals.faint_bg_color,
            canvas: visuals.panel_fill,
            border: noninteractive.bg_stroke.color,
            text,
            muted,
            subtle: muted.gamma_multiply(0.78),
            button_fill: inactive.weak_bg_fill,
            button_border: inactive.bg_stroke.color,
            control_text: inactive.fg_stroke.color,
            grid: noninteractive.bg_stroke.color.gamma_multiply(0.38),
            edge: muted,
            edge_hover: visuals.widgets.hovered.fg_stroke.color,
            canvas_text: text,
            canvas_hint: muted,
            overlay_fill: visuals.window_fill,
            overlay_border: visuals.window_stroke.color,
            shadow: visuals.window_shadow.color,
            success_fill: visuals.selection.bg_fill.gamma_multiply(0.32),
            success_text: visuals.hyperlink_color,
            warning: visuals.warn_fg_color,
            node_outline: text.gamma_multiply(if visuals.dark_mode { 0.72 } else { 0.92 }),
        }
    }
}
