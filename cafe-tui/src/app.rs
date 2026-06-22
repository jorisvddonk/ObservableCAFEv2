use cafe_types::{Chunk, SessionInfo};

#[derive(Debug, Clone, PartialEq)]
pub enum AppMode {
    Normal,
    SessionPicker,
    ModelPicker,
    Confirm(ConfirmAction),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmAction {
    DeleteSession,
}

pub struct App {
    pub sessions: Vec<SessionInfo>,
    pub active_session_idx: usize,
    pub messages: Vec<Chunk>,
    pub input: String,
    pub streaming: bool,
    pub scroll_offset: usize,
    pub mode: AppMode,
    pub status_msg: Option<String>,
    pub model_picker_items: Vec<String>,
    pub model_picker_all: Vec<String>,
    pub model_picker_idx: usize,
    pub model_picker_filter: String,
}

impl App {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            active_session_idx: 0,
            messages: Vec::new(),
            input: String::new(),
            streaming: false,
            scroll_offset: 0,
            mode: AppMode::Normal,
            status_msg: None,
            model_picker_items: Vec::new(),
            model_picker_all: Vec::new(),
            model_picker_idx: 0,
            model_picker_filter: String::new(),
        }
    }

    pub fn active_session(&self) -> Option<&SessionInfo> {
        self.sessions.get(self.active_session_idx)
    }

    pub fn active_session_id(&self) -> Option<&str> {
        self.active_session().map(|s| s.session_id.as_str())
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(1);
    }

    pub fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = usize::MAX;
    }

    pub fn push_message(&mut self, chunk: Chunk) {
        self.messages.push(chunk);
        if self.scroll_offset == 0 {
            // Stay at bottom
        }
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_msg = Some(msg.into());
    }

    pub fn clear_status(&mut self) {
        self.status_msg = None;
    }

    pub fn apply_model_filter(&mut self) {
        let filter = self.model_picker_filter.to_lowercase();
        self.model_picker_items = self
            .model_picker_all
            .iter()
            .filter(|m| m.to_lowercase().contains(&filter))
            .cloned()
            .collect();
        self.model_picker_idx = 0;
    }
}
