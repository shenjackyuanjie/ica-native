/// 用来存 "打开了" 的页面
pub struct AppOpenPage {
    /// 验证消息页面
    pub verify_message: bool,
    /// 关于页面
    pub about: bool,
    /// 设置页面
    pub settings: bool,
    /// 通知等级说明页面
    pub notify_level: bool,
}

impl Default for AppOpenPage {
    fn default() -> Self {
        Self {
            verify_message: false,
            about: false,
            settings: false,
            notify_level: false,
        }
    }
}
