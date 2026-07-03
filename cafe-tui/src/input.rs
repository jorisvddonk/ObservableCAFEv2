use crate::app::{App, AppMode, ConfirmAction};

pub enum InputAction {
    SendMessage(String),
    CreateSession,
    DeleteSession,
    RenameSession(String),
    SetSystemPrompt(String),
    SetModel(String),
    ListModels,
    OpenModelPicker,
    SelectModel(String),
    OpenAgentPicker,
    SelectAgent(String),
    ListAgents,
    SwitchSession(usize),
    ToggleRaw,
    Quit,
    Help,
    None,
}

pub fn handle_key(app: &mut App, key: crossterm::event::KeyEvent) -> InputAction {
    match &app.mode {
        AppMode::SessionPicker => handle_session_picker(app, key),
        AppMode::ModelPicker => handle_model_picker(app, key),
        AppMode::AgentPicker => handle_agent_picker(app, key),
        AppMode::Confirm(action) => handle_confirm(app, key, action.clone()),
        AppMode::Normal => handle_normal(app, key),
    }
}

fn handle_normal(app: &mut App, key: crossterm::event::KeyEvent) -> InputAction {
    use crossterm::event::KeyCode;

    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            InputAction::Quit
        }
        KeyCode::Char('r') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            InputAction::ToggleRaw
        }
        KeyCode::Enter => {
            if app.streaming {
                return InputAction::None;
            }
            let input = app.input.trim().to_string();
            if input.is_empty() {
                return InputAction::None;
            }
            app.input.clear();
            app.scroll_to_bottom();

            // Parse slash commands
            if let Some(cmd) = input.strip_prefix('/') {
                parse_slash_command(app, cmd)
            } else {
                InputAction::SendMessage(input)
            }
        }
        KeyCode::Tab => {
            // Tab on "/agent <text>" opens agent picker with filter
            if let Some(rest) = app.input.strip_prefix("/agent ") {
                app.agent_picker_filter = rest.to_string();
                app.apply_agent_filter();
                InputAction::OpenAgentPicker
            } else if let Some(rest) = app.input.strip_prefix("/model ") {
                app.model_picker_filter = rest.to_string();
                InputAction::OpenModelPicker
            } else if let Some(partial) = app.input.strip_prefix('/') {
                // Tab-complete slash command name (before any space)
                if !partial.contains(' ') {
                    let commands = [
                        "sessions", "new", "delete", "rename",
                        "system", "model", "agent", "clear", "help", "quit",
                    ];
                    let matches: Vec<&&str> = commands.iter().filter(|c| c.starts_with(partial)).collect();
                    if matches.len() == 1 {
                        app.input = format!("/{} ", matches[0]);
                    }
                }
                InputAction::None
            } else {
                InputAction::None
            }
        }
        KeyCode::Backspace => {
            app.input.pop();
            InputAction::None
        }
        KeyCode::Char(c) => {
            app.input.push(c);
            InputAction::None
        }
        KeyCode::Up => {
            app.scroll_up();
            InputAction::None
        }
        KeyCode::Down => {
            app.scroll_down();
            InputAction::None
        }
        KeyCode::PageUp => {
            for _ in 0..10 {
                app.scroll_up();
            }
            InputAction::None
        }
        KeyCode::PageDown => {
            for _ in 0..10 {
                app.scroll_down();
            }
            InputAction::None
        }
        KeyCode::End => {
            app.scroll_to_bottom();
            InputAction::None
        }
        KeyCode::Home => {
            app.scroll_to_top();
            InputAction::None
        }
        _ => InputAction::None,
    }
}

