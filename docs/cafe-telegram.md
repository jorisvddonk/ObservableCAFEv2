# cafe-telegram — Build Guide

**Role:** Telegram bot bridge. Translates Telegram messages into cafe-server API calls
and forwards session chunks back to users via Telegram.

**Language:** Go  
**Build after:** `cafe-server` is running

---

## What it does

- Runs a Telegram bot
- Maps Telegram users to sessions (one default session per user + named sessions)
- Forwards user messages to cafe-server
- Streams LLM responses back as Telegram messages (editing a single message for streaming)
- Supports `/sessions`, `/new`, `/join <id>`, `/share`, `/subscribe <id>`
- Delivers binary chunks (images, audio) as Telegram media messages
- Persists subscriptions across restarts (in a local SQLite file)

---

## Go dependencies

```
go get github.com/go-telegram-bot-api/telegram-bot-api/v5
go get github.com/mattn/go-sqlite3
```

Or use `modernc.org/sqlite` for a pure-Go SQLite (no CGo required):
```
go get modernc.org/sqlite
```

---

## File structure

```
cafe-telegram/
├── main.go             # init, start bot, run update loop
├── bot.go              # Telegram bot handler: route commands + messages
├── client.go           # cafe-server HTTP client (sessions, chat, stream)
├── sessions.go         # user → session mapping + subscription management
├── db.go               # SQLite: persist subscriptions
├── stream.go           # SSE consumer: read chunks, forward to Telegram
└── config.go           # Config from env vars
```

---

## Config (config.go)

```go
type Config struct {
    TelegramToken  string // TELEGRAM_TOKEN
    CafeServerURL  string // CAFE_SERVER_URL, default http://localhost:3000
    CafeToken      string // CAFE_TOKEN
    DBPath         string // TELEGRAM_DB_PATH, default ./telegram.db
    TrustedUsers   []string // TELEGRAM_TRUSTED_USERS, comma-separated
}
```

---

## Update routing (bot.go)

```go
func (b *Bot) handleUpdate(update tgbotapi.Update) {
    if update.Message == nil { return }

    msg := update.Message
    if !b.isTrusted(msg.From) {
        b.reply(msg, "You are not authorized.")
        return
    }

    switch {
    case msg.IsCommand():
        b.handleCommand(msg)
    case msg.Voice != nil || msg.Audio != nil:
        b.handleAudio(msg)
    case msg.Photo != nil:
        b.handlePhoto(msg)
    default:
        b.handleText(msg)
    }
}
```

---

## Message streaming (stream.go)

When a user sends a message:
1. POST to `/api/sessions/:id/chat`
2. Read SSE stream
3. Send an initial Telegram message "..."
4. As tokens arrive, edit that message with the accumulating text
   (rate-limit edits to ~1/second to avoid Telegram API limits)
5. On `chat.stream_complete`, do a final edit with the complete response

```go
func (b *Bot) streamToTelegram(chatID int64, sessionID, userMessage string) {
    // Send placeholder message
    placeholder, _ := b.api.Send(tgbotapi.NewMessage(chatID, "..."))

    var accumulated strings.Builder
    lastEdit := time.Now()

    for chunk := range b.client.StreamChat(sessionID, userMessage) {
        if chunk.ContentType == "binary" {
            b.sendMedia(chatID, chunk)
            continue
        }
        if chunk.ContentType == "null" && chunk.Annotations["chat.stream_complete"] == true {
            break
        }
        accumulated.WriteString(chunk.Content)
        if time.Since(lastEdit) > 800*time.Millisecond {
            edit := tgbotapi.NewEditMessageText(chatID, placeholder.MessageID, accumulated.String())
            b.api.Send(edit)
            lastEdit = time.Now()
        }
    }

    // Final edit
    edit := tgbotapi.NewEditMessageText(chatID, placeholder.MessageID, accumulated.String())
    b.api.Send(edit)
}
```

---

## Commands

| Command             | Action                                                   |
|---------------------|----------------------------------------------------------|
| `/start`            | Welcome message, create default session if needed        |
| `/sessions`         | Show inline keyboard of available sessions               |
| `/new`              | Create a new session, switch to it                       |
| `/join <id>`        | Switch to an existing session by ID                      |
| `/share`            | Reply with a link to the current session in cafe-web     |
| `/id`               | Reply with the current session ID                        |
| `/subscribe <id>`   | Auto-receive messages from that session                  |
| `/unsubscribe <id>` | Stop auto-receiving from that session                    |
| `/subscriptions`    | List active subscriptions                                |

---

## Trust model

Only users listed in `TELEGRAM_TRUSTED_USERS` (or added via admin API) can use the bot.
Untrusted users receive a rejection message. The trusted user list is stored in the
cafe-server's admin DB and synced on startup.

---

## SQLite schema (db.go)

```sql
CREATE TABLE IF NOT EXISTS user_sessions (
    telegram_user_id INTEGER NOT NULL,
    session_id       TEXT NOT NULL,
    is_active        INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (telegram_user_id, session_id)
);

CREATE TABLE IF NOT EXISTS subscriptions (
    telegram_user_id INTEGER NOT NULL,
    session_id       TEXT NOT NULL,
    PRIMARY KEY (telegram_user_id, session_id)
);
```

---

## Environment variables

| Variable                | Default                    | Description                      |
|-------------------------|----------------------------|----------------------------------|
| `TELEGRAM_TOKEN`        | *(required)*               | Bot token from @BotFather        |
| `CAFE_SERVER_URL`       | `http://localhost:3000`    | cafe-server base URL             |
| `CAFE_TOKEN`            | *(required)*               | API token                        |
| `TELEGRAM_DB_PATH`      | `./telegram.db`            | SQLite path                      |
| `TELEGRAM_TRUSTED_USERS`| *(empty)*                  | Comma-separated user IDs/names   |
