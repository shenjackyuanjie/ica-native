//! 合并转发查看器状态。

use crate::ica::types::message::Message;

use super::super::media::{ImageAction, ImageSource};

#[derive(Debug, Clone, Default)]
pub struct ForwardViewerState {
    pub open: bool,
    pub res_id: String,
    pub file_name: String,
    pub fallback_res_id: Option<String>,
    pub messages: Vec<Message>,
    pub loading: bool,
    pub last_error: Option<String>,
    pub pending_action: Option<ForwardViewerAction>,
    request_id: u64,
}

#[derive(Debug, Clone)]
pub enum ForwardViewerAction {
    Reload,
    Image {
        action: ImageAction,
        sources: Vec<ImageSource>,
    },
    OpenReference {
        res_id: String,
        file_name: Option<String>,
        fallback_res_id: Option<String>,
        inline_messages: Option<Vec<Message>>,
    },
}

impl ForwardViewerState {
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

    pub fn fail(&mut self, request_id: u64, error: String) {
        if request_id != self.request_id {
            return;
        }
        self.loading = false;
        self.last_error = Some(error);
    }
}
