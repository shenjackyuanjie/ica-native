pub mod fonts {
    pub const FONT_思源黑体: &[u8] = include_bytes!("../assets/fonts/NotoSansCJKsc-Regular.otf");
    pub const FONT_UNIFONT: &[u8] = include_bytes!("../assets/fonts/unifont-16.0.04.otf");
}

pub mod png {
    pub const ICON_512X: &[u8] = include_bytes!("../assets/png/icon_512x512.png");
}

pub mod svg {
    pub const CHAT_GROUP: egui::ImageSource =
        egui::include_image!("../assets/svg/chat-group-icon.svg");
    pub const CHAT_MUTE: egui::ImageSource = egui::include_image!("../assets/svg/chat-mute.svg");
}

pub mod webp {
    pub const NOTIFICATION: egui::ImageSource =
        egui::include_image!("../assets/webp/notification.webp");
}
