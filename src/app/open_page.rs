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
    /// 定制聊天界面 (ica)
    pub custom_chat_ica: bool,
    /// 定制聊天界面 (extra)
    pub custom_chat_extra: bool,
    /// 在线状态
    pub online_status: bool,
    /// Socket.IO 状态
    pub socketio_status: bool,
    /// 原始的配置文件
    pub raw_config: bool,
    /// 聊天分组编辑器
    pub chat_group_editor: bool,
    /// 好友与群联系人
    pub contacts: bool,
    /// 群/成员管理工具
    pub group_tools: bool,
    /// 账号/登录设备工具
    pub account_tools: bool,
    /// 文件/资源工具
    pub file_tools: bool,
    /// 消息检索/历史工具
    pub message_tools: bool,
    /// 会话设置工具
    pub room_tools: bool,
    /// 全群自动签到
    pub auto_sign: bool,
    /// QQ 关系网分析
    pub relation_network: bool,
}
