#[derive(Clone, Copy)]
pub(super) struct RelationTheme {
    pub(super) page_bg: egui::Color32,
    pub(super) surface: egui::Color32,
    pub(super) surface_alt: egui::Color32,
    pub(super) canvas: egui::Color32,
    pub(super) border: egui::Color32,
    pub(super) text: egui::Color32,
    pub(super) muted: egui::Color32,
    pub(super) subtle: egui::Color32,
    pub(super) button_fill: egui::Color32,
    pub(super) button_border: egui::Color32,
    pub(super) control_text: egui::Color32,
    pub(super) grid: egui::Color32,
    pub(super) edge: egui::Color32,
    pub(super) edge_hover: egui::Color32,
    pub(super) canvas_text: egui::Color32,
    pub(super) canvas_hint: egui::Color32,
    pub(super) overlay_fill: egui::Color32,
    pub(super) overlay_border: egui::Color32,
    pub(super) shadow: egui::Color32,
    pub(super) success_fill: egui::Color32,
    pub(super) success_text: egui::Color32,
    pub(super) warning: egui::Color32,
    pub(super) node_outline: egui::Color32,
}

impl RelationTheme {
    pub(super) fn from_ui(ui: &egui::Ui) -> Self {
        Self::from_visuals(ui.visuals())
    }

    pub(super) fn from_visuals(visuals: &egui::Visuals) -> Self {
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
