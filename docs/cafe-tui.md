# cafe-tui — Build Guide

**Role:** Terminal UI client. Connects to cafe-server's HTTP API (not the bus directly).
Renders chat sessions in the terminal with a ratatui-based interface.

**Build after:** `cafe-types`, `cafe-server`

---

## What it does

- Connects to a running cafe-server over HTTP
- Lists and switches between sessions
- Renders chat history with role-differentiated formatting
- Sends user messages and streams responses in real time (SSE)
- Supports slash commands: `/sessions`, `/new`, `/rename`, `/delete`, `/system`

---

## Cargo.toml dependencies to add

```toml
[dependencies]
cafe-types   = { path = "../cafe-types" }
tokio        = { workspace = true }
serde        = { workspace = true }
serde_json   = { workspace = true }
tracing      = { workspace = true }
anyhow       = { workspace = true }
ratatui      = "0.26"
crossterm    = "0.27"
reqwest      = { version = "0.12", features = ["json", "stream"] }
futures-util = "0.3"
clap         = { version = "4", features = ["derive", "env"] }
```

---

## File structure

```
cafe-tui/src/
```

Use ratatui `Paragraph`, `Block`, `List` widgets. Scroll the message area.
Highlight the current user input line. Show a spinner while `streaming = true`.

---

## SSE streaming (api.rs)

```rust
pub async fn stream_response(
    url: &str,
    token: &str,
    session_id: &str,
    message: &str,
    tx: mpsc::Sender<Chunk>,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let mut stream = client
        .post(format!("{}/api/sessions/{}/chat", url, session_id))
        .bearer_auth(token)
        .json(&json!({ "content": message }))
        .send()
        .await?
        .bytes_stream();

    let mut buffer = String::new();
    while let Some(bytes) = stream.next().await {
        let text = String::from_utf8_lossy(&bytes?);
        buffer.push_str(&text);
        // Parse SSE "data: {...}\n\n" lines
        while let Some(chunk) = try_parse_sse_chunk(&mut buffer) {
            tx.send(chunk).await?;
        }
    }
    Ok(())
}
```

---

## Slash commands

| Command                  | Action                                   |
|--------------------------|------------------------------------------|
| `/sessions`              | Open session picker overlay              |
| `/new`                   | Create new session with default agent    |
| `/rename <name>`         | Rename current session                   |
| `/delete`                | Delete current session (with confirm)    |
| `/system <prompt>`       | Set system prompt for current session    |
| `/clear`                 | Clear message view (not history)         |
| `/quit` or `/exit`       | Exit the TUI                             |

Lines starting with `//` are forwarded directly to the agent as a message.

---

## Terminal lifecycle

```rust
// main.rs
fn main() -> anyhow::Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run app
    let result = run_app(&mut terminal, app).await;

    // Restore terminal (always, even on error)
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    result
}
```

Always restore the terminal in a drop guard or `finally`-style cleanup,
otherwise the user's terminal will be left in raw mode on panic.
