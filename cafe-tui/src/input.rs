use crate::app::{App, AppMode, ConfirmAction};

pub enum InputAction {
    SendMessage(String),
    CreateSession,
    DeleteSession,
    RenameSession(String),
    SetSystemPrompt(String),
    SetModel(String),
    ListModels,
    SwitchSession(usize),
    Quit,
    Help,
    None,
}

pub fn handle_key(app: &mut App, key: crossterm::event::KeyEvent) -> InputAction {
    match &app.mode {
        AppMode::SessionPicker => handle_session_picker(app, key),
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
        _ => InputAction::None,
    }
}

fn parse_slash_command(app: &mut App, cmd: &str) -> InputAction {
    let parts: Vec<&str> = cmd.splitn(2, ' ').collect();
    match parts[0] {
        "sessions" => {
            app.mode = AppMode::SessionPicker;
            InputAction::None
        }
        "new" => InputAction::CreateSession,
        "delete" => {
            app.mode = AppMode::Confirm(ConfirmAction::DeleteSession);
            app.set_status("Delete current session? (y/n)");
            InputAction::None
        }
        "rename" => {
            let name = parts.get(1).unwrap_or(&"").trim().to_string();
            if name.is_empty() {
                app.set_status("Usage: /rename <name>");
                InputAction::None
            } else {
                InputAction::RenameSession(name)
            }
        }
        "system" => {
            let prompt = parts.get(1).unwrap_or(&"").trim().to_string();
            if prompt.is_empty() {
                app.set_status("Usage: /system <prompt>");
                InputAction::None
            } else {
                InputAction::SetSystemPrompt(prompt)
            }
        }
        "model" => {
            let model = parts.get(1).unwrap_or(&"").trim().to_string();
            if model.is_empty() {
                InputAction::ListModels
            } else {
                InputAction::SetModel(model)
            }
        }
        "clear" => {
            app.messages.clear();
            InputAction::None
        }
        "help" => InputAction::Help,
        "quit" | "exit" => InputAction::Quit,
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
