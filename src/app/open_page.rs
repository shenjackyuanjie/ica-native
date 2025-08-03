/// 用来存 "打开了" 的页面
#[derive(Default)]
pub struct AppOpenPage {
    /// 验证消息页面
    pub verify_message: bool,
    /// 关于页面
    pub about: bool,
    /// 设置页面
    pub settings: bool,
    /// 通知等级说明页面
    pub notify_level: bool,
    /// 定制聊天界面
    pub custom_chat: bool,
    /// 在线状态
    pub online_status: bool,
}
