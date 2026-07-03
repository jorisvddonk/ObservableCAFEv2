use cafe_sdk::http::AgentInfo;
use cafe_sdk::{Chunk, SessionInfo};

#[derive(Debug, Clone, PartialEq)]
pub enum AppMode {
    Normal,
    SessionPicker,
    ModelPicker,
    AgentPicker,
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
    pub scroll_offset: i32,
    pub mode: AppMode,
    pub status_msg: Option<String>,
    pub model_picker_items: Vec<String>,
    pub model_picker_all: Vec<String>,
    pub model_picker_idx: usize,
    pub model_picker_filter: String,
    pub raw_mode: bool,
    pub agents: Vec<AgentInfo>,
    pub agent_picker_idx: usize,
    pub agent_picker_filter: String,
    pub agent_picker_items: Vec<usize>,
    pub selected_agent_id: String,
}

impl App {
    pub fn new(agent_id: impl Into<String>) -> Self {
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
            raw_mode: false,
            agents: Vec::new(),
            agent_picker_idx: 0,
            agent_picker_filter: String::new(),
            agent_picker_items: Vec::new(),
            selected_agent_id: agent_id.into(),
        }
    }

    pub fn active_session(&self) -> Option<&SessionInfo> {
        self.sessions.get(self.active_session_idx)
    }

    pub fn active_session_id(&self) -> Option<&str> {
        self.active_session().map(|s| s.session_id.as_str())
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset += 1;
    }

    pub fn scroll_down(&mut self) {
        self.scroll_offset -= 1;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 1_000_000;
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

    pub fn apply_agent_filter(&mut self) {
        let filter = self.agent_picker_filter.to_lowercase();
        self.agent_picker_items = self
            .agents
            .iter()
            .enumerate()
            .filter(|(_, a)| a.id.to_lowercase().contains(&filter) || a.description.to_lowercase().contains(&filter))
            .map(|(i, _)| i)
            .collect();
        self.agent_picker_idx = 0;
    }
}