fn parse_slash_command(app: &mut App, cmd: &str) -> InputAction {
    let parts: Vec<&str> = cmd.splitn(2, ' ').collect();
    match parts[0] {
        "sessions" | "s" => {
            app.mode = AppMode::SessionPicker;
            InputAction::None
        }
        "new" | "n" => InputAction::CreateSession,
        "delete" | "d" => {
            app.mode = AppMode::Confirm(ConfirmAction::DeleteSession);
            app.set_status("Delete current session? (y/n)");
            InputAction::None
        }
        "rename" | "rn" => {
            let name = parts.get(1).unwrap_or(&"").trim().to_string();
            if name.is_empty() {
                app.set_status("Usage: /rename <name>");
                InputAction::None
            } else {
                InputAction::RenameSession(name)
            }
        }
        "system" | "sy" => {
            let prompt = parts.get(1).unwrap_or(&"").trim().to_string();
            if prompt.is_empty() {
                app.set_status("Usage: /system <prompt>");
                InputAction::None
            } else {
                InputAction::SetSystemPrompt(prompt)
            }
        }
        "model" | "m" => {
            let model = parts.get(1).unwrap_or(&"").trim().to_string();
            if model.is_empty() {
                InputAction::ListModels
            } else {
                InputAction::SetModel(model)
            }
        }
        "agent" | "a" => {
            let agent = parts.get(1).unwrap_or(&"").trim().to_string();
            if agent.is_empty() {
                InputAction::ListAgents
            } else {
                InputAction::SelectAgent(agent)
            }
        }
        "clear" | "c" => {
            app.messages.clear();
            InputAction::None
        }
        "help" | "h" => InputAction::Help,
        "quit" | "q" | "exit" => InputAction::Quit,
        "/" => {
            // "//" prefix — send as literal message
            InputAction::SendMessage(format!("/{}", parts.get(1).unwrap_or(&"")))
        }
        _ => {
            app.set_status(format!("Unknown command: /{}", parts[0]));
            InputAction::None
        }
    }
}

fn handle_session_picker(app: &mut App, key: crossterm::event::KeyEvent) -> InputAction {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
            InputAction::None
        }
        KeyCode::Up => {
            if app.active_session_idx > 0 {
                app.active_session_idx -= 1;
            }
            InputAction::None
        }
        KeyCode::Down => {
            if app.active_session_idx + 1 < app.sessions.len() {
                app.active_session_idx += 1;
            }
            InputAction::None
        }
        KeyCode::Enter => {
            let idx = app.active_session_idx;
            app.mode = AppMode::Normal;
            InputAction::SwitchSession(idx)
        }
        _ => InputAction::None,
    }
}

fn handle_model_picker(app: &mut App, key: crossterm::event::KeyEvent) -> InputAction {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
            InputAction::None
        }
        KeyCode::Up => {
            if app.model_picker_idx > 0 {
                app.model_picker_idx -= 1;
            }
            InputAction::None
        }
        KeyCode::Down => {
            if app.model_picker_idx + 1 < app.model_picker_items.len() {
                app.model_picker_idx += 1;
            }
            InputAction::None
        }
        KeyCode::Enter => {
            if app.model_picker_items.is_empty() {
                app.mode = AppMode::Normal;
                return InputAction::None;
            }
            let model = app.model_picker_items[app.model_picker_idx].clone();
            app.mode = AppMode::Normal;
            app.input = format!("/model {}", model);
            InputAction::SelectModel(model)
        }
        KeyCode::Char(c) => {
            app.model_picker_filter.push(c);
            app.apply_model_filter();
            InputAction::None
        }
        KeyCode::Backspace => {
            app.model_picker_filter.pop();
            app.apply_model_filter();
            InputAction::None
        }
        _ => InputAction::None,
    }
}

fn handle_agent_picker(app: &mut App, key: crossterm::event::KeyEvent) -> InputAction {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
            InputAction::None
        }
        KeyCode::Up => {
            if app.agent_picker_idx > 0 {
                app.agent_picker_idx -= 1;
            }
            InputAction::None
        }
        KeyCode::Down => {
            if app.agent_picker_idx + 1 < app.agent_picker_items.len() {
                app.agent_picker_idx += 1;
            }
            InputAction::None
        }
        KeyCode::Enter => {
            if app.agent_picker_items.is_empty() {
                app.mode = AppMode::Normal;
                return InputAction::None;
            }
            let idx = app.agent_picker_items[app.agent_picker_idx];
            let agent_id = app.agents[idx].id.clone();
            app.mode = AppMode::Normal;
            app.input = format!("/agent {}", agent_id);
            InputAction::SelectAgent(agent_id)
        }
        KeyCode::Char(c) => {
            app.agent_picker_filter.push(c);
            app.apply_agent_filter();
            InputAction::None
        }
        KeyCode::Backspace => {
            app.agent_picker_filter.pop();
            app.apply_agent_filter();
            InputAction::None
        }
        _ => InputAction::None,
    }
}

fn handle_confirm(
    app: &mut App,
    key: crossterm::event::KeyEvent,
    action: ConfirmAction,
) -> InputAction {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.mode = AppMode::Normal;
            app.clear_status();
            match action {
                ConfirmAction::DeleteSession => InputAction::DeleteSession,
            }
        }
        _ => {
            app.mode = AppMode::Normal;
            app.clear_status();
            InputAction::None
        }
    }
}
