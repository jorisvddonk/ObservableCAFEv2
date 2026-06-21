mod api;
mod app;
mod config;
mod input;
mod ui;

use anyhow::Result;
use api::ApiClient;
use app::App;
use cafe_types::ContentType;
use config::Config;
use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use input::InputAction;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::from_args();
    let client = Arc::new(ApiClient::new(config.url.clone(), config.token.clone()));

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, client).await;

    // Always restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    client: Arc<ApiClient>,
) -> Result<()> {
    let mut app = App::new();

    // Load initial session list
    match client.list_sessions().await {
        Ok(sessions) => {
            app.sessions = sessions;
            if !app.sessions.is_empty() {
                load_history(&mut app, &client).await;
            }
        }
        Err(e) => {
            app.set_status(format!("Failed to connect: {}", e));
        }
    }

    // Channel for incoming chunks from streaming
    let (chunk_tx, mut chunk_rx) = mpsc::channel::<cafe_types::Chunk>(256);

    loop {
        terminal.draw(|f| ui::draw(f, &app))?;

        // Drain incoming chunks from background streaming task
        while let Ok(chunk) = chunk_rx.try_recv() {
            let is_complete = chunk
                .get_annotation::<bool>("chat.stream_complete")
                .unwrap_or(false);
            if chunk.content_type != ContentType::Null || is_complete {
                app.push_message(chunk);
            }
            if is_complete {
                app.streaming = false;
            }
        }

        // Poll for keyboard events (non-blocking, 50ms timeout)
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                let action = input::handle_key(&mut app, key);

                match action {
                    InputAction::Quit => break,

                    InputAction::SendMessage(msg) => {
                        if app.active_session_id().is_none() {
                            app.set_status("No active session. Use /new to create one.");
                            continue;
                        }
                        let session_id = app.active_session_id().unwrap().to_string();

                        // Add user message to display immediately
                        let user_chunk =
                            cafe_types::Chunk::new_text(msg.clone(), "com.nominal.cafe-tui")
                                .with_annotation("chat.role", "user");
                        app.push_message(user_chunk);
                        app.streaming = true;
                        app.clear_status();

                        let client2 = client.clone();
                        let tx = chunk_tx.clone();
                        tokio::spawn(async move {
                            if let Err(e) = client2.stream_chat(&session_id, &msg, tx).await {
                                tracing::error!("stream_chat error: {}", e);
                            }
                        });
                    }

                    InputAction::CreateSession => {
                        match client.create_session("default").await {
                            Ok(id) => {
                                // Refresh session list
                                if let Ok(sessions) = client.list_sessions().await {
                                    app.sessions = sessions;
                                    // Switch to new session
                                    if let Some(idx) =
                                        app.sessions.iter().position(|s| s.session_id == id)
                                    {
                                        app.active_session_idx = idx;
                                        app.messages.clear();
                                    }
                                }
                                app.set_status(format!("Created session {}", id));
                            }
                            Err(e) => app.set_status(format!("Failed to create session: {}", e)),
                        }
                    }

                    InputAction::DeleteSession => {
                        if let Some(id) = app.active_session_id().map(String::from) {
                            match client.delete_session(&id).await {
                                Ok(()) => {
                                    if let Ok(sessions) = client.list_sessions().await {
                                        app.sessions = sessions;
                                        app.active_session_idx = 0;
                                        app.messages.clear();
                                        if !app.sessions.is_empty() {
                                            load_history(&mut app, &client).await;
                                        }
                                    }
                                    app.set_status("Session deleted.");
                                }
                                Err(e) => {
                                    app.set_status(format!("Failed to delete: {}", e))
                                }
                            }
                        }
                    }

                    InputAction::SwitchSession(idx) => {
                        app.active_session_idx = idx;
                        app.messages.clear();
                        load_history(&mut app, &client).await;
                    }

                    InputAction::SetSystemPrompt(prompt) => {
                        if let Some(id) = app.active_session_id().map(String::from) {
                            match client.set_system_prompt(&id, &prompt).await {
                                Ok(()) => app.set_status("System prompt updated."),
                                Err(e) => {
                                    app.set_status(format!("Failed to set prompt: {}", e))
                                }
                            }
                        }
                    }

                    InputAction::SetModel(model) => {
                        if let Some(id) = app.active_session_id().map(String::from) {
                            match client.set_model(&id, &model).await {
                                Ok(()) => app.set_status(format!("Model set to {}", model)),
                                Err(e) => {
                                    app.set_status(format!("Failed to set model: {}", e))
                                }
                            }
                        }
                    }

                    InputAction::ListModels => {
                        match client.list_models().await {
                            Ok(models) => {
                                let text = if models.is_empty() {
                                    "Usage: /model <name> — no models listed by server".to_string()
                                } else {
                                    format!(
                                        "Usage: /model <name>\nAvailable models:\n{}",
                                        models.join("\n")
                                    )
                                };
                                let chunk = cafe_types::Chunk::new_text(text, "com.nominal.cafe-tui")
                                    .with_annotation("chat.role", "system");
                                app.push_message(chunk);
                            }
                            Err(e) => {
                                app.set_status(format!("Usage: /model <name> (failed to list: {})", e))
                            }
                        }
                    }

                    InputAction::Help => {
                        let help_text = "Commands:\n  /sessions  - Browse sessions\n  /new       - Create new session\n  /delete    - Delete current session\n  /rename    - Rename current session\n  /system    - Set system prompt\n  /model     - Set LLM model\n  /clear     - Clear messages\n  /help      - Show this help\n  /quit      - Exit";
                        let help_chunk = cafe_types::Chunk::new_text(help_text, "com.nominal.cafe-tui")
                            .with_annotation("chat.role", "system");
                        app.push_message(help_chunk);
                    }

                    InputAction::RenameSession(_name) => {
                        app.set_status("Rename not yet implemented.");
                    }

                    InputAction::None => {}
                }
            }
        }
    }

    Ok(())
}

async fn load_history(app: &mut App, client: &ApiClient) {
    if let Some(id) = app.active_session_id().map(String::from) {
        match client.get_history(&id).await {
            Ok(chunks) => {
                app.messages = chunks
                    .into_iter()
                    .filter(|c| {
                        c.content_type == ContentType::Text
                            && (c.role() == Some("user") || c.role() == Some("assistant"))
                    })
                    .collect();
            }
            Err(e) => app.set_status(format!("Failed to load history: {}", e)),
        }
    }
}
