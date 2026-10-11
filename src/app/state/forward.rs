//! 合并转发查看器状态。
//!
//! 合并转发记录由 Bridge 按 `res_id` 拉取，查看器通过 `Arc<Mutex<..>>`
//! 在主窗口与独立查看窗口之间共享。与公告查看器相同：窗口只置位意图，
//! 真正的命令发送与响应回填由主循环完成，过期响应用 `request_id` 丢弃。

use super::super::media::{ImageAction, ImageSource};
use crate::ica::types::message::Message;

/// 合并转发查看器状态（主窗口与查看窗口通过 `Arc<Mutex<..>>` 共享）。
#[derive(Debug, Clone, Default)]
pub struct ForwardViewerState {
    /// 查看窗口是否打开。
    pub open: bool,
    /// 当前查看的合并转发资源 id。
    pub res_id: String,
    /// 当前查看的合并转发标题（文件名）。
    pub file_name: String,
    /// 拉取失败时尝试的备用资源 id。
    pub fallback_res_id: Option<String>,
    /// 已解析出的转发消息列表。
    pub messages: Vec<Message>,
    /// 是否正在拉取。
    pub loading: bool,
    /// 最近一次失败的原因。
    pub last_error: Option<String>,
    /// 窗口内产生、需要主循环执行的动作（重新拉取、查看图片等）。
    pub pending_action: Option<ForwardViewerAction>,
    /// 单调递增的请求序号，用于丢弃过期响应。
    request_id: u64,
}

/// 合并转发查看器产生、交由主循环执行的动作。
#[derive(Debug, Clone)]
pub enum ForwardViewerAction {
    /// 重新拉取当前合并转发。
    Reload,
    /// 查看/保存/复制某张图片。
    Image {
        /// 对图片执行的操作。
        action: ImageAction,
        /// 可连续浏览的图片来源列表。
        sources: Vec<ImageSource>,
    },
    /// 打开一条嵌套的合并转发引用。
    OpenReference {
        /// 被引用合并转发的资源 id。
        res_id: String,
        /// 被引用合并转发的标题。
        file_name: Option<String>,
        /// 拉取失败时的备用资源 id。
        fallback_res_id: Option<String>,
        /// 已随消息下发的内嵌内容，存在时无需再发起请求。
        inline_messages: Option<Vec<Message>>,
    },
}

impl ForwardViewerState {
    /// 开始一次拉取并返回请求序号。
    ///
    /// 会打开查看窗口、清空旧消息并进入加载态。
    pub fn begin_request(
        &mut self,
        res_id: String,
        file_name: Option<String>,
        fallback_res_id: Option<String>,
    ) -> u64 {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.open = true;
        self.res_id = res_id;
        self.file_name = file_name.unwrap_or_default();
        self.fallback_res_id = fallback_res_id;
        self.messages.clear();
        self.loading = true;
        self.last_error = None;
        self.pending_action = None;
        self.request_id
    }

    /// 回填一次成功响应；`request_id` 过期时静默丢弃。
    ///
    /// 响应可能带回真正的 `res_id`（历史数据里的占位 id 会被替换），此时同步更新。
    pub fn apply_response(
        &mut self,
        request_id: u64,
        res_id: Option<String>,
        messages: Vec<Message>,
    ) {
        if request_id != self.request_id {
            return;
        }
        if let Some(res_id) = res_id {
            self.res_id = res_id;
        }
        self.fallback_res_id = None;
        self.messages = messages;
        self.loading = false;
        self.last_error = None;
    }

    /// 直接用已随消息下发的内嵌内容打开查看器，无需发起请求。
    pub fn open_inline(
        &mut self,
        res_id: String,
        file_name: Option<String>,
        messages: Vec<Message>,
    ) {
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.open = true;
        self.res_id = res_id;
        self.file_name = file_name.unwrap_or_default();
        self.fallback_res_id = None;
        self.messages = messages;
        self.loading = false;
        self.last_error = None;
        self.pending_action = None;
    }

    /// 记录一次拉取失败；过期请求静默丢弃。
    pub fn fail(&mut self, request_id: u64, error: String) {
        if request_id != self.request_id {
            return;
        }
        self.loading = false;
        self.last_error = Some(error);
    }
}
